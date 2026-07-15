// SPDX-License-Identifier: GPL-2.0-only
//! The Modrix MCP server: everything a user can do through the GUI or CLI,
//! exposed as Model Context Protocol tools so AI agents can manage mods
//! autonomously. Hand-rolled JSON-RPC 2.0 over stdio (newline-delimited) -
//! the protocol subset MCP needs is small, and `serde_json` is the only
//! dependency it takes (matching the hand-rolled loopback IPC).
//!
//! Run it with `modrix mcp`; point an agent's MCP client at that command.
//! Tools mirror the engine's action layer; resources expose the installed
//! per-game skill files (`modrix://skill/...`) plus built-in guidance, so an
//! agent discovers *how to mod each game well* the moment its plugin is
//! installed.

mod resources;
mod tools;

use std::io::{BufRead, Write};

use serde_json::{Value, json};

pub use tools::Ctx;

/// Largest inbound JSON-RPC frame accepted.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The MCP protocol revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Serve MCP over `input`/`output` until EOF. Blocking; the caller owns the
/// engine for the whole session.
///
/// # Errors
///
/// Returns an I/O error from the transport; protocol-level problems are
/// answered in-band and never kill the session.
pub fn serve(ctx: &Ctx, input: &mut dyn BufRead, output: &mut dyn Write) -> std::io::Result<()> {
    loop {
        let line = match read_frame(input)? {
            Frame::Eof => return Ok(()),
            Frame::Oversized => {
                respond(
                    output,
                    &error_frame(&Value::Null, -32600, "frame too large"),
                )?;
                continue;
            }
            Frame::Line(line) => line,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            respond(output, &error_frame(&Value::Null, -32700, "parse error"))?;
            continue;
        };
        if let Some(reply) = handle(ctx, &frame) {
            respond(output, &reply)?;
        }
    }
}

/// One inbound frame.
enum Frame {
    /// A complete line (newline stripped by the JSON parser's whitespace rule).
    Line(String),
    /// A line past [`MAX_FRAME_BYTES`] - drained and dropped.
    Oversized,
    /// The client hung up.
    Eof,
}

/// Read one newline-terminated frame with an explicit memory bound: an
/// oversized line is drained (never buffered) and reported as such.
fn read_frame(input: &mut dyn BufRead) -> std::io::Result<Frame> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(if buf.is_empty() {
                Frame::Eof
            } else {
                Frame::Line(String::from_utf8_lossy(&buf).into_owned())
            });
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let take = newline.map_or(available.len(), |p| p.saturating_add(1));
        if buf.len().saturating_add(take) > MAX_FRAME_BYTES {
            input.consume(take);
            if newline.is_none() {
                drain_line(input)?;
            }
            return Ok(Frame::Oversized);
        }
        buf.extend_from_slice(available.get(..take).unwrap_or_default());
        input.consume(take);
        if newline.is_some() {
            return Ok(Frame::Line(String::from_utf8_lossy(&buf).into_owned()));
        }
    }
}

/// Discard input up to and including the next newline (or EOF), bounded.
fn drain_line(input: &mut dyn BufRead) -> std::io::Result<()> {
    // 64 MiB of garbage is where we stop humoring the client.
    let mut discarded: usize = 0;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let take = newline.map_or(available.len(), |p| p.saturating_add(1));
        input.consume(take);
        discarded = discarded.saturating_add(take);
        if newline.is_some() || discarded > 64 * 1024 * 1024 {
            return Ok(());
        }
    }
}

/// Route one frame. Notifications (no `id`) get `None` - nothing is written.
fn handle(ctx: &Ctx, frame: &Value) -> Option<Value> {
    let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
    let id = frame.get("id").cloned();
    let params = frame.get("params").cloned().unwrap_or(Value::Null);
    // Notifications never get a response, whatever the method.
    let id = id?;
    let result = match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::list()),
        "tools/call" => Ok(tools::call(ctx, &params)),
        "resources/list" => Ok(resources::list(ctx)),
        "resources/read" => resources::read(ctx, &params),
        other => Err(format!("method not found: {other}")),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => error_frame(&id, -32601, &message),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {}, "resources": {} },
        "serverInfo": {
            "name": "modrix",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions":
            "Modrix mod manager. Start with list_games/get_active_game; read the \
             modrix://skill resources for game-specific guidance. Mutating tools \
             mirror what a user does in the GUI: install mods, resolve conflicts \
             with rules, order plugins, then deploy and verify.",
    })
}

fn error_frame(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn respond(output: &mut dyn Write, frame: &Value) -> std::io::Result<()> {
    let mut text = frame.to_string();
    text.push('\n');
    output.write_all(text.as_bytes())?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = modrix_core::Paths::rooted_at(tmp.path());
        let engine = modrix_core::Engine::open(&paths).unwrap();
        (tmp, Ctx::new(engine).unwrap())
    }

    fn roundtrip(ctx: &Ctx, requests: &str) -> Vec<Value> {
        let mut input = std::io::BufReader::new(requests.as_bytes());
        let mut output = Vec::new();
        serve(ctx, &mut input, &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initialize_handshake_and_tool_listing() {
        let (_tmp, ctx) = ctx();
        let replies = roundtrip(
            &ctx,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n\
             {\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        // The notification got no reply: exactly two frames back.
        assert_eq!(replies.len(), 2);
        assert_eq!(
            replies[0]["result"]["protocolVersion"],
            super::PROTOCOL_VERSION
        );
        let tools = replies[1]["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 25, "expected a full tool surface");
        for tool in tools {
            assert!(tool["name"].is_string());
            assert!(tool["inputSchema"]["type"] == "object");
        }
    }

    #[test]
    fn a_read_tool_and_a_mutating_tool_work() {
        let (tmp, ctx) = ctx();
        // No games yet.
        let replies = roundtrip(
            &ctx,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\
             \"params\":{\"name\":\"list_games\",\"arguments\":{}}}\n",
        );
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(text.trim(), "[]");

        // Register a game through the tool surface, then see it listed.
        let install = tmp.path().join("game");
        std::fs::create_dir_all(&install).unwrap();
        let def = tmp.path().join("game.toml");
        std::fs::write(&def, "api_version = 2\nid = \"t\"\nname = \"T\"\n").unwrap();
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "add_game", "arguments": {
                "def_path": def, "install_path": install,
            }},
        });
        let replies = roundtrip(&ctx, &format!("{request}\n"));
        assert_eq!(replies[0]["result"]["isError"], Value::Bool(false));
        let replies = roundtrip(
            &ctx,
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\
             \"params\":{\"name\":\"list_games\",\"arguments\":{}}}\n",
        );
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"plugin_id\":\"t\""), "got: {text}");
    }

    #[test]
    fn malformed_and_oversized_frames_are_rejected_in_band() {
        let (_tmp, ctx) = ctx();
        let big = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":\"{}\"}}\n",
            "x".repeat(super::MAX_FRAME_BYTES)
        );
        let replies = roundtrip(&ctx, &format!("not json\n{big}"));
        assert_eq!(replies[0]["error"]["code"], -32700);
        assert!(
            replies
                .iter()
                .any(|r| r["error"]["code"] == -32600 || r["error"]["code"] == -32700)
        );
    }

    #[test]
    fn unknown_tools_return_tool_errors_not_crashes() {
        let (_tmp, ctx) = ctx();
        let replies = roundtrip(
            &ctx,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\
             \"params\":{\"name\":\"explode\",\"arguments\":{}}}\n",
        );
        assert_eq!(replies[0]["result"]["isError"], Value::Bool(true));
    }
}
