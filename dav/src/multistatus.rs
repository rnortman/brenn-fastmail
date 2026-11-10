//! A `{DAV:}multistatus` answer (RFC 4918 section 13) read into its
//! responses, and a `{DAV:}error` body's precondition (section 16).

use roxmltree::{Document, Node, ParsingOptions};

use crate::error::Error;

/// The DAV namespace (RFC 4918 section 21.1).
pub(crate) const DAV: &str = "DAV:";

/// The longest precondition name [`error_condition`] reports.
const CONDITION_MAX: usize = 64;

/// A document parsed as XML, with no DTD (a DTD is refused, which closes
/// entity expansion).
pub(crate) fn parse(xml: &str) -> Result<Document<'_>, roxmltree::Error> {
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: false,
            ..ParsingOptions::default()
        },
    )
}

/// One `{DAV:}response`: its href, trimmed, and the property elements of
/// every `{DAV:}propstat` whose status line carries a 2xx code.
pub(crate) struct Response<'a, 'input> {
    pub(crate) href: String,
    pub(crate) props: Vec<Node<'a, 'input>>,
}

impl<'a, 'input> Response<'a, 'input> {
    /// The 2xx property `{namespace}name`, when the response has it.
    pub(crate) fn prop(&self, namespace: &str, name: &str) -> Option<Node<'a, 'input>> {
        self.props
            .iter()
            .copied()
            .find(|prop| prop.has_tag_name((namespace, name)))
    }
}

/// `doc`'s responses. A root that is not `{DAV:}multistatus`, or a response
/// with no href, is an error.
pub(crate) fn responses<'a, 'input>(
    doc: &'a Document<'input>,
) -> Result<Vec<Response<'a, 'input>>, Error> {
    let root = doc.root_element();
    if !root.has_tag_name((DAV, "multistatus")) {
        return Err(Error::Multistatus(format!(
            "its root is `{}`, not DAV's multistatus",
            root.tag_name().name()
        )));
    }
    children(root, DAV, "response")
        .map(|response| {
            let href = children(response, DAV, "href")
                .next()
                .ok_or(Error::ResponseWithoutHref)?;
            let props = children(response, DAV, "propstat")
                .filter(|propstat| {
                    children(*propstat, DAV, "status")
                        .next()
                        .is_some_and(|status| is_success(&text(status)))
                })
                .flat_map(|propstat| children(propstat, DAV, "prop"))
                .flat_map(|prop| prop.children().filter(Node::is_element))
                .collect();
            Ok(Response {
                href: text(href).trim().to_owned(),
                props,
            })
        })
        .collect()
}

/// `xml` read as a multi-status: the parse and [`responses`] in one, the
/// parser's error mapped to [`Error::Multistatus`]. `read` sees the
/// responses while the document is alive.
pub(crate) fn read<T>(
    xml: &str,
    read: impl FnOnce(Vec<Response<'_, '_>>) -> Result<T, Error>,
) -> Result<T, Error> {
    let doc = parse(xml).map_err(|e| Error::Multistatus(e.to_string()))?;
    read(responses(&doc)?)
}

/// The element children of `node` named `{namespace}name`.
pub(crate) fn children<'a, 'input>(
    node: Node<'a, 'input>,
    namespace: &'a str,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |child| child.has_tag_name((namespace, name)))
}

/// The text content of `node`: its text and CDATA children, joined.
pub(crate) fn text(node: Node<'_, '_>) -> String {
    node.children()
        .filter(Node::is_text)
        .filter_map(|child| child.text())
        .collect()
}

/// Whether a `{DAV:}status` line (`HTTP/1.1 200 OK`) carries a 2xx code.
fn is_success(line: &str) -> bool {
    line.split_whitespace().nth(1).is_some_and(|code| {
        code.len() == 3 && code.starts_with('2') && code.bytes().all(|b| b.is_ascii_digit())
    })
}

