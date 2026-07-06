// SPDX-License-Identifier: GPL-2.0-only
//! A small async HTTP/HTTPS client built on the pure-Rust, GPLv2-clean stack:
//! hyper for HTTP/1.1, rustls for TLS with the RustCrypto crypto provider, and
//! the OS trust store for roots. No reqwest, no ring (see `docs/ARCHITECTURE.md`
//! §11). One connection per request (a download manager is not high-QPS) with
//! bounded redirect following.

use std::sync::Arc;

use http_body_util::Empty;
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::error::Error;

/// Maximum redirects to follow before giving up.
const MAX_REDIRECTS: u8 = 5;

/// A reusable HTTP client. Cheap to clone (shares one TLS config).
#[derive(Clone)]
pub(crate) struct HttpClient {
    tls: Arc<rustls::ClientConfig>,
}

/// The head of a response plus its still-streaming body.
pub(crate) struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Incoming,
}

impl Response {
    /// A header value by (lowercase) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

impl HttpClient {
    /// Build a client with a rustls config using the RustCrypto provider and the
    /// OS trust store.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tls`] if the TLS configuration cannot be built.
    pub(crate) fn new() -> Result<Self, Error> {
        let provider = Arc::new(rustls_rustcrypto::provider());
        let mut roots = rustls::RootCertStore::empty();
        let loaded = rustls_native_certs::load_native_certs();
        for cert in loaded.certs {
            // Skip individual malformed roots rather than failing entirely.
            let _ = roots.add(cert);
        }
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Tls(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            tls: Arc::new(config),
        })
    }

    /// Perform a `GET`, following up to [`MAX_REDIRECTS`] redirects.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Http`] on a transport failure or a redirect loop.
    pub(crate) async fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<Response, Error> {
        let mut current = url.to_owned();
        for _ in 0..MAX_REDIRECTS {
            let response = self.get_once(&current, headers).await?;
            if is_redirect(response.status) {
                match response.header("location") {
                    Some(location) => {
                        current = resolve_redirect(&current, location)?;
                        continue;
                    }
                    None => return Ok(response),
                }
            }
            return Ok(response);
        }
        Err(Error::Http("too many redirects".to_owned()))
    }

    async fn get_once(&self, url: &str, headers: &[(&str, String)]) -> Result<Response, Error> {
        let target = Target::parse(url)?;
        let stream = self.connect(&target).await?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| Error::Http(e.to_string()))?;
        // Drive the connection in the background; it ends when the body is read.
        tokio::spawn(async move {
            let _ = conn.await;
        });

        // Only inject our default UA if the caller didn't supply one - a mod host
        // may gate on the browser's User-Agent, which the extension replays.
        let has_ua = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("user-agent"));
        let mut builder = Request::builder()
            .method("GET")
            .uri(&target.path_and_query)
            .header("host", &target.host)
            .header("connection", "close");
        if !has_ua {
            builder = builder.header("user-agent", "ModManager");
        }
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        let request = builder
            .body(Empty::<Bytes>::new())
            .map_err(|e| Error::Http(e.to_string()))?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or("").to_owned(),
                )
            })
            .collect();
        Ok(Response {
            status,
            headers,
            body: response.into_body(),
        })
    }

    async fn connect(&self, target: &Target) -> Result<Stream, Error> {
        let tcp = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|e| Error::Http(format!("connect {}: {e}", target.host)))?;
        if !target.tls {
            return Ok(Stream::Plain(tcp));
        }
        let name = rustls::pki_types::ServerName::try_from(target.host.clone())
            .map_err(|e| Error::Tls(e.to_string()))?;
        let tls = TlsConnector::from(Arc::clone(&self.tls))
            .connect(name, tcp)
            .await
            .map_err(|e| Error::Tls(e.to_string()))?;
        Ok(Stream::Tls(Box::new(tls)))
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Resolve a `Location` against the current URL (absolute or origin-relative).
fn resolve_redirect(current: &str, location: &str) -> Result<String, Error> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_owned());
    }
    let base = Target::parse(current)?;
    let scheme = if base.tls { "https" } else { "http" };
    if location.starts_with('/') {
        Ok(format!("{scheme}://{}{location}", base.authority()))
    } else {
        Ok(format!("{scheme}://{}/{location}", base.authority()))
    }
}

/// A parsed request target.
struct Target {
    tls: bool,
    host: String,
    port: u16,
    path_and_query: String,
}

impl Target {
    fn parse(url: &str) -> Result<Self, Error> {
        let uri: Uri = url
            .parse()
            .map_err(|_| Error::Http(format!("bad url: {url}")))?;
        let tls = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err(Error::Http(format!("unsupported scheme in {url}"))),
        };
        let host = uri
            .host()
            .ok_or_else(|| Error::Http(format!("no host in {url}")))?
            .to_owned();
        let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
        let path_and_query = uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), ToString::to_string);
        Ok(Self {
            tls,
            host,
            port,
            path_and_query,
        })
    }

    fn authority(&self) -> String {
        let default = if self.tls { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// A connection that is either plain TCP or TLS, unified so hyper sees one type.
enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}
