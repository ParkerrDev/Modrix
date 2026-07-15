// SPDX-License-Identifier: GPL-2.0-only
//! Single-instance guard and loopback IPC.
//!
//! The loopback listener *is* the single-instance mechanism: binding
//! `127.0.0.1:<port>` succeeds only for the primary instance; a second launch
//! (or `modrix-protocol` forwarding an `nxm://` link) finds the port taken,
//! reads the session token from the lockfile, and forwards its request to the
//! primary instead of starting a duplicate. If nothing is running, the caller
//! becomes primary and can service browser clicks headlessly.
//!
//! Security: loopback-only, and a request is authorized one of two ways:
//! it carries the per-session token written to the (user-private) lockfile
//! (secondaries and `modrix-protocol` read it there), or its `Origin` header
//! is a browser-extension origin (`chrome-extension://…`, `moz-extension://…`).
//! Browsers stamp `Origin` themselves and web pages cannot forge another
//! scheme, so the extension works with zero configuration while a drive-by
//! website (an `http(s)://` origin, or none) is still refused without the
//! token.

mod error;
mod wire;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub use error::{Error, Result};

/// The default loopback port. Fixed so that binding it is the single-instance
/// mutex; overridable for tests and unusual setups.
pub const DEFAULT_PORT: u16 = 41_015;

/// One inbound request handed to the primary's handler.
#[derive(Debug, Clone)]
pub struct Message {
    /// The request path (e.g. `/nxm`).
    pub path: String,
    /// The request body (e.g. the `nxm://` URL).
    pub body: String,
}

/// A handler's reply.
#[derive(Debug, Clone)]
pub struct Reply {
    /// HTTP status code.
    pub status: u16,
    /// Response body.
    pub body: String,
}

impl Reply {
    /// A `200 OK` reply.
    #[must_use]
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }

    /// A `400 Bad Request` reply.
    #[must_use]
    pub fn bad_request(body: impl Into<String>) -> Self {
        Self {
            status: 400,
            body: body.into(),
        }
    }

    /// A `500` reply carrying an error message.
    #[must_use]
    pub fn error(body: impl Into<String>) -> Self {
        Self {
            status: 500,
            body: body.into(),
        }
    }
}

/// The outcome of [`acquire`]: either we hold the instance, or one already runs.
pub enum Role {
    /// We are the sole instance; serve requests.
    Primary(Primary),
    /// Another instance holds the port; forward to it.
    Secondary(Secondary),
    /// The port is taken but no readable lockfile pairs with it - some other
    /// program owns the port, or a crashed instance left no lock. Hand-off
    /// is unavailable, but the caller should keep running.
    PortTaken,
}

/// Try to become the primary instance by binding `port`; if it is already taken,
/// return a [`Secondary`] that can forward to the running primary.
///
/// The lockfile at `lockfile` stores the bound port and session token so a
/// secondary (or `modrix-protocol`) can authenticate to the primary.
///
/// # Errors
///
/// Returns [`Error::Io`] on an unexpected bind failure, [`Error::Random`] if a
/// token cannot be generated, or [`Error::Lockfile`] if the port is held but the
/// lockfile is missing or unreadable (e.g. an unrelated process on the port).
pub fn acquire(lockfile: &Path, port: u16) -> Result<Role> {
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            let token = generate_token()?;
            let port = listener.local_addr()?.port();
            write_lock(lockfile, port, &token)?;
            Ok(Role::Primary(Primary {
                listener,
                token,
                port,
            }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // No readable lockfile alongside a taken port must not abort the
            // caller: the port owner may not be a Modrix instance at all.
            match read_lock(lockfile) {
                Ok(info) => Ok(Role::Secondary(Secondary {
                    port: info.port,
                    token: info.token,
                })),
                Err(_) => Ok(Role::PortTaken),
            }
        }
        Err(e) => Err(Error::Io(e)),
    }
}

/// Build a [`Secondary`] from an existing lockfile without binding anything -
/// how `modrix-protocol` reaches a running primary to forward an `nxm://` link.
///
/// # Errors
///
/// Returns [`Error::Lockfile`] if the lockfile is missing or unreadable (i.e. no
/// primary is running).
pub fn secondary_from_lock(lockfile: &Path) -> Result<Secondary> {
    let info = read_lock(lockfile)?;
    Ok(Secondary {
        port: info.port,
        token: info.token,
    })
}