/// The precondition a 4xx `{DAV:}error` body names: the local name of its
/// first element child (`valid-filter`), when that name is at most 64
/// characters. `None` for a body that is not XML, not a DAV error, or names
/// none.
pub fn error_condition(body: &str) -> Option<String> {
    let doc = parse(body).ok()?;
    let root = doc.root_element();
    if !root.has_tag_name((DAV, "error")) {
        return None;
    }
    let name = root.first_element_child()?.tag_name().name();
    (name.chars().count() <= CONDITION_MAX).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hrefs and property local names `xml`'s responses carry.
    fn read_names(xml: &str) -> Result<Vec<(String, Vec<String>)>, Error> {
        read(xml, |responses| {
            Ok(responses
                .into_iter()
                .map(|response| {
                    let names = response
                        .props
                        .iter()
                        .map(|prop| prop.tag_name().name().to_owned())
                        .collect();
                    (response.href, names)
                })
                .collect())
        })
    }

    #[test]
    fn prefixes_and_a_default_namespace_read_alike() {
        let expected = vec![(
            "/dav/principals/".to_owned(),
            vec!["current-user-principal".to_owned()],
        )];
        for xml in [
            include_str!("../fixtures/multistatus_upper.xml"),
            include_str!("../fixtures/multistatus_lower.xml"),
            include_str!("../fixtures/multistatus_default.xml"),
        ] {
            assert_eq!(read_names(xml), Ok(expected.clone()), "{xml}");
        }
    }

    #[test]
    fn only_a_2xx_propstat_is_read() {
        let names = read_names(include_str!("../fixtures/multistatus_mixed.xml")).unwrap();
        assert_eq!(
            names,
            vec![
                (
                    "/dav/calendars/user/alice@example.com/".to_owned(),
                    vec!["resourcetype".to_owned(), "displayname".to_owned()]
                ),
                (
                    "/dav/calendars/user/alice@example.com/gone/".to_owned(),
                    vec![]
                ),
            ]
        );
    }

    #[test]
    fn a_document_that_is_not_a_multistatus_is_refused() {
        assert_eq!(
            read_names(
                r#"<D:multistatus xmlns:D="DAV:"><D:response><D:status>HTTP/1.1 200 OK</D:status></D:response></D:multistatus>"#
            ),
            Err(Error::ResponseWithoutHref)
        );
        assert_eq!(
            read_names(r#"<D:propfind xmlns:D="DAV:"/>"#),
            Err(Error::Multistatus(
                "its root is `propfind`, not DAV's multistatus".to_owned()
            ))
        );
        assert_eq!(
            read_names(r#"<multistatus xmlns="urn:example"/>"#),
            Err(Error::Multistatus(
                "its root is `multistatus`, not DAV's multistatus".to_owned()
            ))
        );
        assert!(matches!(
            read_names("HTTP/1.1 200 OK"),
            Err(Error::Multistatus(_))
        ));
        let dtd = r#"<?xml version="1.0"?>
<!DOCTYPE m [<!ENTITY a "aaaaaaaaaa">]>
<D:multistatus xmlns:D="DAV:"><D:response><D:href>&a;</D:href></D:response></D:multistatus>"#;
        assert!(matches!(read_names(dtd), Err(Error::Multistatus(_))));
    }

    #[test]
    fn an_empty_multistatus_has_no_responses() {
        assert_eq!(
            read_names(r#"<D:multistatus xmlns:D="DAV:"/>"#),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_status_line_is_2xx_by_its_code() {
        assert!(is_success("HTTP/1.1 200 OK"));
        assert!(is_success("  HTTP/1.1 207 Multi-Status "));
        assert!(!is_success("HTTP/1.1 404 Not Found"));
        assert!(!is_success("HTTP/1.1 2000 OK"));
        assert!(!is_success("200"));
        assert!(!is_success(""));
    }

    #[test]
    fn an_error_body_names_its_precondition() {
        assert_eq!(
            error_condition(
                r#"<?xml version="1.0"?><D:error xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:valid-filter/></D:error>"#
            ),
            Some("valid-filter".to_owned())
        );
        assert_eq!(error_condition(r#"<D:error xmlns:D="DAV:"/>"#), None);
        assert_eq!(
            error_condition(r#"<D:multistatus xmlns:D="DAV:"><x/></D:multistatus>"#),
            None
        );
        assert_eq!(error_condition("Forbidden"), None);
        let long = format!(r#"<D:error xmlns:D="DAV:"><{}/></D:error>"#, "a".repeat(65));
        assert_eq!(error_condition(&long), None);
    }
}
