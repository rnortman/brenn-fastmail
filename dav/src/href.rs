//! A DAV href as the path brenn's `api-request` will send.
//!
//! A server answers an href as an absolute path or as an absolute URL
//! (RFC 4918 section 8.3). A handle handed to an agent is a path that
//! `api-request` accepts as it stands, so this module holds an href to the
//! rule `api-request` applies to a request path: begins with `/`, at most one
//! trailing `/`, and every segment, percent-decoded once, is UTF-8, not empty,
//! not `.` or `..`, and holds no `/`, `\` or `%`.

use crate::error::Error;

/// The longest path this crate hands out, in bytes.
pub const MAX_PATH: usize = 1024;

/// `href` as the path to send to `host`: an href beginning with `/` as it
/// stands, one beginning `https://<host>/` (the host compared without case)
/// with that prefix taken off, anything else refused. The path is then held
/// to `api-request`'s rule and to [`MAX_PATH`].
pub fn path(href: &str, host: &str) -> Result<String, Error> {
    let refuse = |reason| Error::Href {
        href: href.to_owned(),
        reason,
    };
    let path = if href.starts_with('/') {
        href
    } else {
        on_host(href, host).ok_or_else(|| refuse("is neither a path nor a URL on the host"))?
    };
    if path.len() > MAX_PATH {
        return Err(refuse("is longer than 1024 bytes"));
    }
    if path.contains(['?', '#']) {
        return Err(refuse("carries a query or a fragment"));
    }
    let rest = &path[1..];
    if rest.is_empty() {
        return Ok(path.to_owned());
    }
    for segment in rest.strip_suffix('/').unwrap_or(rest).split('/') {
        check_segment(segment).map_err(refuse)?;
    }
    Ok(path.to_owned())
}

/// Whether `path` names a collection: it ends in `/`.
pub fn is_collection(path: &str) -> bool {
    path.ends_with('/')
}

/// The path of `url` when it is `https://<host>/...`, from its `/` on.
fn on_host<'a>(url: &'a str, host: &str) -> Option<&'a str> {
    let scheme = "https://";
    let prefix = url.get(..scheme.len() + host.len())?;
    if !prefix.eq_ignore_ascii_case(&format!("{scheme}{host}")) {
        return None;
    }
    let path = &url[prefix.len()..];
    path.starts_with('/').then_some(path)
}

/// A raw segment checked as `api-request` checks it once decoded.
fn check_segment(segment: &str) -> Result<(), &'static str> {
    let decoded = decode(segment).ok_or("has a segment that does not percent-decode to UTF-8")?;
    if decoded.is_empty() || decoded == "." || decoded == ".." {
        return Err("has an empty, `.` or `..` segment");
    }
    if decoded.contains(['/', '\\', '%']) {
        return Err("has a segment that decodes to hold `/`, `\\` or `%`");
    }
    Ok(())
}

/// `segment` percent-decoded once. A `%` not followed by two hex digits
/// stays as it is, which [`check_segment`] then refuses. `None` when the
/// decoded bytes are not UTF-8.
fn decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'%'
            && let Some(hex) = bytes.get(at + 1..at + 3)
            && hex.iter().all(u8::is_ascii_hexdigit)
        {
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(byte);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "caldav.example.com";

    fn refused(href: &str) -> &'static str {
        match path(href, HOST) {
            Err(Error::Href { reason, .. }) => reason,
            other => panic!("{href:?} was not refused: {other:?}"),
        }
    }

    #[test]
    fn a_path_is_taken_as_it_stands() {
        for href in [
            "/",
            "/dav/calendars/user/alice@example.com/",
            "/dav/calendars/user/alice%40example.com/work/",
            "/dav/calendars/user/alice@example.com/work/event-1.ics",
            "/a%20b/",
        ] {
            assert_eq!(path(href, HOST), Ok(href.to_owned()), "{href}");
        }
    }

    #[test]
    fn a_url_on_the_host_loses_its_prefix() {
        assert_eq!(
            path(
                "https://CalDAV.Example.com/dav/principals/user/alice/",
                HOST
            ),
            Ok("/dav/principals/user/alice/".to_owned())
        );
        assert_eq!(
            refused("https://other.example.com/dav/"),
            "is neither a path nor a URL on the host"
        );
        assert_eq!(
            refused("http://caldav.example.com/dav/"),
            "is neither a path nor a URL on the host"
        );
        assert_eq!(
            refused("https://caldav.example.com.evil.example/dav/"),
            "is neither a path nor a URL on the host"
        );
        assert_eq!(
            refused("https://caldav.example.com:8443/dav/"),
            "is neither a path nor a URL on the host"
        );
        assert_eq!(
            refused("dav/calendars/"),
            "is neither a path nor a URL on the host"
        );
    }

    #[test]
    fn a_segment_brenn_will_not_send_is_refused() {
        for href in [
            "/a/%2F/", "/a/%2f/", "/a/%5C/", "/a/%25/", "/a/b%/", "/a/b\\c/",
        ] {
            assert_eq!(
                refused(href),
                "has a segment that decodes to hold `/`, `\\` or `%`",
                "{href}"
            );
        }
        for href in ["/a/./", "/a/../", "/a//b/", "/a/%2e%2E/", "/a//", "//"] {
            assert_eq!(refused(href), "has an empty, `.` or `..` segment", "{href}");
        }
        assert_eq!(
            refused("/a/%ff/"),
            "has a segment that does not percent-decode to UTF-8"
        );
    }

    #[test]
    fn a_query_a_fragment_or_a_long_path_is_refused() {
        assert_eq!(refused("/a/?x=1"), "carries a query or a fragment");
        assert_eq!(refused("/a/#x"), "carries a query or a fragment");
        let long = format!("/{}", "a".repeat(MAX_PATH));
        assert_eq!(refused(&long), "is longer than 1024 bytes");
        let fits = format!("/{}", "a".repeat(MAX_PATH - 1));
        assert_eq!(path(&fits, HOST), Ok(fits.clone()));
    }

    #[test]
    fn a_collection_ends_in_a_slash() {
        assert!(is_collection("/dav/calendars/user/alice/work/"));
        assert!(!is_collection("/dav/calendars/user/alice/work/event.ics"));
    }
}
