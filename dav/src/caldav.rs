//! CalDAV (RFC 4791) request bodies and the readers of their multi-status
//! answers: the principal (RFC 5397), its calendar home, the calendars in
//! it, and the calendar objects a `calendar-query` selects.

use roxmltree::Node;

use crate::error::Error;
use crate::multistatus::{self, DAV, Response, children, text};

/// The CalDAV namespace (RFC 4791 section 14).
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";

/// `PROPFIND` body asking who the credential is: `current-user-principal`
/// (RFC 5397 section 3).
pub const PRINCIPAL_PROPFIND: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:"><D:prop><D:current-user-principal/></D:prop></D:propfind>
"#;

/// `PROPFIND` body asking a principal for its `calendar-home-set`
/// (RFC 4791 section 6.2.1).
pub const HOME_PROPFIND: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><C:calendar-home-set/></D:prop></D:propfind>
"#;

/// `PROPFIND` body asking a calendar home's members what they are and what
/// they are called (RFC 4791 sections 5.2.1 and 5.2.3).
pub const CALENDARS_PROPFIND: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:resourcetype/><D:displayname/><C:calendar-description/><C:supported-calendar-component-set/></D:prop></D:propfind>
"#;

/// The `VEVENT` properties a window answer carries. `RRULE` and `RDATE`, the
/// properties that make an event recur (RFC 5545 section 3.8.5), are asked
/// so that an answer the server did not expand can be told from one it did.
const WINDOW_PROPERTIES: [&str; 10] = [
    "UID",
    "SUMMARY",
    "DTSTART",
    "DTEND",
    "DURATION",
    "LOCATION",
    "STATUS",
    "RECURRENCE-ID",
    "RRULE",
    "RDATE",
];

// TODO(calendar-unconfirmed-against-live-fastmail): the server's expansion
// and property subset are written from RFC 4791 and not yet seen from
// Fastmail.
/// `REPORT` body selecting the events that overlap `start`..`end` and
/// asking the server to expand each recurring one into its occurrences in
/// that window (RFC 4791 sections 7.8, 9.6.5 and 9.9). `start` and `end`
/// are UTC instants written `YYYYMMDDTHHMMSSZ`, already checked to be
/// digits, so the body needs no escaping.
pub fn events_report(start: &str, end: &str) -> String {
    let props: String = WINDOW_PROPERTIES
        .iter()
        .map(|name| format!(r#"<C:prop name="{name}"/>"#))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><C:calendar-data><C:comp name="VCALENDAR"><C:comp name="VEVENT">{props}</C:comp></C:comp><C:expand start="{start}" end="{end}"/></C:calendar-data></D:prop><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="{start}" end="{end}"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>
"#
    )
}

/// A calendar collection in a calendar home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    /// The collection's href as the server wrote it.
    pub href: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// A calendar object resource a `calendar-query` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// The resource's href as the server wrote it.
    pub href: String,
    /// Its `calendar-data`: iCalendar text.
    pub data: String,
}

/// The one href in the answer's `current-user-principal`.
pub fn principal(xml: &str) -> Result<String, Error> {
    one_href(xml, DAV, "current-user-principal")
}

/// The one href in the answer's `calendar-home-set`. A principal with
/// several homes is an error: these tools read one.
pub fn calendar_home(xml: &str) -> Result<String, Error> {
    one_href(xml, CALDAV, "calendar-home-set")
}

/// The calendars a `Depth: 1` [`CALENDARS_PROPFIND`] of a home answered:
/// each response whose `resourcetype` holds CalDAV's `calendar` and whose
/// `supported-calendar-component-set`, when present, names `VEVENT`. The
/// home itself, a scheduling inbox or outbox and a task list are left out.
pub fn calendars(xml: &str) -> Result<Vec<Calendar>, Error> {
    multistatus::read(xml, |responses| {
        Ok(responses
            .iter()
            .filter(|response| is_event_calendar(response))
            .map(|response| Calendar {
                href: response.href.clone(),
                name: trimmed(response.prop(DAV, "displayname")),
                description: trimmed(response.prop(CALDAV, "calendar-description")),
            })
            .collect())
    })
}

