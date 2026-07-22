//! A minimal HTTP/1.1 client: GET only, one request per connection
//! (`Connection: close`), no persistent connections, no pipelining. Plain
//! HTTP runs directly over `net::tcp`; `https://` runs the exact same
//! request/response logic over `net::tls`'s TLS 1.3 client instead -- see
//! that module's doc comment for the honest, serious gap in what "https"
//! means here (real encryption, no certificate authentication).

use super::Ipv4Addr;
use alloc::string::String;
use alloc::vec::Vec;

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum FetchError {
    DnsFailed,
    ConnectFailed,
    TlsFailed(super::tls::TlsError),
    BadResponse,
}

/// Fetches a `http://` or `https://` URL (scheme defaults to `http` if
/// omitted, so a bare `example.com/path` typed into the Browser's address
/// bar still works). This is the entry point `gui::widgets::browser` uses.
pub fn fetch(url: &str) -> Result<Response, FetchError> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        (false, url)
    };
    let (host, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if https {
        get_https(host, path)
    } else {
        get(host, path)
    }
}

/// Fetches `path` from `host` on plain HTTP port 80: resolves the hostname
/// via the real DNS resolver, opens a real TCP connection, sends a real
/// GET request, and parses whatever the server actually sent back.
pub fn get(host: &str, path: &str) -> Result<Response, FetchError> {
    let ip = resolve(host)?;
    let mut stream = super::tcp::connect(ip, 80).ok_or(FetchError::ConnectFailed)?;
    stream.send(request_bytes(host, path).as_bytes());
    let raw = stream.read_to_end();
    parse_response(&raw).ok_or(FetchError::BadResponse)
}

/// Same request, over a real TLS 1.3 connection instead of plain TCP.
pub fn get_https(host: &str, path: &str) -> Result<Response, FetchError> {
    let ip = resolve(host)?;
    let mut stream = super::tls::connect(ip, host).map_err(FetchError::TlsFailed)?;
    stream.send(request_bytes(host, path).as_bytes());
    let raw = stream.read_to_end();
    parse_response(&raw).ok_or(FetchError::BadResponse)
}

fn request_bytes(host: &str, path: &str) -> String {
    alloc::format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: moonOS/1.0\r\nAccept: text/html,text/plain,*/*\r\nConnection: close\r\n\r\n"
    )
}

/// `host` may itself already be a dotted IPv4 address (typed directly into
/// the Browser's address bar) -- try that first before spending a real DNS
/// round-trip on it.
fn resolve(host: &str) -> Result<Ipv4Addr, FetchError> {
    if let Some(ip) = parse_ipv4(host) {
        return Ok(ip);
    }
    super::dns::query_a(host, super::dns_server()).ok_or(FetchError::DnsFailed)
}

fn parse_ipv4(s: &str) -> Option<Ipv4Addr> {
    let mut parts = s.split('.');
    let mut out = [0u8; 4];
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

fn parse_response(raw: &[u8]) -> Option<Response> {
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let header_text = core::str::from_utf8(&raw[..header_end]).ok()?;
    let status_line = header_text.split("\r\n").next()?;
    let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
    Some(Response {
        status,
        body: raw[header_end..].to_vec(),
    })
}

/// A short label for the response body, decoded as UTF-8 on a best-effort
/// basis (lossy: real pages aren't guaranteed to be valid UTF-8, and this
/// has no charset-detection logic).
pub fn body_text(resp: &Response) -> String {
    String::from_utf8_lossy(&resp.body).into_owned()
}
