// SPDX-License-Identifier: GPL-2.0-only
//! A deliberately tiny, bounded HTTP/1.1 read/write for the loopback endpoint.
//!
//! The IPC surface is one endpoint on `127.0.0.1` receiving small, well-formed
//! requests (from `modrix-protocol` and the browser userscript), so a full HTTP
//! server is overkill. This handles exactly what we need - a request line,
//! headers, and a `Content-Length` body - with every read bounded in both size
//! and time so a hostile or stuck local client cannot exhaust us (Power of Ten).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::{Error, Result};

/// Header block ceiling.
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Request/response body ceiling.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Per-read timeout.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Connect timeout for the forwarding client.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The session-token header both sides use.
pub(crate) const TOKEN_HEADER: &str = "x-modrix-token";

/// A parsed inbound request.
pub(crate) struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub token: Option<String>,
    pub body: String,
}

/// Read and parse one request from `stream`, bounded in size and time.
pub(crate) async fn read_request(stream: &mut TcpStream) -> Result<ParsedRequest> {
    let (head, mut body) = read_head(stream).await?;
    let head = std::str::from_utf8(&head).map_err(|_| Error::Malformed("non-utf8 headers"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(Error::Malformed("no request line"))?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .ok_or(Error::Malformed("no method"))?
        .to_owned();
    let path = parts.next().ok_or(Error::Malformed("no path"))?.to_owned();

    let mut token = None;
    let mut content_length = 0_usize;
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(Error::Malformed("bad header"))?;
        match name.trim().to_ascii_lowercase().as_str() {
            TOKEN_HEADER => token = Some(value.trim().to_owned()),
            "content-length" => {
                content_length = value
                    .trim()
                    .parse()
                    .map_err(|_| Error::Malformed("bad length"))?;
            }
            _ => {}
        }
    }
    if content_length > MAX_BODY_BYTES {
        return Err(Error::TooLarge {
            what: "body",
            limit: MAX_BODY_BYTES,
        });
    }
    read_body(stream, &mut body, content_length).await?;
    Ok(ParsedRequest {
        method,
        path,
        token,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// Read up to and including the `\r\n\r\n` header terminator; return
/// `(header_bytes, leftover_body_bytes)`.
async fn read_head(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            let head = buf.get(..pos).unwrap_or_default().to_vec();
            let body_start = pos.checked_add(4).ok_or(Error::Malformed("overflow"))?;
            let rest = buf.get(body_start..).unwrap_or_default().to_vec();
            return Ok((head, rest));
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(Error::TooLarge {
                what: "header",
                limit: MAX_HEADER_BYTES,
            });
        }
        let n = read_chunk(stream, &mut chunk).await?;
        if n == 0 {
            return Err(Error::Malformed("eof before headers"));
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
}

/// Read the remaining body bytes until `content_length` is satisfied.
async fn read_body(
    stream: &mut TcpStream,
    body: &mut Vec<u8>,
    content_length: usize,
) -> Result<()> {
    let mut chunk = [0_u8; 4096];
    while body.len() < content_length {
        if body.len() > MAX_BODY_BYTES {
            return Err(Error::TooLarge {
                what: "body",
                limit: MAX_BODY_BYTES,
            });
        }
        let n = read_chunk(stream, &mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
    body.truncate(content_length);
    Ok(())
}

/// Write a response with CORS headers so the browser userscript can reach us.
pub(crate) async fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: content-type, {TOKEN_HEADER}\r\n\
         Connection: close\r\n\r\n{body}",
        reason = reason(status),
        len = body.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Forward a `POST` to the primary instance and return `(status, body)`.
pub(crate) async fn client_send(
    port: u16,
    path: &str,
    token: &str,
    body: &str,
) -> Result<(u16, String)> {
    let connect = TcpStream::connect(("127.0.0.1", port));
    let mut stream = match timeout(CONNECT_TIMEOUT, connect).await {
        Ok(result) => result?,
        Err(_) => return Err(Error::Timeout),
    };
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{TOKEN_HEADER}: {token}\r\n\
         Content-Type: text/plain\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    );
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    parse_response(&read_to_end(&mut stream).await?)
}

/// Read a whole (bounded) response from `stream` until EOF.
async fn read_to_end(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let ceiling = MAX_HEADER_BYTES.saturating_add(MAX_BODY_BYTES);
    loop {
        if buf.len() > ceiling {
            return Err(Error::TooLarge {
                what: "response",
                limit: ceiling,
            });
        }
        let n = read_chunk(stream, &mut chunk).await?;
        if n == 0 {
            return Ok(buf);
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
}

fn parse_response(buf: &[u8]) -> Result<(u16, String)> {
    let text = String::from_utf8_lossy(buf);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or(Error::Malformed("no response terminator"))?;
    let status_line = head
        .split("\r\n")
        .next()
        .ok_or(Error::Malformed("no status line"))?;
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(Error::Malformed("no status code"))?;
    Ok((status, body.to_owned()))
}

async fn read_chunk(stream: &mut TcpStream, buf: &mut [u8]) -> Result<usize> {
    match timeout(READ_TIMEOUT, stream.read(buf)).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(Error::Timeout),
    }
}

/// Find the first occurrence of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let last = hay.len().checked_sub(needle.len())?;
    (0..=last).find(|&i| match i.checked_add(needle.len()) {
        Some(end) => hay.get(i..end) == Some(needle),
        None => false,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Internal Server Error",
    }
}