/// Whether a live primary instance is currently reachable, judged from the
/// lockfile **without binding the port** - so it is safe to call before
/// [`acquire`], where a bind-probe would race the real acquire.
///
/// Reads the recorded port and opens a short-lived loopback connection to it. A
/// crashed primary leaves a stale lockfile but nothing listening, so the connect
/// fails and this returns `false`; a missing or unreadable lockfile likewise
/// reads as "no primary". A frontend uses this to refuse a second window rather
/// than open one in a degraded mode.
#[must_use]
pub fn primary_is_live(lockfile: &Path) -> bool {
    let Ok(info) = read_lock(lockfile) else {
        return false;
    };
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, info.port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

/// The primary instance: owns the bound socket and serves authenticated requests.
pub struct Primary {
    listener: std::net::TcpListener,
    token: String,
    port: u16,
}

impl Primary {
    /// The session token secondaries and the browser must present.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The bound port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Serve requests forever, invoking `handler` for each authenticated one.
    /// Unauthenticated requests get `401`; `OPTIONS` preflights get `204`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the accept loop fails fatally.
    pub async fn serve<H, F>(self, handler: H) -> Result<()>
    where
        H: Fn(Message) -> F + Clone + Send + Sync + 'static,
        F: std::future::Future<Output = Reply> + Send,
    {
        self.listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(self.listener)?;
        let token = std::sync::Arc::new(self.token);
        loop {
            let (stream, _peer) = listener.accept().await?;
            let handler = handler.clone();
            let token = std::sync::Arc::clone(&token);
            // Per-connection errors are isolated: one bad client never stops the
            // listener.
            tokio::spawn(async move {
                let _ = handle_connection(stream, &token, handler).await;
            });
        }
    }
}

async fn handle_connection<H, F>(
    mut stream: tokio::net::TcpStream,
    token: &str,
    handler: H,
) -> Result<()>
where
    H: Fn(Message) -> F,
    F: std::future::Future<Output = Reply>,
{
    let request = wire::read_request(&mut stream).await?;
    let origin = request.origin.clone();
    if request.method.eq_ignore_ascii_case("OPTIONS") {
        return wire::write_response(&mut stream, 204, "", origin.as_deref()).await;
    }
    if !authorized(&request, token) {
        return wire::write_response(&mut stream, 401, "unauthorized", origin.as_deref()).await;
    }
    let reply = handler(Message {
        path: request.path,
        body: request.body,
    })
    .await;
    wire::write_response(&mut stream, reply.status, &reply.body, origin.as_deref()).await
}

/// Whether a request may reach the handler: the session token always works;
/// a browser-extension `Origin` works without one (see the module docs).
fn authorized(request: &wire::ParsedRequest, token: &str) -> bool {
    if request.token.as_deref() == Some(token) {
        return true;
    }
    request.origin.as_deref().is_some_and(is_extension_origin)
}

/// Whether `origin` is a browser-extension origin. Requires a non-empty
/// extension id after the scheme so a bare scheme cannot slip through.
fn is_extension_origin(origin: &str) -> bool {
    const SCHEMES: [&str; 3] = [
        "chrome-extension://",
        "moz-extension://",
        "safari-web-extension://",
    ];
    SCHEMES
        .iter()
        .any(|scheme| origin.len() > scheme.len() && origin.starts_with(scheme))
}

/// A handle to the running primary instance, used to forward a request to it.
pub struct Secondary {
    port: u16,
    token: String,
}

impl Secondary {
    /// Forward a request to the primary and return its reply.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`]/[`Error::Timeout`] if the primary cannot be reached,
    /// or [`Error::Malformed`] if its response is unintelligible.
    pub async fn send(&self, path: &str, body: &str) -> Result<Reply> {
        let (status, body) = wire::client_send(self.port, path, &self.token, body).await?;
        Ok(Reply { status, body })
    }
}

// --- token + lockfile ------------------------------------------------------

/// The on-disk single-instance record.
#[derive(serde::Serialize, serde::Deserialize)]
struct LockInfo {
    port: u16,
    token: String,
}

fn generate_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
    let mut token = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}

fn write_lock(lockfile: &Path, port: u16, token: &str) -> Result<()> {
    let info = LockInfo {
        port,
        token: token.to_owned(),
    };
    let json = serde_json::to_vec(&info).map_err(|e| Error::Lockfile(e.to_string()))?;
    if let Some(parent) = lockfile.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_sibling(lockfile);
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, lockfile)?;
    Ok(())
}

