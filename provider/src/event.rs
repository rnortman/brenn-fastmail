//! A `VEVENT` as the calendar tools show and select it: the events of a
//! resource, the main one and its overrides, and one event's times and text
//! in a window's shape. Values are passed through as written: no time is
//! converted and no recurrence expanded. Every string is cut to a serialized
//! width, so each answer stays under the result ceiling.

use std::fmt::Display;

use brenn_provider_kit::{Args, Failure, internal};
use fastmail_dav::ical::{Component, Property, When};
use serde_json::{Value, json};

use crate::dav::QUOTED_PATH_MAX;
use crate::failure::invalid;
use crate::results::{cut_value, cut_width};
use crate::settings::API_NAME;

/// The widest `summary` and `location` of an occurrence in a window or an
/// override.
pub const BRIEF_TEXT_MAX: usize = 300;
/// The widest code-like value: `time_zone`, `status`, `duration`,
/// `response`, `role`.
pub const CODE_MAX: usize = 64;

/// The component name of a calendar object.
pub const VCALENDAR: &str = "VCALENDAR";
/// The component name of an event.
pub const VEVENT: &str = "VEVENT";

/// The request's failure for calendar data at `path` that cannot be shown
/// as it is: it names the resource and the problem, never a value.
pub fn malformed(path: &str, problem: impl Display) -> Failure {
    internal(format!(
        "{API_NAME}'s calendar data at `{}`: {problem}",
        cut_width(path, QUOTED_PATH_MAX).0
    ))
}

/// The events `calendar` holds, in the order written.
pub fn events(calendar: &Component) -> impl Iterator<Item = &Component> {
    calendar.components.iter().filter(|c| c.name == VEVENT)
}

/// `event`'s `RECURRENCE-ID` as written.
pub fn recurrence_id(event: &Component) -> Option<&str> {
    event.property("RECURRENCE-ID").map(|id| id.value.as_str())
}

/// The events of the resource at `path`, the one with no `RECURRENCE-ID`
/// first when there is one, then the others by `RECURRENCE-ID` as written.
/// Never empty: data with no event is the caller's failure; data with two
/// events without a `RECURRENCE-ID`, or two with the same one, is malformed.
pub fn selected<'a>(path: &str, calendar: &'a Component) -> Result<Vec<&'a Component>, Failure> {
    let mut all: Vec<&Component> = events(calendar).collect();
    all.sort_by(|a, b| recurrence_id(a).cmp(&recurrence_id(b)));
    if let Some(pair) = all
        .windows(2)
        .find(|pair| recurrence_id(pair[0]) == recurrence_id(pair[1]))
    {
        return Err(malformed(
            path,
            match recurrence_id(pair[0]) {
                None => "more than one event has no RECURRENCE-ID",
                Some(_) => "two events have the same RECURRENCE-ID",
            },
        ));
    }
    if all.is_empty() {
        return Err(invalid(format!("`{path}` holds no event")));
    }
    Ok(all)
}

/// `event`'s TEXT property `name` at `key`, unescaped and cut at `max`;
/// whether it was cut.
pub fn put_text(out: &mut Args, key: &str, event: &Component, name: &str, max: usize) -> bool {
    let Some(property) = event.property(name) else {
        return false;
    };
    let (text, was_cut) = cut_width(&property.text(), max);
    out.insert(key.to_owned(), json!(text));
    was_cut
}

/// `event`'s value `name` at `key` as written, cut at `max`.
fn put_raw(out: &mut Args, key: &str, event: &Component, name: &str, max: usize) {
    if let Some(property) = event.property(name) {
        out.insert(key.to_owned(), cut_value(&property.value, max));
    }
}

/// `event`'s `DTSTART`, `DTEND` or `RECURRENCE-ID` as `{ date }` or
/// `{ date_time, time_zone? }`; `None` when the event has none.
fn when(event: &Component, name: &str, path: &str) -> Result<Option<Value>, Failure> {
    event
        .property(name)
        .map(|property| shape(property, path))
        .transpose()
}

/// `event`'s `DTSTART`, from the resource at `path`; an event with none is
/// the request's failure.
pub fn start<'a>(event: &'a Component, path: &str) -> Result<&'a Property, Failure> {
    event
        .property("DTSTART")
        .ok_or_else(|| malformed(path, "an event has no DTSTART"))
}

/// A time property, from the resource at `path`, as `{ date }` or
/// `{ date_time, time_zone? }`; a time of neither shape is the request's
/// failure.
pub fn shape(property: &Property, path: &str) -> Result<Value, Failure> {
    let when = When::of(property).map_err(|error| malformed(path, error))?;
    Ok(match &when {
        When::Date(_) => json!({ "date": when.to_string() }),
        When::DateTime { time_zone, .. } => {
            let mut out = Args::new();
            out.insert("date_time".into(), json!(when.to_string()));
            if let Some(zone) = time_zone {
                out.insert("time_zone".into(), cut_value(zone, CODE_MAX));
            }
            Value::Object(out)
        }
    })
}

