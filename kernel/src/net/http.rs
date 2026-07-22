//! A minimal HTTP/1.1 client: GET only, one request per connection
//! (`Connection: close`, so `net::tcp`'s "read until FIN" is exactly
//! right), no persistent connections, no pipelining, no HTTPS (there's no
//! TLS implementation in this kernel -- that's a project on the scale of
//! the crypto library this OS doesn't have, see `gui::login`'s doc comment
//! for the same honest gap). Built directly on `net::tcp`'s one-shot
//! blocking client and `net::dns` for name resolution.

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
    BadResponse,
}

/// Fetches `path` from `host` on plain HTTP port 80: resolves the hostname
/// via the real DNS resolver, opens a real TCP connection, sends a real
/// GET request, and parses whatever the server actually sent back.
pub fn get(host: &str, path: &str) -> Result<Response, FetchError> {
    let dns = super::dns_server();
    let ip = resolve(host, dns).ok_or(FetchError::DnsFailed)?;

    let mut stream = super::tcp::connect(ip, 80).ok_or(FetchError::ConnectFailed)?;
    let request = alloc::format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: moonOS/1.0\r\nAccept: text/html,text/plain,*/*\r\nConnection: close\r\n\r\n"
    );
    stream.send(request.as_bytes());
    let raw = stream.read_to_end();
    parse_response(&raw).ok_or(FetchError::BadResponse)
}

/// `host` may itself already be a dotted IPv4 address (typed directly into
/// the Browser's address bar) -- try that first before spending a real DNS
/// round-trip on it.
fn resolve(host: &str, dns: Ipv4Addr) -> Option<Ipv4Addr> {
    if let Some(ip) = parse_ipv4(host) {
        return Some(ip);
    }
    super::dns::query_a(host, dns)
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
