//! A stored calendar resource as the write tools take it: its events read
//! by the rule the read tools select them by, the refusal of an event that
//! involves other people, and an update's edit of the main event, which
//! leaves every line it does not name as it was read.

use brenn_provider_kit::Failure;
use fastmail_dav::Error;
use fastmail_dav::ical::{self, Component, Property, When};
use fastmail_provider::event::{recurrence_id, selected};
use fastmail_provider::failure::invalid;

/// The refusal of an event with an organizer or attendees.
pub const INVOLVES_PEOPLE: &str = "this event involves other people (it has an organizer or \
     attendees). These tools do not change or delete such an event, because Fastmail would send \
     mail about it from the user's account: change it in Fastmail";

/// The properties that make a stored event a scheduling object (RFC 6638
/// section 3.1), or name who would be told of a change to it.
const PEOPLE: [&str; 2] = ["ORGANIZER", "ATTENDEE"];

/// The events of one stored resource, as the write tools decide on them.
#[derive(Debug, PartialEq)]
pub struct Resource<'a> {
    /// The event the read tools show as the resource's own: the main event,
    /// or with none the changed occurrence with the lowest `RECURRENCE-ID`
    /// as written.
    pub main: &'a Component,
    /// The event with no `RECURRENCE-ID`, when there is one.
    pub master: Option<&'a Component>,
    /// How many events carry a `RECURRENCE-ID`: occurrences changed apart
    /// from the series.
    pub overrides: usize,
}

impl Resource<'_> {
    /// Whether the resource is a series: its main event has `RRULE` or
    /// `RDATE`, or the resource holds a changed occurrence.
    pub fn recurring(&self) -> bool {
        self.overrides > 0
            || self.master.is_some_and(|master| {
                master.property("RRULE").is_some() || master.property("RDATE").is_some()
            })
    }
}

// TODO(calendar-write-refuses-events-with-attendees): an event with an
// organizer or attendees is refused, since storing a change to it makes
// Fastmail mail the other people; who is told is the owner's question.
/// The resource at `path`, read from `calendar`: its events as the read
/// tools select them, every one of them, overrides included, held to having
/// no organizer and no attendee. Data with no event is the caller's failure,
/// two main events or two alike overrides are malformed, and an event with
/// other people on it is refused.
pub fn read<'a>(path: &str, calendar: &'a Component) -> Result<Resource<'a>, Failure> {
    let all = selected(path, calendar)?;
    if all
        .iter()
        .any(|event| PEOPLE.iter().any(|name| event.property(name).is_some()))
    {
        return Err(invalid(INVOLVES_PEOPLE.to_owned()));
    }
    let main = *all.first().expect("`selected` is never empty");
    let master = Some(main).filter(|event| recurrence_id(event).is_none());
    Ok(Resource {
        main,
        master,
        overrides: all.len() - usize::from(master.is_some()),
    })
}

/// What an update changes in the main event. TEXT values are as the caller
/// gave them; [`edit`] escapes them.
#[derive(Debug, Default, PartialEq)]
pub struct Edit<'a> {
    pub summary: Option<&'a str>,
    pub description: Option<&'a str>,
    pub location: Option<&'a str>,
    /// The new `DTSTART` and `DTEND`.
    pub times: Option<(&'a When, &'a When)>,
    /// The clock `DTSTAMP` and `LAST-MODIFIED` are stamped with, in
    /// milliseconds since the epoch.
    pub now_ms: u64,
}

/// `event`'s first property `name` replaced in its place by `property`, any
/// later one of that name removed; `property` appended when there was none.
fn replace(event: &mut Component, property: Property) {
    let mut placed = false;
    let mut kept = Vec::with_capacity(event.properties.len() + 1);
    for old in event.properties.drain(..) {
        if old.name != property.name {
            kept.push(old);
        } else if !placed {
            kept.push(property.clone());
            placed = true;
        }
    }
    if !placed {
        kept.push(property);
    }
    event.properties = kept;
}

/// Every property of `event` named one of `names` removed.
fn remove(event: &mut Component, names: &[&str]) {
    event
        .properties
        .retain(|property| !names.contains(&property.name.as_str()));
}

