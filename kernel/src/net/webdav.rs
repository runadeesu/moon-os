//! A real WebDAV client (RFC 4918): PROPFIND (directory listing), GET,
//! PUT, MKCOL, DELETE, built on `net::http`'s TCP/TLS-backed request
//! machinery -- makes a real WebDAV server usable as a File Manager
//! "network location," the same real over-the-wire round trip as the
//! Browser app and plain HTTP(S) fetches, just with WebDAV's extra
//! methods and an XML response body instead of HTML.
//!
//! Deliberately not attempted: locking (LOCK/UNLOCK -- this client never
//! holds a lock token, so concurrent-edit conflicts aren't detected),
//! COPY/MOVE, property *patching* (PROPPATCH), or anything beyond the
//! handful of read-only properties (`resourcetype`, `getcontentlength`,
//! `displayname`) needed to render a listing. And the multistatus XML
//! parser (`parse_multistatus`, below) is a small, deliberately narrow
//! text scanner, not a general XML parser -- see its own doc comment.

use super::http::{self, FetchError, Response};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One `<D:response>` entry from a PROPFIND multistatus reply.
pub struct Entry {
    pub href: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub name: Option<String>,
}

/// Finds the first `<...local_name...>...</...local_name>` element in
/// `xml` at or after byte offset `from`, tolerant of a namespace prefix
/// (`D:href`, `d:href`, bare `href`, ...) and case on the tag name itself.
/// Returns `(inner_text, tag_close_end_offset)` so callers can keep
/// scanning forward past repeated elements (e.g. each `<response>`).
/// Doesn't handle same-named nested elements (WebDAV responses don't
/// nest `href` inside `href`, etc.) or CDATA sections -- a real generic
/// XML parser handles those; this only needs to handle the shallow,
/// predictable shape a WebDAV multistatus response actually has.
fn find_tag<'a>(xml: &'a str, local_name: &str, from: usize) -> Option<(&'a str, usize)> {
    let lower = xml.to_ascii_lowercase();
    let mut search_from = from;
    loop {
        let open_lt = lower[search_from..].find('<')? + search_from;
        let open_gt = lower[open_lt..].find('>')? + open_lt;
        let inner = lower[open_lt + 1..open_gt].trim_end();
        if inner.starts_with('/') {
            search_from = open_gt + 1;
            continue;
        }
        let name_part = inner.split([' ', '/']).next().unwrap_or(inner);
        let local = name_part.rsplit(':').next().unwrap_or(name_part);
        if local != local_name {
            search_from = open_gt + 1;
            continue;
        }
        let content_start = open_gt + 1;
        if inner.ends_with('/') {
            // Self-closing, e.g. `<D:collection/>` -- empty content.
            return Some((&xml[content_start..content_start], content_start));
        }
        let mut scan = content_start;
        loop {
            let next_lt = lower[scan..].find('<')? + scan;
            let next_gt = lower[next_lt..].find('>')? + next_lt;
            let next_inner = lower[next_lt + 1..next_gt].trim_end();
            if let Some(stripped) = next_inner.strip_prefix('/') {
                let close_local = stripped.rsplit(':').next().unwrap_or(stripped);
                if close_local == local_name {
                    return Some((&xml[content_start..next_lt], next_gt + 1));
                }
            }
            scan = next_gt + 1;
        }
    }
}

/// Decodes a PROPFIND response body into one [`Entry`] per `<response>`.
pub fn parse_multistatus(xml: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut offset = 0;
    while let Some((content, end)) = find_tag(xml, "response", offset) {
        offset = end;
        let Some((href, _)) = find_tag(content, "href", 0) else {
            continue;
        };
        let is_dir = content.to_ascii_lowercase().contains("collection");
        let size = find_tag(content, "getcontentlength", 0).and_then(|(t, _)| t.trim().parse().ok());
        let name = find_tag(content, "displayname", 0)
            .map(|(t, _)| t.trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(Entry {
            href: href.trim().to_string(),
            is_dir,
            size,
            name,
        });
    }
    out
}

/// Lists `base_url` (a WebDAV collection) one level deep (`Depth: 1`).
pub fn list_dir(base_url: &str) -> Result<Vec<Entry>, FetchError> {
    let body = b"<?xml version=\"1.0\"?><propfind xmlns=\"DAV:\"><prop><resourcetype/><getcontentlength/><displayname/></prop></propfind>";
    let resp = http::request(
        base_url,
        "PROPFIND",
        &[("Depth", "1"), ("Content-Type", "text/xml")],
        body,
    )?;
    Ok(parse_multistatus(&http::body_text(&resp)))
}

fn ok_status(resp: &Response) -> Result<(), FetchError> {
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(FetchError::BadResponse)
    }
}