/// The calendar object resources a [`events_report`] answer carries, each
/// with its `calendar-data`. A response without it is an error naming the
/// href.
pub fn event_resources(xml: &str) -> Result<Vec<Resource>, Error> {
    multistatus::read(xml, |responses| {
        responses
            .into_iter()
            .map(|response| match response.prop(CALDAV, "calendar-data") {
                Some(data) => Ok(Resource {
                    data: text(data).trim().to_owned(),
                    href: response.href,
                }),
                None => Err(Error::NoCalendarData {
                    href: response.href,
                }),
            })
            .collect()
    })
}

/// The one href inside the property `{namespace}name`, across the answer's
/// responses.
fn one_href(xml: &str, namespace: &str, name: &'static str) -> Result<String, Error> {
    multistatus::read(xml, |responses| {
        let props: Vec<Node> = responses
            .iter()
            .filter_map(|response| response.prop(namespace, name))
            .collect();
        if props.is_empty() {
            return Err(Error::Property {
                name,
                problem: "is not in the answer",
            });
        }
        let mut hrefs = props
            .into_iter()
            .flat_map(|prop| children(prop, DAV, "href"))
            .map(|href| text(href).trim().to_owned());
        match (hrefs.next(), hrefs.next()) {
            (Some(href), None) => Ok(href),
            (None, _) => Err(Error::Property {
                name,
                problem: "holds no href",
            }),
            (Some(_), Some(_)) => Err(Error::Property {
                name,
                problem: "holds more than one href",
            }),
        }
    })
}

fn is_event_calendar(response: &Response<'_, '_>) -> bool {
    let calendar = response
        .prop(DAV, "resourcetype")
        .is_some_and(|kind| children(kind, CALDAV, "calendar").next().is_some());
    let events = response
        .prop(CALDAV, "supported-calendar-component-set")
        .is_none_or(|set| {
            children(set, CALDAV, "comp").any(|comp| comp.attribute("name") == Some("VEVENT"))
        });
    calendar && events
}

