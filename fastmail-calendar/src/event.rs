//! A `VEVENT` as the tools show it: one occurrence in a window, or one event
//! in full with its changed occurrences in brief. Values are passed through
//! as written: no time is converted and no recurrence expanded. Every string
//! is cut to a serialized width and every list capped, so each answer stays
//! under the result ceiling.

use std::fmt::Display;

use brenn_provider_kit::{Args, Failure, internal};
use fastmail_dav::ical::{Component, Property, When};
use fastmail_provider::dav::QUOTED_PATH_MAX;
use fastmail_provider::failure::invalid;
use fastmail_provider::results::{capped_list, cut_value, cut_width};
use fastmail_provider::settings::API_NAME;
use serde_json::{Value, json};

/// The widest `summary` and `location` of an occurrence in a window or an
/// override.
pub const BRIEF_TEXT_MAX: usize = 300;
/// The widest `summary` and `location` of an event read in full.
pub const FULL_TEXT_MAX: usize = 1000;
/// The widest `description`.
pub const DESCRIPTION_MAX: usize = 8000;
/// The widest `uid`.
pub const UID_MAX: usize = 255;
/// The widest person's `name`.
pub const NAME_MAX: usize = 400;
/// The widest person's `email`.
pub const EMAIL_MAX: usize = 320;
/// The widest code-like value: `time_zone`, `status`, `duration`,
/// `response`, `role`.
pub const CODE_MAX: usize = 64;
/// The widest recurrence line.
pub const RULE_MAX: usize = 500;
/// The most attendees, recurrence lines and overrides shown.
pub const ATTENDEES_MAX: usize = 50;
pub const RULES_MAX: usize = 20;
pub const OVERRIDES_MAX: usize = 50;

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

/// `event`'s TEXT property `name` at `key`, unescaped and cut at `max`;
/// whether it was cut.
fn put_text(out: &mut Args, key: &str, event: &Component, name: &str, max: usize) -> bool {
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

/// A time property as `{ date }` or `{ date_time, time_zone? }`.
fn shape(property: &Property, path: &str) -> Result<Value, Failure> {
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

/// An `ORGANIZER` or `ATTENDEE`: its address without a leading `mailto:`,
/// and its `CN`.
fn person(property: &Property) -> Args {
    let mut out = Args::new();
    let value = property.value.as_str();
    let address = match value.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => &value[7..],
        _ => value,
    };
    if !address.is_empty() {
        out.insert("email".into(), cut_value(address, EMAIL_MAX));
    }
    if let Some(name) = property.param("CN") {
        out.insert("name".into(), cut_value(name, NAME_MAX));
    }
    out
}

/// A recurrence property as its content line, `RRULE:FREQ=WEEKLY`, with its
/// parameters; a parameter value holding `:`, `;` or `,` is quoted.
fn content_line(property: &Property) -> String {
    let mut line = property.name.clone();
    for (name, values) in &property.params {
        let values: Vec<String> = values
            .iter()
            .map(|value| {
                if value.contains([':', ';', ',']) {
                    format!("\"{value}\"")
                } else {
                    value.clone()
                }
            })
            .collect();
        line.push_str(&format!(";{name}={}", values.join(",")));
    }
    line.push(':');
    line.push_str(&property.value);
    line
}

/// `event`'s `RECURRENCE-ID` as written.
fn recurrence_id(event: &Component) -> Option<&str> {
    event.property("RECURRENCE-ID").map(|id| id.value.as_str())
}

/// The calendar data read from the resource at `path` as one event in
/// full. The main event is the one with no `RECURRENCE-ID`, or, when every
/// one has it, the first by that value; the others are its overrides, by
/// `RECURRENCE-ID`. Data with no event is the caller's failure; data with
/// two events without a `RECURRENCE-ID`, or two with the same one, is
/// malformed.
pub fn full(path: &str, calendar: &Component) -> Result<Value, Failure> {
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
    let Some((main, overrides)) = all.split_first() else {
        return Err(invalid(format!("`{path}` holds no event")));
    };
    let mut out = Args::new();
    out.insert("event".into(), json!(path));
    put_text(&mut out, "uid", main, "UID", UID_MAX);
    out.extend(occurrence(main, path, FULL_TEXT_MAX)?);
    if put_text(
        &mut out,
        "description",
        main,
        "DESCRIPTION",
        DESCRIPTION_MAX,
    ) {
        out.insert("description_truncated".into(), json!(true));
    }
    if let Some(organizer) = main.property("ORGANIZER") {
        out.insert("organizer".into(), Value::Object(person(organizer)));
    }
    let attendees: Vec<&Property> = main
        .properties
        .iter()
        .filter(|p| p.name == "ATTENDEE")
        .collect();
    capped_list(
        &mut out,
        "attendees",
        attendees,
        ATTENDEES_MAX,
        |attendee| {
            let mut out = person(attendee);
            if let Some(response) = attendee.param("PARTSTAT") {
                out.insert("response".into(), cut_value(response, CODE_MAX));
            }
            if let Some(role) = attendee.param("ROLE") {
                out.insert("role".into(), cut_value(role, CODE_MAX));
            }
            Value::Object(out)
        },
    );
    let rules: Vec<&Property> = main
        .properties
        .iter()
        .filter(|p| matches!(p.name.as_str(), "RRULE" | "RDATE" | "EXDATE"))
        .collect();
    capped_list(&mut out, "recurrence", rules, RULES_MAX, |rule| {
        cut_value(&content_line(rule), RULE_MAX)
    });
    // Every override is read, shown or not: a malformed one fails the call.
    let overrides = overrides
        .iter()
        .map(|event| occurrence(event, path, BRIEF_TEXT_MAX))
        .collect::<Result<Vec<_>, _>>()?;
    capped_list(
        &mut out,
        "overrides",
        overrides,
        OVERRIDES_MAX,
        Value::Object,
    );
    Ok(Value::Object(out))
}