/// Downloads `url`'s content -- a plain WebDAV `GET`, same as any HTTP GET.
pub fn get(url: &str) -> Result<Vec<u8>, FetchError> {
    let resp = http::request(url, "GET", &[], &[])?;
    ok_status(&resp)?;
    Ok(resp.body)
}

/// Uploads `data` to `url` via `PUT`, creating or overwriting it.
pub fn put(url: &str, data: &[u8]) -> Result<(), FetchError> {
    ok_status(&http::request(url, "PUT", &[], data)?)
}

/// Creates a collection (directory) at `url` via `MKCOL`.
pub fn mkcol(url: &str) -> Result<(), FetchError> {
    ok_status(&http::request(url, "MKCOL", &[], &[])?)
}

/// Deletes the resource at `url`.
pub fn delete(url: &str) -> Result<(), FetchError> {
    ok_status(&http::request(url, "DELETE", &[], &[])?)
}

/// Cross-verified against Python's `xml.etree.ElementTree` parsing the
/// exact same multistatus body (confirmed well-formed XML) and reporting
/// the same href/is_dir/size/displayname for all three `<response>`
/// entries before this fixture was trusted.
pub fn self_test() {
    let xml = concat!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
        "<D:multistatus xmlns:D=\"DAV:\">\n",
        "  <D:response>\n",
        "    <D:href>/webdav/</D:href>\n",
        "    <D:propstat>\n",
        "      <D:prop>\n",
        "        <D:displayname>webdav</D:displayname>\n",
        "        <D:resourcetype><D:collection/></D:resourcetype>\n",
        "      </D:prop>\n",
        "      <D:status>HTTP/1.1 200 OK</D:status>\n",
        "    </D:propstat>\n",
        "  </D:response>\n",
        "  <D:response>\n",
        "    <D:href>/webdav/notes.txt</D:href>\n",
        "    <D:propstat>\n",
        "      <D:prop>\n",
        "        <D:displayname>notes.txt</D:displayname>\n",
        "        <D:getcontentlength>1234</D:getcontentlength>\n",
        "        <D:resourcetype/>\n",
        "      </D:prop>\n",
        "      <D:status>HTTP/1.1 200 OK</D:status>\n",
        "    </D:propstat>\n",
        "  </D:response>\n",
        "  <D:response>\n",
        "    <D:href>/webdav/sub</D:href>\n",
        "    <D:propstat>\n",
        "      <D:prop>\n",
        "        <D:displayname>sub</D:displayname>\n",
        "        <D:resourcetype><D:collection/></D:resourcetype>\n",
        "      </D:prop>\n",
        "      <D:status>HTTP/1.1 200 OK</D:status>\n",
        "    </D:propstat>\n",
        "  </D:response>\n",
        "</D:multistatus>\n",
    );

    let entries = parse_multistatus(xml);
    assert_eq!(entries.len(), 3, "webdav: expected 3 <response> entries");

    assert_eq!(entries[0].href, "/webdav/");
    assert!(entries[0].is_dir);
    assert_eq!(entries[0].size, None);
    assert_eq!(entries[0].name.as_deref(), Some("webdav"));

    assert_eq!(entries[1].href, "/webdav/notes.txt");
    assert!(!entries[1].is_dir);
    assert_eq!(entries[1].size, Some(1234));
    assert_eq!(entries[1].name.as_deref(), Some("notes.txt"));

    assert_eq!(entries[2].href, "/webdav/sub");
    assert!(entries[2].is_dir);
    assert_eq!(entries[2].size, None);
    assert_eq!(entries[2].name.as_deref(), Some("sub"));

    crate::serial_println!(
        "webdav: self-test passed (multistatus XML scanner matches independent ElementTree parse)"
    );
}