/// `calendar` with `master`, its main event as [`read`] gives it, changed as
/// `change` says, and nothing else: each text given replaces the property of
/// its name, which takes the old one's parameters with it; a new description
/// drops `X-ALT-DESC`, a new location `GEO` and
/// `X-APPLE-STRUCTURED-LOCATION`, since each restates the old text; new
/// times replace `DTSTART` and `DTEND` and drop `DURATION`, which an event
/// holds instead of `DTEND` (RFC 5545 section 3.6.1); and `DTSTAMP` and
/// `LAST-MODIFIED` are stamped with the clock. Overrides, `VTIMEZONE`s and
/// every other line are left as read. A value that cannot be written is an
/// error naming the property.
///
/// # Panics
///
/// When `master` is not one of `calendar`'s components.
pub fn edit(
    calendar: &Component,
    master: &Component,
    change: &Edit<'_>,
) -> Result<Component, Error> {
    let at = calendar
        .components
        .iter()
        .position(|component| std::ptr::eq(component, master))
        .expect("the master is one of the calendar's events");
    let mut edited = calendar.clone();
    let event = &mut edited.components[at];
    for (name, text, restating) in [
        ("SUMMARY", change.summary, &[][..]),
        ("DESCRIPTION", change.description, &["X-ALT-DESC"][..]),
        (
            "LOCATION",
            change.location,
            &["GEO", "X-APPLE-STRUCTURED-LOCATION"][..],
        ),
    ] {
        if let Some(text) = text {
            replace(
                event,
                Property::new(name, Vec::new(), ical::escape_text(text))?,
            );
            remove(event, restating);
        }
    }
    if let Some((start, end)) = change.times {
        replace(event, start.property("DTSTART"));
        replace(event, end.property("DTEND"));
        remove(event, &["DURATION"]);
    }
    let stamp = ical::utc_stamp(change.now_ms);
    replace(event, Property::new("DTSTAMP", Vec::new(), stamp.clone())?);
    replace(event, Property::new("LAST-MODIFIED", Vec::new(), stamp)?);
    Ok(edited)
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::ErrorKind;
    use brenn_provider_kit::testutil::failure;
    use fastmail_provider::testutil::calendar_of;

    use super::*;

    const PATH: &str = "/dav/calendars/user/alice@example.com/work/e1.ics";

    /// `stored` with its master, as `read` selects it, changed by `change`.
    fn edit_master(stored: &Component, change: &Edit<'_>) -> Result<Component, Error> {
        let master = read(PATH, stored).unwrap().master.expect("a master");
        edit(stored, master, change)
    }

    fn lines(component: &Component) -> Vec<String> {
        ical::write(component)
            .split("\r\n")
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_resource_is_its_master_and_a_count_of_its_overrides() {
        let single = calendar_of(&["UID:a\r\n"]);
        let read_single = read(PATH, &single).unwrap();
        assert_eq!(read_single.master, Some(&single.components[0]));
        assert_eq!(read_single.main, &single.components[0]);
        assert_eq!(read_single.overrides, 0);
        assert!(!read_single.recurring());
        for rule in ["RRULE:FREQ=DAILY\r\n", "RDATE:20261103T090000Z\r\n"] {
            let series = calendar_of(&[&format!("UID:a\r\n{rule}")]);
            assert!(read(PATH, &series).unwrap().recurring(), "{rule}");
        }
        let with_override =
            calendar_of(&["UID:a\r\nRECURRENCE-ID:20261103T090000Z\r\n", "UID:a\r\n"]);
        let read_both = read(PATH, &with_override).unwrap();
        assert_eq!(read_both.master, Some(&with_override.components[1]));
        assert_eq!(read_both.main, &with_override.components[1]);
        assert_eq!(read_both.overrides, 1);
        assert!(read_both.recurring());
        let only = calendar_of(&[
            "UID:a\r\nRECURRENCE-ID:20261103T090000Z\r\n",
            "UID:a\r\nRECURRENCE-ID:20261110T090000Z\r\n",
        ]);
        let read_only = read(PATH, &only).unwrap();
        assert_eq!(read_only.master, None);
        assert_eq!(read_only.main, &only.components[0]);
        assert_eq!(read_only.overrides, 2);
        assert!(read_only.recurring());
    }

    #[test]
    fn an_organizer_or_attendee_anywhere_is_refused() {
        let people = failure(ErrorKind::InvalidArgs, INVOLVES_PEOPLE);
        for events in [
            &["ORGANIZER:mailto:alice@example.com\r\n"][..],
            &["ATTENDEE;CN=Bob:mailto:bob@example.com\r\n"][..],
            &[
                "RRULE:FREQ=DAILY\r\n",
                "RECURRENCE-ID:20261103T090000Z\r\nATTENDEE:mailto:bob@example.com\r\n",
            ][..],
            &["RECURRENCE-ID:20261103T090000Z\r\nORGANIZER:mailto:alice@example.com\r\n"][..],
        ] {
            assert_eq!(
                read(PATH, &calendar_of(events)).unwrap_err(),
                people,
                "{events:?}"
            );
        }
    }

    #[test]
    fn the_read_tools_selection_failures_are_kept() {
        assert_eq!(
            read(PATH, &calendar_of(&[])).unwrap_err(),
            failure(ErrorKind::InvalidArgs, &format!("`{PATH}` holds no event"))
        );
        let two = calendar_of(&["UID:a\r\n", "UID:b\r\n"]);
        assert_eq!(read(PATH, &two).unwrap_err().kind, ErrorKind::Internal);
    }

    #[test]
    fn an_edit_replaces_the_named_lines_in_place_and_keeps_the_rest() {
        let stored = calendar_of(&[concat!(
            "UID:a\r\n",
            "DTSTAMP:20200101T000000Z\r\n",
            "SUMMARY;LANGUAGE=en:Old\r\n",
            "x-custom;X-P=\"a:b\":^^kept\r\n",
            "DESCRIPTION:old\r\n",
            "X-ALT-DESC;FMTTYPE=text/html:<p>old</p>\r\n",
            "SUMMARY:Second\r\n",
            "GEO:1.0;2.0\r\n",
            "LOCATION:Room 1\r\n",
            "X-APPLE-STRUCTURED-LOCATION;VALUE=URI:geo:1,2\r\n",
            "DTSTART;TZID=Europe/London:20261102T090000\r\n",
            "DURATION:PT1H\r\n",
            "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY\r\nEND:VALARM\r\n",
        )]);
        let summary_only = Edit {
            summary: Some("New, title; here"),
            now_ms: 0,
            ..Edit::default()
        };
        assert_eq!(
            lines(&edit_master(&stored, &summary_only).unwrap()),
            [
                "BEGIN:VCALENDAR",
                "VERSION:2.0",
                "BEGIN:VEVENT",
                "UID:a",
                "DTSTAMP:19700101T000000Z",
                r"SUMMARY:New\, title\; here",
                "x-custom;X-P=\"a:b\":^^kept",
                "DESCRIPTION:old",
                "X-ALT-DESC;FMTTYPE=text/html:<p>old</p>",
                "GEO:1.0;2.0",
                "LOCATION:Room 1",
                "X-APPLE-STRUCTURED-LOCATION;VALUE=URI:geo:1,2",
                "DTSTART;TZID=Europe/London:20261102T090000",
                "DURATION:PT1H",
                "LAST-MODIFIED:19700101T000000Z",
                "BEGIN:VALARM",
                "TRIGGER:-PT5M",
                "ACTION:DISPLAY",
                "END:VALARM",
                "END:VEVENT",
                "END:VCALENDAR",
                "",
            ]
        );
        let start = When::DateTime {
            local: "20261102T100000".to_owned(),
            utc: false,
            time_zone: Some("America/New_York".to_owned()),
        };
        let end = When::DateTime {
            local: "20261102T110000".to_owned(),
            utc: false,
            time_zone: Some("America/New_York".to_owned()),
        };
        let everything = Edit {
            summary: None,
            description: Some("two\nlines"),
            location: Some("Room 2"),
            times: Some((&start, &end)),
            now_ms: 1_793_610_000_000,
        };
        assert_eq!(
            lines(&edit_master(&stored, &everything).unwrap()),
            [
                "BEGIN:VCALENDAR",
                "VERSION:2.0",
                "BEGIN:VEVENT",
                "UID:a",
                "DTSTAMP:20261102T090000Z",
                "SUMMARY;LANGUAGE=en:Old",
                "x-custom;X-P=\"a:b\":^^kept",
                r"DESCRIPTION:two\nlines",
                "SUMMARY:Second",
                "LOCATION:Room 2",
                "DTSTART;TZID=America/New_York:20261102T100000",
                "DTEND;TZID=America/New_York:20261102T110000",
                "LAST-MODIFIED:20261102T090000Z",
                "BEGIN:VALARM",
                "TRIGGER:-PT5M",
                "ACTION:DISPLAY",
                "END:VALARM",
                "END:VEVENT",
                "END:VCALENDAR",
                "",
            ]
        );
    }

    #[test]
    fn an_edit_reaches_the_master_alone() {
        let stored = calendar_of(&[
            "UID:a\r\nRECURRENCE-ID:20261103T090000Z\r\nSUMMARY:Moved\r\n",
            "UID:a\r\nSUMMARY:Weekly\r\nRRULE:FREQ=WEEKLY\r\n",
        ]);
        let change = Edit {
            summary: Some("Renamed"),
            ..Edit::default()
        };
        let edited = edit_master(&stored, &change).unwrap();
        assert_eq!(edited.components[0], stored.components[0]);
        assert_eq!(
            edited.components[1].property("SUMMARY").unwrap().value,
            "Renamed"
        );
    }

    #[test]
    #[should_panic(expected = "the master is one of the calendar's events")]
    fn an_edit_of_an_event_from_another_calendar_panics() {
        let stored = calendar_of(&["UID:a\r\n"]);
        let other = calendar_of(&["UID:a\r\n"]);
        let _ = edit(&stored, &other.components[0], &Edit::default());
    }

    #[test]
    fn a_dtend_is_replaced_and_a_missing_one_appended() {
        let date = |d: &str| When::Date(d.to_owned());
        let (start, end) = (date("20261102"), date("20261103"));
        let change = Edit {
            times: Some((&start, &end)),
            ..Edit::default()
        };
        let stored = calendar_of(&["DTEND:20261102T100000Z\r\nDTSTART:20261102T090000Z\r\n"]);
        let event = &edit_master(&stored, &change).unwrap().components[0];
        let names: Vec<&str> = event.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["DTEND", "DTSTART", "DTSTAMP", "LAST-MODIFIED"]);
        assert_eq!(When::of(event.property("DTEND").unwrap()).unwrap(), end);
        assert_eq!(When::of(event.property("DTSTART").unwrap()).unwrap(), start);
    }
}
