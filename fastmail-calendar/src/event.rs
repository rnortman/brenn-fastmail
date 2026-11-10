//! One event in full, with its changed occurrences in brief. Values are
//! passed through as written: no time is converted and no recurrence
//! expanded. Every string is cut to a serialized width and every list capped,
//! so the answer stays under the result ceiling.

use brenn_provider_kit::{Args, Failure};
use fastmail_dav::ical::{Component, Property};
use fastmail_provider::event::{BRIEF_TEXT_MAX, CODE_MAX, occurrence, put_text, selected};
use fastmail_provider::results::{capped_list, cut_value};
use serde_json::{Value, json};

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
/// The widest recurrence line.
pub const RULE_MAX: usize = 500;
/// The most attendees, recurrence lines and overrides shown.
pub const ATTENDEES_MAX: usize = 50;
pub const RULES_MAX: usize = 20;
pub const OVERRIDES_MAX: usize = 50;

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

/// The calendar data read from the resource at `path` as one event in
/// full. The main event is the one with no `RECURRENCE-ID`, or, when every
/// one has it, the first by that value; the others are its overrides, by
/// `RECURRENCE-ID`. Data with no event is the caller's failure; data with
/// two events without a `RECURRENCE-ID`, or two with the same one, is
/// malformed.
pub fn full(path: &str, calendar: &Component) -> Result<Value, Failure> {
    let all = selected(path, calendar)?;
    let (main, overrides) = all.split_first().expect("a selection is never empty");
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