fn read_lock(lockfile: &Path) -> Result<LockInfo> {
    let bytes = std::fs::read(lockfile)
        .map_err(|e| Error::Lockfile(format!("cannot read {}: {e}", lockfile.display())))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Lockfile(e.to_string()))
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".tmp.");
    name.push(
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("instance")),
    );
    path.parent()
        .map_or_else(|| PathBuf::from(&name), |dir| dir.join(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lockfile() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.json");
        (dir, path)
    }

    #[tokio::test]
    async fn primary_serves_authenticated_requests() {
        let (_dir, lock) = lockfile();
        let Role::Primary(primary) = acquire(&lock, 0).unwrap() else {
            panic!("first acquire should be primary");
        };
        let port = primary.port();
        let token = primary.token().to_owned();

        tokio::spawn(primary.serve(|msg: Message| async move {
            Reply::ok(format!("got {}::{}", msg.path, msg.body))
        }));
        // Give the listener a moment to start accepting.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let secondary = Secondary { port, token };
        let reply = secondary
            .send("/nxm", "nxm://game/mods/1/files/2")
            .await
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, "got /nxm::nxm://game/mods/1/files/2");
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let (_dir, lock) = lockfile();
        let Role::Primary(primary) = acquire(&lock, 0).unwrap() else {
            panic!("primary");
        };
        let port = primary.port();
        tokio::spawn(primary.serve(|_m: Message| async { Reply::ok("secret") }));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let impostor = Secondary {
            port,
            token: "deadbeef".to_owned(),
        };
        let reply = impostor.send("/nxm", "x").await.unwrap();
        assert_eq!(reply.status, 401);
    }

    /// Send a raw HTTP request and return `(status, response_head)`.
    async fn send_raw(port: u16, request: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap();
        (status, text)
    }

    fn spawn_echo_primary(lock: &Path) -> u16 {
        let Role::Primary(primary) = acquire(lock, 0).unwrap() else {
            panic!("primary");
        };
        let port = primary.port();
        tokio::spawn(primary.serve(|_m: Message| async { Reply::ok("served") }));
        port
    }

    #[tokio::test]
    async fn extension_origin_is_authorized_without_a_token() {
        let (_dir, lock) = lockfile();
        let port = spawn_echo_primary(&lock);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let origin = "moz-extension://0f9a1b2c-3d4e";
        let (status, head) = send_raw(
            port,
            &format!(
                "POST /downloads HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: {origin}\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(status, 200, "extension origins are zero-config");
        // CORS must echo the specific origin, not the wildcard.
        assert!(
            head.contains(&format!("Access-Control-Allow-Origin: {origin}")),
            "got: {head}"
        );
    }

    #[tokio::test]
    async fn web_page_origin_without_a_token_is_rejected() {
        let (_dir, lock) = lockfile();
        let port = spawn_echo_primary(&lock);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        for origin in ["https://evil.example", "http://127.0.0.1:8080", "null"] {
            let (status, _head) = send_raw(
                port,
                &format!(
                    "POST /downloads HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: {origin}\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            )
            .await;
            assert_eq!(status, 401, "web origin {origin} must still need the token");
        }
    }

    #[test]
    fn extension_origin_check_requires_a_real_id() {
        assert!(is_extension_origin("chrome-extension://abcdef"));
        assert!(is_extension_origin("safari-web-extension://ABC-123"));
        assert!(!is_extension_origin("chrome-extension://"));
        assert!(!is_extension_origin("https://chrome-extension.example"));
        assert!(!is_extension_origin(""));
    }

    #[tokio::test]
    async fn second_acquire_on_same_port_is_secondary() {
        let (_dir, lock) = lockfile();
        let Role::Primary(primary) = acquire(&lock, 0).unwrap() else {
            panic!("primary");
        };
        let port = primary.port();
        // Binding the held port again must fall through to Secondary, carrying
        // the token from the lockfile.
        match acquire(&lock, port).unwrap() {
            Role::Secondary(secondary) => {
                assert_eq!(secondary.token, primary.token());
            }
            Role::Primary(_) | Role::PortTaken => panic!("second acquire must be secondary"),
        }
    }

    #[tokio::test]
    async fn taken_port_without_a_lockfile_degrades_to_port_taken() {
        let (_dir, lock) = lockfile();
        let Role::Primary(primary) = acquire(&lock, 0).unwrap() else {
            panic!("primary");
        };
        // A *different* data dir has no lockfile pairing with the held port -
        // the caller must keep running with hand-off unavailable, not die.
        let (_other_dir, other_lock) = lockfile();
        match acquire(&other_lock, primary.port()).unwrap() {
            Role::PortTaken => {}
            _ => panic!("expected PortTaken"),
        }
    }

    #[test]
    fn primary_is_live_sees_a_held_port() {
        let (_dir, lock) = lockfile();
        let Role::Primary(primary) = acquire(&lock, 0).unwrap() else {
            panic!("primary");
        };
        // The listener is bound (held by `primary`), so a connect succeeds even
        // without an accept loop running.
        assert!(primary_is_live(&lock));
        drop(primary);
    }

    #[test]
    fn primary_is_live_is_false_for_a_stale_lock() {
        let (_dir, lock) = lockfile();
        // Port 1 is privileged and unbound for this user: connect is refused, so
        // a lockfile pointing there reads as a crashed/stale primary.
        write_lock(&lock, 1, "stale").unwrap();
        assert!(!primary_is_live(&lock));
    }

    #[test]
    fn primary_is_live_is_false_without_a_lockfile() {
        let (_dir, lock) = lockfile();
        assert!(!primary_is_live(&lock));
    }
}