/// `event`, from the resource at `path`, in the window's shape: `summary`
/// and `location` cut at `text_max`, `start`, `end` or else `duration`,
/// `status` and `recurrence_id`, each only when the event has it. An event
/// with no `DTSTART`, or a time of neither shape, is the request's failure.
pub fn occurrence(event: &Component, path: &str, text_max: usize) -> Result<Args, Failure> {
    let mut out = Args::new();
    put_text(&mut out, "summary", event, "SUMMARY", text_max);
    put_text(&mut out, "location", event, "LOCATION", text_max);
    out.insert("start".into(), shape(start(event, path)?, path)?);
    match when(event, "DTEND", path)? {
        Some(end) => {
            out.insert("end".into(), end);
        }
        None => put_raw(&mut out, "duration", event, "DURATION", CODE_MAX),
    }
    put_raw(&mut out, "status", event, "STATUS", CODE_MAX);
    if let Some(recurrence_id) = when(event, "RECURRENCE-ID", path)? {
        out.insert("recurrence_id".into(), recurrence_id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::ErrorKind;
    use brenn_provider_kit::testutil::failure;

    use super::*;
    use crate::testutil::calendar_of;

    const PATH: &str = "/dav/calendars/user/alice@example.com/work/e1.ics";

    fn summaries(selected: &[&Component]) -> Vec<String> {
        selected
            .iter()
            .map(|event| event.property("SUMMARY").unwrap().value.clone())
            .collect()
    }

    #[test]
    fn the_master_comes_first_then_the_overrides_by_recurrence_id() {
        let data = calendar_of(&[
            "SUMMARY:b\r\nRECURRENCE-ID:20261110T090000Z\r\n",
            "SUMMARY:m\r\n",
            "SUMMARY:a\r\nRECURRENCE-ID:20261103T090000Z\r\n",
        ]);
        let all = selected(PATH, &data).unwrap();
        assert_eq!(summaries(&all), ["m", "a", "b"]);
        assert_eq!(recurrence_id(all[0]), None);
    }

    #[test]
    fn overrides_alone_are_taken_lowest_first() {
        let data = calendar_of(&[
            "SUMMARY:b\r\nRECURRENCE-ID:20261110T090000Z\r\n",
            "SUMMARY:a\r\nRECURRENCE-ID:20261103T090000Z\r\n",
        ]);
        let all = selected(PATH, &data).unwrap();
        assert_eq!(summaries(&all), ["a", "b"]);
        assert_eq!(recurrence_id(all[0]), Some("20261103T090000Z"));
    }

    #[test]
    fn two_masters_two_alike_overrides_and_no_event_are_refused() {
        let two_masters = calendar_of(&["SUMMARY:a\r\n", "SUMMARY:b\r\n"]);
        assert_eq!(
            selected(PATH, &two_masters).unwrap_err(),
            failure(
                ErrorKind::Internal,
                &format!(
                    "Fastmail's calendar data at `{PATH}`: more than one event has no \
                     RECURRENCE-ID"
                )
            )
        );
        let alike = calendar_of(&[
            "SUMMARY:a\r\nRECURRENCE-ID:20261103T090000Z\r\n",
            "SUMMARY:b\r\nRECURRENCE-ID:20261103T090000Z\r\n",
        ]);
        assert_eq!(
            selected(PATH, &alike).unwrap_err(),
            failure(
                ErrorKind::Internal,
                &format!(
                    "Fastmail's calendar data at `{PATH}`: two events have the same \
                     RECURRENCE-ID"
                )
            )
        );
        assert_eq!(
            selected(PATH, &calendar_of(&[])).unwrap_err(),
            failure(ErrorKind::InvalidArgs, &format!("`{PATH}` holds no event"))
        );
    }

    #[test]
    fn a_time_is_shaped_as_a_date_or_a_date_time_with_its_zone() {
        let data = calendar_of(&[concat!(
            "DTSTART;VALUE=DATE:20261102\r\n",
            "DTEND:20261102T090000Z\r\n",
            "RECURRENCE-ID;TZID=Europe/Berlin:20261102T100000\r\n",
        )]);
        let event = &data.components[0];
        let shaped = |name: &str| shape(event.property(name).unwrap(), PATH).unwrap();
        assert_eq!(shaped("DTSTART"), json!({ "date": "2026-11-02" }));
        assert_eq!(
            shaped("DTEND"),
            json!({ "date_time": "2026-11-02T09:00:00Z" })
        );
        assert_eq!(
            shaped("RECURRENCE-ID"),
            json!({ "date_time": "2026-11-02T10:00:00", "time_zone": "Europe/Berlin" })
        );
        let bad = calendar_of(&["DTSTART:tomorrow\r\n"]);
        assert_eq!(
            shape(bad.components[0].property("DTSTART").unwrap(), PATH)
                .unwrap_err()
                .kind,
            ErrorKind::Internal
        );
    }
}