/// A property's text, trimmed; `None` when absent or blank.
fn trimmed(prop: Option<Node<'_, '_>>) -> Option<String> {
    let text = text(prop?);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multistatus::parse;

    /// `{namespace}name` elements below `node`, in document order.
    fn named<'a, 'input>(
        node: Node<'a, 'input>,
        namespace: &'a str,
        name: &'a str,
    ) -> Vec<Node<'a, 'input>> {
        node.descendants()
            .filter(|n| n.has_tag_name((namespace, name)))
            .collect()
    }

    /// The local names of the property elements a `PROPFIND` body asks.
    fn asked(body: &str) -> Vec<(String, String)> {
        let doc = parse(body).unwrap();
        let root = doc.root_element();
        assert!(root.has_tag_name((DAV, "propfind")));
        let prop = children(root, DAV, "prop").next().unwrap();
        prop.children()
            .filter(Node::is_element)
            .map(|p| {
                (
                    p.tag_name().namespace().unwrap().to_owned(),
                    p.tag_name().name().to_owned(),
                )
            })
            .collect()
    }

    fn pair(namespace: &str, name: &str) -> (String, String) {
        (namespace.to_owned(), name.to_owned())
    }

    #[test]
    fn each_propfind_asks_its_properties() {
        assert_eq!(
            asked(PRINCIPAL_PROPFIND),
            vec![pair(DAV, "current-user-principal")]
        );
        assert_eq!(
            asked(HOME_PROPFIND),
            vec![pair(CALDAV, "calendar-home-set")]
        );
        assert_eq!(
            asked(CALENDARS_PROPFIND),
            vec![
                pair(DAV, "resourcetype"),
                pair(DAV, "displayname"),
                pair(CALDAV, "calendar-description"),
                pair(CALDAV, "supported-calendar-component-set"),
            ]
        );
    }

    #[test]
    fn the_window_query_filters_by_time_and_expands() {
        let body = events_report("20261102T000000Z", "20261109T000000Z");
        let doc = parse(&body).unwrap();
        let root = doc.root_element();
        assert!(root.has_tag_name((CALDAV, "calendar-query")));

        let data = named(root, CALDAV, "calendar-data");
        assert_eq!(data.len(), 1);
        let comps: Vec<_> = named(data[0], CALDAV, "comp")
            .into_iter()
            .map(|c| c.attribute("name").unwrap())
            .collect();
        assert_eq!(comps, ["VCALENDAR", "VEVENT"]);
        let event = named(data[0], CALDAV, "comp")[1];
        let props: Vec<_> = children(event, CALDAV, "prop")
            .map(|p| p.attribute("name").unwrap())
            .collect();
        assert_eq!(props, WINDOW_PROPERTIES);
        let expand = children(data[0], CALDAV, "expand").next().unwrap();
        assert_eq!(expand.attribute("start"), Some("20261102T000000Z"));
        assert_eq!(expand.attribute("end"), Some("20261109T000000Z"));

        let filter = children(root, CALDAV, "filter").next().unwrap();
        let calendar = children(filter, CALDAV, "comp-filter").next().unwrap();
        assert_eq!(calendar.attribute("name"), Some("VCALENDAR"));
        let event = children(calendar, CALDAV, "comp-filter").next().unwrap();
        assert_eq!(event.attribute("name"), Some("VEVENT"));
        let range = children(event, CALDAV, "time-range").next().unwrap();
        assert_eq!(range.attribute("start"), Some("20261102T000000Z"));
        assert_eq!(range.attribute("end"), Some("20261109T000000Z"));
    }

    #[test]
    fn the_principal_and_home_are_one_href_each() {
        assert_eq!(
            principal(include_str!("../fixtures/multistatus_upper.xml")),
            Ok("/dav/principals/user/alice@example.com/".to_owned())
        );
        assert_eq!(
            calendar_home(include_str!("../fixtures/home.xml")),
            Ok("/dav/calendars/user/alice@example.com/".to_owned())
        );
        assert_eq!(
            calendar_home(include_str!("../fixtures/home_two.xml")),
            Err(Error::Property {
                name: "calendar-home-set",
                problem: "holds more than one href",
            })
        );
        assert_eq!(
            principal(include_str!("../fixtures/principal_missing.xml")),
            Err(Error::Property {
                name: "current-user-principal",
                problem: "is not in the answer",
            })
        );
        assert_eq!(
            principal(include_str!("../fixtures/principal_unauthenticated.xml")),
            Err(Error::Property {
                name: "current-user-principal",
                problem: "holds no href",
            })
        );
    }

    #[test]
    fn a_home_lists_only_its_event_calendars() {
        let calendars = calendars(include_str!("../fixtures/calendars.xml")).unwrap();
        let home = "/dav/calendars/user/alice@example.com";
        assert_eq!(
            calendars,
            vec![
                Calendar {
                    href: format!("{home}/work/"),
                    name: Some("Work".to_owned()),
                    description: Some("Meetings & travel".to_owned()),
                },
                Calendar {
                    href: format!("{home}/family/"),
                    name: Some("Family".to_owned()),
                    description: None,
                },
                Calendar {
                    href: format!("{home}/unnamed/"),
                    name: None,
                    description: None,
                },
            ]
        );
    }

    #[test]
    fn calendar_data_is_read_escaped_or_in_cdata() {
        // XML reads each CRLF of the fixture as LF (XML 1.0 section 2.11).
        let resources = event_resources(include_str!("../fixtures/events.xml")).unwrap();
        let base = "/dav/calendars/user/alice@example.com/work";
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].href, format!("{base}/escaped.ics"));
        assert!(
            resources[0]
                .data
                .starts_with("BEGIN:VCALENDAR\nBEGIN:VEVENT\n"),
            "{:?}",
            resources[0].data
        );
        assert!(resources[0].data.contains("SUMMARY:Lunch & <talk>\n"));
        assert_eq!(resources[1].href, format!("{base}/cdata.ics"));
        assert!(
            resources[1]
                .data
                .contains("SUMMARY:Review <draft> & sign\n")
        );
        assert!(resources[1].data.ends_with("END:VCALENDAR"));
        assert_eq!(
            event_resources(r#"<D:multistatus xmlns:D="DAV:"/>"#),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_resource_without_calendar_data_is_an_error() {
        assert_eq!(
            event_resources(include_str!("../fixtures/events_no_data.xml")),
            Err(Error::NoCalendarData {
                href: "/dav/calendars/user/alice@example.com/work/bare.ics".to_owned()
            })
        );
    }
}
