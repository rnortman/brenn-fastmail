//! The calendar write provider's decisions, in `std`, `serde_json`, the DAV
//! crate, the shared provider crate and `brenn-provider-kit`: validating a
//! tool's arguments, building each CalDAV request as `api-request` arguments,
//! and reading Fastmail's answer into the tool's result. Nothing here names
//! the guest SDK, so the whole module runs on the host under test. Tools are
//! the generated `Provided` enum, whose SDK-facing parts are wasm32-only.
//!
//! A create is one `PUT` of a new event under `If-None-Match: *`, so it never
//! replaces a stored one. An update is a `GET` of the event and a `PUT` of it
//! changed, under `If-Match` with the entity tag the `GET` answered, so it
//! never overwrites a change made since the read. A delete is the same `GET`
//! and a `DELETE` under `If-Match`. A failure that leaves the
//! write's outcome unknown carries the tool's hint saying what may already
//! have happened; Fastmail's refusal does not.

// The host test's crate root is this file, so it declares the generated
// module and its sibling itself; in the component, `lib.rs` does.
#[cfg(test)]
mod resource;
#[cfg(test)]
mod spec;

use brenn_provider_kit::{
    Args, COMPACT_RESULT_CEILING, Call, Failure, Logic, NestedError, Next, Tool, bounded, capped,
    flag, internal, no_such_step, object,
};
use fastmail_dav::href;
use fastmail_dav::ical::{self, Component, Property, When};
use fastmail_provider::args::{
    ACCOUNT_ARG, Lines, account_arg, calendar_path_arg, distinct_list, fits_shape, held_text,
    json_width, one_line,
};
use fastmail_provider::dav::{
    CALENDAR_ACCEPT, CALENDAR_CONTENT_TYPE, DavRequest, Expect, Precondition, dav_call, etag,
    step_payload,
};
use fastmail_provider::event::{
    BRIEF_TEXT_MAX, VCALENDAR, VEVENT, malformed, put_text, shape, start,
};
use fastmail_provider::failure::{StepFailure, derived_u64, invalid, missing_derived, stored_args};
use fastmail_provider::settings::Settings;
use serde_json::{Value, json};

use crate::resource::Edit;
use crate::spec::Provided;

/// The most characters a `summary` or a `location` may hold.
const TEXT_MAX_CHARS: usize = 1000;
/// The most characters a `description` may hold.
const DESCRIPTION_MAX_CHARS: usize = 8000;
/// The most characters a `time_zone` may hold.
const TIME_ZONE_MAX_CHARS: usize = 64;
/// The most `recurrence` lines, and the most characters of a rule after
/// `RRULE:`.
const RECURRENCE_MAX: usize = 3;
const RULE_MAX_CHARS: usize = 500;

/// A create's one step, the write.
const CREATE_STEP: u8 = 1;
/// An update's and a delete's steps: the read, then the write.
const READ_STEP: u8 = 1;
const WRITE_STEP: u8 = 2;

/// The statuses a `PUT` answers when it stored the event: 201 for a resource
/// it created, 200 or 204 for one it replaced (RFC 7231 section 4.3.4).
const STORED: &[u64] = &[201, 204, 200];
/// The statuses a `DELETE` answers when it removed the event.
const DELETED: &[u64] = &[204, 200];
/// The status a `GET` of an event answers.
const OK: u64 = 200;

// TODO(calendar-write-bounded-by-call-argument-cap): an update writes the
// event back whole inside one nested call, whose arguments Brenn caps at
// 64 KiB, so a larger event is refused.
/// The widest event body an update writes back, as the call carries it:
/// escaped as JSON, each line end counting four bytes and each `"` and `\`
/// two. The 64 KiB argument cap less room for the call's other fields, a
/// widest path and entity tag among them.
const BODY_MAX_WIRE_BYTES: usize = 60_000;

/// The `derived` keys an update's write carries: the fields it changed and
/// how many overrides it left.
const UPDATED_KEY: &str = "updated";
const OVERRIDES_KEY: &str = "overrides";

/// The `derived` keys a delete's write carries: what the answer says of the
/// event it removed.
const SUMMARY_KEY: &str = "summary";
const START_KEY: &str = "start";
const SERIES_KEY: &str = "series";

/// The product that wrote a new event (RFC 5545 section 3.7.3).
const PRODID: &str = "-//brenn-fastmail//fastmail-calendar-write//EN";
/// What a new event's name begins with, and what it ends with.
const NAME_PREFIX: &str = "brenn-";
const NAME_SUFFIX: &str = ".ics";

/// A create whose name is taken: the server stored nothing.
const NAME_TAKEN: &str =
    "an event already has the name this tool chose, so nothing was created; ask again";
/// What a lost answer to a create leaves the caller to know.
const CREATE_UNKNOWN_HINT: &str = "the event may have been created before its answer was lost: \
     look for it with FastmailCalendarEvents in its window before asking again, or the user has \
     two";

/// A write whose entity tag no longer matches: the server changed nothing.
const CHANGED_SINCE_READ: &str = "the event changed since it was read, so nothing was changed; \
     read it again with FastmailCalendarEvent and ask again";
/// What a lost answer to an update's write leaves the caller to know.
const UPDATE_UNKNOWN_HINT: &str = "the change may already have been applied: read the event with \
     FastmailCalendarEvent before asking again; asking again with the same arguments is safe";
/// What a lost answer to a delete leaves the caller to know.
const DELETE_UNKNOWN_HINT: &str = "the event may already be deleted: asking again is safe, since a \
     deleted event is refused as no event";
/// A recurring event's delete without `series: true`.
const SERIES_NEEDED: &str = "this is a recurring event: pass `series: true` to delete every \
     occurrence. One occurrence cannot be deleted here (delete it in Fastmail)";
/// A read that answered no entity tag a write could be conditioned on.
const NO_ETAG: &str = "Fastmail answered the event without an entity tag it can be matched by, \
     so it cannot be changed safely";

impl Tool for Provided {
    fn name(self) -> &'static str {
        Provided::name(self)
    }

    fn from_name(name: &str) -> Option<Provided> {
        Provided::from_name(name)
    }
}

/// A request's in-flight row, naming one of this component's tools.
pub type InFlight = brenn_provider_kit::InFlight<Provided>;

/// What every step reads beside its arguments: the instance's settings and
/// the activation's clock, which names a new event and stamps a written one.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub settings: Settings,
    /// The activation's UTC clock, in milliseconds since the epoch.
    pub now_ms: u64,
}

/// This component's step functions, for the kit's activation loop.
pub struct Fastmail;

impl Logic for Fastmail {
    type Tool = Provided;
    type Settings = Context;

    fn plan(tool: Provided, args: &Value, context: &Context) -> Result<Call, String> {
        plan(tool, args, context)
    }

    fn advance(state: &InFlight, outcome: Result<Value, NestedError>, context: &Context) -> Next {
        advance(state, outcome, context)
    }
}

/// A request's arguments, validated.
#[derive(Clone, Debug, PartialEq)]
struct Parsed {
    /// Which of the user's accounts on the key provider every nested call
    /// names, by the operator's name for it; `None` for the user's default or
    /// only one.
    account: Option<String>,
    tool: ToolArgs,
}

/// A tool's own arguments, validated.
#[derive(Clone, Debug, PartialEq)]
enum ToolArgs {
    Create(CreateArgs),
    Update(UpdateArgs),
    Delete(DeleteArgs),
}

/// The kind a `start` or `end` argument is written as.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// `2026-11-02`, a whole day.
    Date,
    /// `2026-11-02T09:00:00Z`.
    Instant,
    /// `2026-11-02T09:00:00`, in `time_zone`.
    Local,
}

/// An event's `start` and `end`, validated: of one kind, `end` later, and in
/// a zone exactly when they are local times.
#[derive(Clone, Debug, PartialEq)]
struct Times {
    start: When,
    end: When,
    kind: Kind,
}

/// `text` as a `start` or `end` argument: its kind and its value as
/// iCalendar writes it, `20261102` or `20261102T090000`. Field ranges are the
/// server's to check.
fn time_value(text: &str) -> Option<(Kind, String)> {
    let kind = if fits_shape(text, b"0000-00-00") {
        Kind::Date
    } else if fits_shape(text, b"0000-00-00T00:00:00Z") {
        Kind::Instant
    } else if fits_shape(text, b"0000-00-00T00:00:00") {
        Kind::Local
    } else {
        return None;
    };
    let compact = text
        .chars()
        .filter(|c| !matches!(c, '-' | ':' | 'Z'))
        .collect();
    Some((kind, compact))
}

/// The `start` or `end` argument at `key`, as the caller wrote it, with its
/// kind and value.
fn time_arg(map: &Args, key: &str) -> Result<(String, Kind, String), String> {
    let refusal = || {
        format!(
            "`{key}` must be a date (2026-11-02), a UTC instant (2026-11-02T09:00:00Z) or a \
             local time (2026-11-02T09:00:00)"
        )
    };
    match map.get(key) {
        None => Err(format!("`{key}` is required")),
        Some(Value::String(text)) => {
            let (kind, value) = time_value(text).ok_or_else(refusal)?;
            Ok((text.clone(), kind, value))
        }
        Some(_) => Err(refusal()),
    }
}

/// Whether `zone` is an IANA zone name as this component admits one: 1 to
/// `TIME_ZONE_MAX_CHARS` characters, segments of letters, digits, `_`, `+`
/// and `-` joined by `/`.
fn is_zone_name(zone: &str) -> bool {
    zone.len() <= TIME_ZONE_MAX_CHARS
        && zone.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
        })
}

/// The `time_zone` argument; absent is `None`. Not looked up: the component
/// holds no zone data.
fn zone_arg(map: &Args) -> Result<Option<String>, String> {
    match map.get("time_zone") {
        None => Ok(None),
        Some(Value::String(zone)) if is_zone_name(zone) => Ok(Some(zone.clone())),
        Some(_) => Err(format!(
            "`time_zone` must be an IANA time zone name (Europe/London): at most \
             {TIME_ZONE_MAX_CHARS} characters, letters, digits, `_`, `+` and `-` in segments \
             joined by `/`"
        )),
    }
}

/// `start`, `end` and `time_zone`, held to the rules they share.
fn times_args(map: &Args) -> Result<Times, String> {
    let (start_text, kind, start) = time_arg(map, "start")?;
    let (end_text, end_kind, end) = time_arg(map, "end")?;
    let zone = zone_arg(map)?;
    if end_kind != kind {
        return Err(
            "`start` and `end` must be of one kind: both dates, both UTC instants or both local \
             times"
                .to_owned(),
        );
    }
    match (kind, &zone) {
        (Kind::Local, None) => {
            return Err(
                "`start` and `end` are local times, which need `time_zone`: give it, or write \
                 them as UTC instants ending in Z"
                    .to_owned(),
            );
        }
        (Kind::Date | Kind::Instant, Some(_)) => {
            return Err(
                "`time_zone` is only for local times (2026-11-02T09:00:00): dates and UTC \
                 instants take none"
                    .to_owned(),
            );
        }
        _ => {}
    }
    // One kind, one shape: the strings order as the times do.
    if end_text <= start_text {
        return Err("`end` must be later than `start`".to_owned());
    }
    let when = |value: String| match kind {
        Kind::Date => When::Date(value),
        Kind::Instant | Kind::Local => When::DateTime {
            local: value,
            utc: kind == Kind::Instant,
            time_zone: zone.clone(),
        },
    };
    Ok(Times {
        start: when(start),
        end: when(end),
        kind,
    })
}

/// A one-line text argument at `key` of at most `TEXT_MAX_CHARS`.
fn line_arg(map: &Args, key: &str) -> Result<Option<String>, String> {
    one_line(map, key, TEXT_MAX_CHARS)
}

/// The `description` argument: at most `DESCRIPTION_MAX_CHARS`, line breaks
/// admitted, no other control character.
fn description_arg(map: &Args) -> Result<Option<String>, String> {
    let Some(text) = capped(map, "description", DESCRIPTION_MAX_CHARS)? else {
        return Ok(None);
    };
    held_text("`description`", text, DESCRIPTION_MAX_CHARS, Lines::Many)?;
    Ok(Some(text.to_owned()))
}

/// Whether `line` is an `RRULE:` line as `recurrence` admits one: 1 to
/// `RULE_MAX_CHARS` letters, digits and `=;,+-` after the prefix, which keeps
/// a line break or another property out of the event.
fn is_rule_line(line: &str) -> bool {
    line.strip_prefix("RRULE:").is_some_and(|rule| {
        (1..=RULE_MAX_CHARS).contains(&rule.len())
            && rule
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"=;,+-".contains(&b))
    })
}

/// The `recurrence` argument: 1 to `RECURRENCE_MAX` distinct `RRULE:` lines.
fn recurrence_arg(map: &Args) -> Result<Option<Vec<String>>, String> {
    distinct_list(
        map,
        "recurrence",
        RECURRENCE_MAX,
        "`RRULE:` lines",
        |line| match line {
            Value::String(line) if is_rule_line(line) => Ok(line.clone()),
            _ => Err(format!(
                "each of `recurrence` must be an `RRULE:` line: `RRULE:` then 1 to \
                 {RULE_MAX_CHARS} letters, digits and `=;,+-`"
            )),
        },
    )
}

/// The create tool's arguments, validated.
#[derive(Clone, Debug, PartialEq)]
struct CreateArgs {
    calendar: String,
    summary: String,
    times: Times,
    description: Option<String>,
    location: Option<String>,
    recurrence: Vec<String>,
}

impl CreateArgs {
    const KEYS: &[&str] = &[
        "calendar",
        "summary",
        "start",
        "end",
        "time_zone",
        "description",
        "location",
        "recurrence",
        ACCOUNT_ARG,
    ];

    fn parse(map: &Args) -> Result<CreateArgs, String> {
        let calendar = calendar_path_arg(map, "calendar", true)?;
        let summary = line_arg(map, "summary")?.ok_or("`summary` is required")?;
        let times = times_args(map)?;
        let recurrence = recurrence_arg(map)?.unwrap_or_default();
        if !recurrence.is_empty() && times.kind == Kind::Instant {
            return Err(
                "`recurrence` on an event with a time needs `time_zone`, with `start` and `end` \
                 as local times (2026-11-02T09:00:00), so the series keeps its time of day \
                 across daylight-saving changes"
                    .to_owned(),
            );
        }
        Ok(CreateArgs {
            calendar,
            summary,
            times,
            description: description_arg(map)?,
            location: line_arg(map, "location")?,
            recurrence,
        })
    }
}

/// The update tool's arguments, validated: at least one change.
#[derive(Clone, Debug, PartialEq)]
struct UpdateArgs {
    event: String,
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    times: Option<Times>,
}

impl UpdateArgs {
    const KEYS: &[&str] = &[
        "event",
        "summary",
        "description",
        "location",
        "start",
        "end",
        "time_zone",
        ACCOUNT_ARG,
    ];

    fn parse(map: &Args) -> Result<UpdateArgs, String> {
        let event = calendar_path_arg(map, "event", false)?;
        let times = match (map.contains_key("start"), map.contains_key("end")) {
            (true, true) => Some(times_args(map)?),
            (false, false) if map.contains_key("time_zone") => {
                return Err("`time_zone` is given only with `start` and `end`".to_owned());
            }
            (false, false) => None,
            _ => return Err("`start` and `end` are given together".to_owned()),
        };
        let args = UpdateArgs {
            event,
            summary: line_arg(map, "summary")?,
            description: description_arg(map)?,
            location: line_arg(map, "location")?,
            times,
        };
        if args.changed().is_empty() {
            return Err(
                "give at least one change: `summary`, `description`, `location`, or `start` and \
                 `end`"
                    .to_owned(),
            );
        }
        Ok(args)
    }

    /// The fields this update changes, in the order the tool takes them.
    fn changed(&self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        for (key, given) in [
            ("summary", self.summary.is_some()),
            ("description", self.description.is_some()),
            ("location", self.location.is_some()),
        ] {
            if given {
                changed.push(key);
            }
        }
        if self.times.is_some() {
            changed.extend(["start", "end"]);
        }
        changed
    }
}

/// The delete tool's arguments, validated.
#[derive(Clone, Debug, PartialEq)]
struct DeleteArgs {
    event: String,
    /// Whether a recurring event may be deleted, every occurrence with it.
    series: bool,
}

impl DeleteArgs {
    const KEYS: &[&str] = &["event", "series", ACCOUNT_ARG];

    fn parse(map: &Args) -> Result<DeleteArgs, String> {
        Ok(DeleteArgs {
            event: calendar_path_arg(map, "event", false)?,
            series: flag(map, "series")?,
        })
    }
}

fn parse(tool: Provided, args: &Value) -> Result<Parsed, String> {
    let keys = match tool {
        Provided::FastmailCalendarEventCreate => CreateArgs::KEYS,
        Provided::FastmailCalendarEventUpdate => UpdateArgs::KEYS,
        Provided::FastmailCalendarEventDelete => DeleteArgs::KEYS,
    };
    let map = object(args, keys)?;
    let tool = match tool {
        Provided::FastmailCalendarEventCreate => ToolArgs::Create(CreateArgs::parse(map)?),
        Provided::FastmailCalendarEventUpdate => ToolArgs::Update(UpdateArgs::parse(map)?),
        Provided::FastmailCalendarEventDelete => ToolArgs::Delete(DeleteArgs::parse(map)?),
    };
    Ok(Parsed {
        account: account_arg(map)?,
        tool,
    })
}

/// A new event's name without `.ics`, which is also its `UID`:
/// `brenn-<now_ms>-<hash>`, the hash a 64-bit FNV-1a of the arguments as
/// serialized JSON, in 16 hex digits. Two requests alike in both are one
/// name, which `If-None-Match: *` refuses the second time.
fn event_uid(args: &Value, now_ms: u64) -> String {
    let text = serde_json::to_string(args).expect("a JSON value serialises");
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{NAME_PREFIX}{now_ms}-{hash:016x}")
}

/// A property this component writes; a value it cannot write is the
/// request's refusal, so nothing is sent.
fn property(name: &str, value: String) -> Result<Property, String> {
    Property::new(name, Vec::new(), value)
        .map_err(|error| format!("the event cannot be written: {error}"))
}

// TODO(calendar-write-unconfirmed-against-live-fastmail): that Fastmail takes
// a `TZID` with no `VTIMEZONE`, honours the preconditions and stores the
// event at the path it was put to is assumed, not observed.
/// The new event as iCalendar: one `VEVENT` with `UID`, `DTSTAMP`, the
/// times, the texts given and each `RRULE`, and no organizer, attendee,
/// `VTIMEZONE` or alarm.
fn new_event(args: &CreateArgs, uid: &str, now_ms: u64) -> Result<String, String> {
    let mut event = vec![
        property("UID", uid.to_owned())?,
        property("DTSTAMP", ical::utc_stamp(now_ms))?,
        args.times.start.property("DTSTART"),
        args.times.end.property("DTEND"),
        property("SUMMARY", ical::escape_text(&args.summary))?,
    ];
    for (name, text) in [
        ("DESCRIPTION", &args.description),
        ("LOCATION", &args.location),
    ] {
        if let Some(text) = text {
            event.push(property(name, ical::escape_text(text))?);
        }
    }
    for line in &args.recurrence {
        let rule = line.strip_prefix("RRULE:").expect("a held rule line");
        event.push(property("RRULE", rule.to_owned())?);
    }
    let calendar = Component {
        name: VCALENDAR.to_owned(),
        properties: vec![
            property("VERSION", "2.0".to_owned())?,
            property("PRODID", PRODID.to_owned())?,
        ],
        components: vec![Component {
            name: VEVENT.to_owned(),
            properties: event,
            components: Vec::new(),
        }],
    };
    Ok(ical::write(&calendar))
}

/// Validate a request's arguments and issue its first call. An error is the
/// request's `invalid_args` detail, naming the argument.
pub fn plan(tool: Provided, args: &Value, context: &Context) -> Result<Call, String> {
    let parsed = parse(tool, args)?;
    let account = parsed.account.as_deref();
    match &parsed.tool {
        ToolArgs::Create(create) => {
            let uid = event_uid(args, context.now_ms);
            let path = href::path(&format!("{}{uid}{NAME_SUFFIX}", create.calendar), "").map_err(
                |error| format!("`calendar` is too long to hold a new event's path: {error}"),
            )?;
            let body = new_event(create, &uid, context.now_ms)?;
            Ok(Call {
                step: CREATE_STEP,
                api: dav_call(
                    &context.settings,
                    account,
                    &path,
                    DavRequest::Put {
                        content_type: CALENDAR_CONTENT_TYPE,
                        body,
                        when: Precondition::Absent,
                    },
                ),
            })
        }
        ToolArgs::Update(update) => Ok(read_call(&context.settings, account, &update.event)),
        ToolArgs::Delete(delete) => Ok(read_call(&context.settings, account, &delete.event)),
    }
}

/// The `GET` of the event at `path` that a change to it, or its removal,
/// starts from.
fn read_call(settings: &Settings, account: Option<&str>, path: &str) -> Call {
    Call {
        step: READ_STEP,
        api: dav_call(
            settings,
            account,
            path,
            DavRequest::Get {
                accept: CALENDAR_ACCEPT,
            },
        ),
    }
}

/// The hint on a failure that leaves `tool`'s write at `step` unknown; `None`
/// at a step that writes nothing.
fn write_hint(tool: Provided, step: u8) -> Option<&'static str> {
    match (tool, step) {
        (Provided::FastmailCalendarEventCreate, CREATE_STEP) => Some(CREATE_UNKNOWN_HINT),
        (Provided::FastmailCalendarEventUpdate, WRITE_STEP) => Some(UPDATE_UNKNOWN_HINT),
        (Provided::FastmailCalendarEventDelete, WRITE_STEP) => Some(DELETE_UNKNOWN_HINT),
        _ => None,
    }
}

fn has_step(tool: Provided, step: u8) -> bool {
    match tool {
        Provided::FastmailCalendarEventCreate => step == CREATE_STEP,
        Provided::FastmailCalendarEventUpdate | Provided::FastmailCalendarEventDelete => {
            step == READ_STEP || step == WRITE_STEP
        }
    }
}

/// What the request does with its nested call's answer: complete, fail, or
/// issue its next step. Every completion is held to the result ceiling; every
/// failure `step` leaves unknown carries the step's write hint.
pub fn advance(state: &InFlight, outcome: Result<Value, NestedError>, context: &Context) -> Next {
    if !has_step(state.tool, state.step) {
        return Next::Fail(no_such_step(state.tool, state.step));
    }
    match step(state, outcome, context) {
        Ok(Next::Complete(value)) => bounded(value, COMPACT_RESULT_CEILING, None),
        Ok(next) => next,
        Err(failure) => Next::Fail(failure.with_hint(write_hint(state.tool, state.step))),
    }
}

/// One step. An `Err` says whether Fastmail is known to have done nothing.
fn step(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    context: &Context,
) -> Result<Next, StepFailure> {
    let parsed = stored_args(state, parse)?;
    let account = parsed.account.as_deref();
    match &parsed.tool {
        ToolArgs::Update(args) => {
            let missing = format!("event at `{}`", args.event);
            if state.step == READ_STEP {
                let (calendar, tag) = read_event(state, outcome, &args.event)?;
                Ok(update_call(args, &calendar, &tag, account, context)?)
            } else {
                let expect = Expect {
                    statuses: STORED,
                    missing: Some(&missing),
                    stale: Some(CHANGED_SINCE_READ),
                    hint: None,
                };
                step_payload(state, outcome, &expect)?;
                Ok(updated(state, args)?)
            }
        }
        ToolArgs::Create(args) => {
            let missing = format!("calendar at `{}`", args.calendar);
            let expect = Expect {
                statuses: STORED,
                missing: Some(&missing),
                stale: Some(NAME_TAKEN),
                hint: None,
            };
            step_payload(state, outcome, &expect)?;
            created(state, args)
        }
        ToolArgs::Delete(args) => {
            if state.step == READ_STEP {
                let (calendar, tag) = read_event(state, outcome, &args.event)?;
                Ok(delete_call(args, &calendar, &tag, account, context)?)
            } else {
                let missing = format!("event at `{}`", args.event);
                let expect = Expect {
                    statuses: DELETED,
                    missing: Some(&missing),
                    stale: Some(CHANGED_SINCE_READ),
                    hint: None,
                };
                step_payload(state, outcome, &expect)?;
                Ok(deleted(state, args)?)
            }
        }
    }
}

/// The event at `path` from its `GET`'s answer, and the strong entity tag a
/// write to it is conditioned on. An answer without one, or data that does
/// not parse, ends the request before any write.
fn read_event(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    path: &str,
) -> Result<(Component, String), StepFailure> {
    let missing = format!("event at `{path}`");
    let expect = Expect {
        statuses: &[OK],
        missing: Some(&missing),
        ..Expect::default()
    };
    let answer = step_payload(state, outcome, &expect)?;
    let tag = etag(&answer).ok_or_else(|| internal(NO_ETAG))?.to_owned();
    let calendar = ical::parse(&answer.body).map_err(|error| malformed(path, error))?;
    Ok((calendar, tag))
}

/// Create's answer: the path the event was put to, and its `UID`, the name
/// without `.ics`.
fn created(state: &InFlight, args: &CreateArgs) -> Result<Next, StepFailure> {
    let event = &state.target.path;
    let uid = event
        .strip_prefix(&args.calendar)
        .and_then(|name| name.strip_suffix(NAME_SUFFIX))
        .filter(|uid| uid.starts_with(NAME_PREFIX) && !uid.contains('/'))
        .ok_or_else(|| {
            internal(format!(
                "in-flight row at {} step {} put to a path that is not a new event's in `{}`",
                state.tool.name(),
                state.step,
                args.calendar
            ))
        })?;
    Ok(Next::Complete(json!({ "event": event, "uid": uid })))
}

/// Update's write, from the event the read answered: the stored `calendar`
/// with its main event changed, `PUT` under `If-Match: tag`. An event the
/// tools do not change, or one too large to write back, is refused and
/// nothing is sent.
fn update_call(
    args: &UpdateArgs,
    calendar: &Component,
    tag: &str,
    account: Option<&str>,
    context: &Context,
) -> Result<Next, Failure> {
    let stored = crate::resource::read(&args.event, calendar)?;
    let Some(master) = stored.master else {
        return Err(invalid(format!(
            "`{}` holds only changed occurrences of a series, which these tools do not change",
            args.event
        )));
    };
    // TODO(calendar-write-no-single-occurrence-edit): a series' time and rule,
    // and one occurrence, are refused, since naming an occurrence in the
    // series' own form needs a time converted between zones.
    if stored.recurring() && args.times.is_some() {
        return Err(invalid(
            "this is a recurring event: these tools change its summary, description and \
             location and not its time or rule; one occurrence cannot be changed either. Change \
             it in Fastmail"
                .to_owned(),
        ));
    }
    let change = Edit {
        summary: args.summary.as_deref(),
        description: args.description.as_deref(),
        location: args.location.as_deref(),
        times: args.times.as_ref().map(|times| (&times.start, &times.end)),
        now_ms: context.now_ms,
    };
    let edited = crate::resource::edit(calendar, master, &change)
        .map_err(|error| invalid(format!("the event cannot be written: {error}")))?;
    let body = ical::write(&edited);
    let width = json_width(&body);
    if width > BODY_MAX_WIRE_BYTES {
        return Err(invalid(format!(
            "this event is too large for these tools to write back ({width} bytes as sent, of \
             {BODY_MAX_WIRE_BYTES}); change it in Fastmail"
        )));
    }
    let derived = Args::from_iter([
        (UPDATED_KEY.to_owned(), json!(args.changed())),
        (OVERRIDES_KEY.to_owned(), json!(stored.overrides)),
    ]);
    Ok(Next::Call {
        call: Call {
            step: WRITE_STEP,
            api: dav_call(
                &context.settings,
                account,
                &args.event,
                DavRequest::Put {
                    content_type: CALENDAR_CONTENT_TYPE,
                    body,
                    when: Precondition::Unchanged(tag),
                },
            ),
        },
        derived,
    })
}

/// Delete's write, from the event the read answered: a `DELETE` under
/// `If-Match: tag`, carrying what the answer says of the event. An event the
/// tools do not change, or a recurring one without `series`, is refused and
/// nothing is sent.
fn delete_call(
    args: &DeleteArgs,
    calendar: &Component,
    tag: &str,
    account: Option<&str>,
    context: &Context,
) -> Result<Next, Failure> {
    let stored = crate::resource::read(&args.event, calendar)?;
    let series = stored.recurring();
    if series && !args.series {
        return Err(invalid(SERIES_NEEDED.to_owned()));
    }
    let mut derived = Args::new();
    put_text(
        &mut derived,
        SUMMARY_KEY,
        stored.main,
        "SUMMARY",
        BRIEF_TEXT_MAX,
    );
    derived.insert(
        START_KEY.to_owned(),
        shape(start(stored.main, &args.event)?, &args.event)?,
    );
    derived.insert(SERIES_KEY.to_owned(), json!(series));
    // TODO(calendar-event-delete-unruled): deleting an event is a tool of its
    // own that a deployment withholds by its clause and grant; whether the
    // published example grants it is the owner's question.
    Ok(Next::Call {
        call: Call {
            step: WRITE_STEP,
            api: dav_call(
                &context.settings,
                account,
                &args.event,
                DavRequest::Delete { unchanged: tag },
            ),
        },
        derived,
    })
}

/// Delete's answer: the event removed, its summary and start as the read
/// tools show them, and `series` when every occurrence went with it.
fn deleted(state: &InFlight, args: &DeleteArgs) -> Result<Next, Failure> {
    let mut out = Args::new();
    out.insert("deleted".into(), json!(true));
    out.insert("event".into(), json!(args.event));
    if let Some(summary) = state.derived.get(SUMMARY_KEY) {
        out.insert("summary".into(), summary.clone());
    }
    let start = state
        .derived
        .get(START_KEY)
        .filter(|start| start.is_object())
        .ok_or_else(|| missing_derived(state, START_KEY))?;
    out.insert("start".into(), start.clone());
    match state.derived.get(SERIES_KEY) {
        Some(Value::Bool(true)) => {
            out.insert("series".into(), json!(true));
        }
        Some(Value::Bool(false)) => {}
        _ => return Err(missing_derived(state, SERIES_KEY)),
    }
    Ok(Next::Complete(Value::Object(out)))
}

/// Update's answer: the event, the fields changed and, for a series with
/// changed occurrences, how many of them kept their own text.
fn updated(state: &InFlight, args: &UpdateArgs) -> Result<Next, Failure> {
    let changed = state
        .derived
        .get(UPDATED_KEY)
        .filter(|changed| changed.is_array())
        .ok_or_else(|| missing_derived(state, UPDATED_KEY))?;
    let mut out = Args::new();
    out.insert("event".into(), json!(args.event));
    out.insert("updated".into(), changed.clone());
    let overrides = derived_u64(state, OVERRIDES_KEY)?;
    if overrides > 0 {
        out.insert("overrides_unchanged".into(), json!(overrides));
    }
    Ok(Next::Complete(Value::Object(out)))
}

#[cfg(test)]
mod tests {
    use brenn_provider_kit::activation::BACKLOG_NS;
    use brenn_provider_kit::testutil::{
        Answered, MemoryHost, failure, http, nested, text_body, truncated,
    };
    use brenn_provider_kit::{
        ApiCall, Bound, CALL_BUDGET, Delivered, ErrorKind, Input, Probes, ToolBounds, check,
        hold_probes, hold_required, input_schema, probes,
    };
    use fastmail_provider::dav::ETAG_MAX;
    use fastmail_provider::testutil::{
        ACCOUNT_PATTERN, HOST_ARGS_CAP_BYTES, account_probes, completed, ics_tagged, incoming,
        settings, written,
    };
    use serde_json::Map;

    use super::*;

    const CALENDAR: &str = "/dav/calendars/user/alice@example.com/work/";
    const CALENDAR_PATTERN: &str = "^/([^/?#]+/)*$";
    const EVENT: &str = "/dav/calendars/user/alice@example.com/work/e1.ics";
    const EVENT_PATTERN: &str = "^(/[^/?#]+)+$";
    const TIME_PATTERN: &str = "^[0-9]{4}-[0-9]{2}-[0-9]{2}(T[0-9]{2}:[0-9]{2}:[0-9]{2}Z?)?$";
    const ZONE_PATTERN: &str = "^[A-Za-z0-9_+-]+(/[A-Za-z0-9_+-]+)*$";
    const RULE_PATTERN: &str = "^RRULE:[A-Za-z0-9=;,+-]{1,500}$";
    /// 2026-11-02T09:00:00Z.
    const NOW: u64 = 1_793_610_000_000;
    const CREATE: Provided = Provided::FastmailCalendarEventCreate;
    const UPDATE: Provided = Provided::FastmailCalendarEventUpdate;
    const DELETE: Provided = Provided::FastmailCalendarEventDelete;
    const VALID_DATA: &str = r#"<?xml version="1.0"?><D:error xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:valid-calendar-data/></D:error>"#;

    fn context() -> Context {
        Context {
            settings: settings(),
            now_ms: NOW,
        }
    }

    fn all_day() -> Value {
        json!({ "calendar": CALENDAR, "summary": "Offsite", "start": "2026-11-02",
                "end": "2026-11-03" })
    }

    fn zoned() -> Value {
        json!({ "calendar": CALENDAR, "summary": "Standup", "start": "2026-11-02T09:00:00",
                "end": "2026-11-02T09:15:00", "time_zone": "Europe/London" })
    }

    /// `base` with `key` set to `value`.
    fn with(base: Value, key: &str, value: Value) -> Value {
        let mut args = base;
        args[key] = value;
        args
    }

    fn call(args: &Value) -> Call {
        plan(CREATE, args, &context()).unwrap()
    }

    fn refused(args: Value) -> String {
        plan(CREATE, &args, &context()).unwrap_err()
    }

    fn state_for(args: &Value, call: &Call) -> InFlight {
        InFlight::new(
            &"r".repeat(32),
            CREATE,
            args,
            call,
            Map::new(),
            1_000,
            1_000,
        )
    }

    /// The create planned from `args` and answered `outcome`.
    fn answered(args: &Value, outcome: Result<Value, NestedError>) -> Next {
        advance(&state_for(args, &call(args)), outcome, &context())
    }

    fn failed(next: Next) -> Failure {
        match next {
            Next::Fail(failure) => failure,
            other => panic!("not failed: {other:?}"),
        }
    }

    /// The text a create of `args` puts: its event's lines between `UID`,
    /// `DTSTAMP` and the end.
    fn event_text(args: &Value, lines: &[&str]) -> String {
        let mut text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:{PRODID}\r\nBEGIN:VEVENT\r\nUID:{}\r\n\
             DTSTAMP:{}\r\n",
            event_uid(args, NOW),
            ical::utc_stamp(NOW)
        );
        for line in lines {
            text.push_str(line);
            text.push_str("\r\n");
        }
        text.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
        text
    }

    fn path_probes() -> Probes {
        let long = format!("/{}/", "x".repeat(href::MAX_PATH - 1));
        Probes {
            admitted: [CALENDAR, "/", "/a%40b/"].map(|p| json!(p)).to_vec(),
            refused: [
                "dav/x/", "/a/b", "/a//", "/a/../", "/a%2Fb/", "/a?b/", "/a#b/", "",
            ]
            .map(|p| json!(p))
            .into_iter()
            .chain([json!(long), json!(1)])
            .collect(),
        }
    }

    /// `bound`'s own probes with `admitted` and `refused` added.
    fn text_probes(bound: &Bound, admitted: &[&str], refused: &[&str]) -> Probes {
        let mut probes = probes(bound).expect("a text bound has probes");
        probes.admitted.extend(admitted.iter().map(|t| json!(t)));
        probes.refused.extend(refused.iter().map(|t| json!(t)));
        probes
    }

    fn rules(lines: &[String]) -> Value {
        json!(lines)
    }

    /// The update arguments that change what the create arguments `base`
    /// would make, of the event at `EVENT`.
    fn as_update(base: Value) -> Value {
        let mut args = base;
        let map = args.as_object_mut().unwrap();
        map.remove("calendar");
        map.remove("recurrence");
        map.insert("event".into(), json!(EVENT));
        args
    }

    fn event_path_probes() -> Probes {
        let long = format!("/{}", "x".repeat(href::MAX_PATH));
        Probes {
            admitted: [EVENT, "/e", "/a%40b/e.ics"].map(|p| json!(p)).to_vec(),
            refused: [
                "e.ics", "/a/", "/", "/a//b", "/a/../b", "/a%2Fb", "/a?b", "/a#b", "",
            ]
            .map(|p| json!(p))
            .into_iter()
            .chain([json!(long), json!(1)])
            .collect(),
        }
    }

    /// `tool`'s probes of each argument, with the arguments they vary.
    fn tool_probes(tool: Provided, key: &str, bound: &Bound) -> (Value, Probes) {
        match (tool, key) {
            (Provided::FastmailCalendarEventCreate, _) => probes_of(key, bound),
            (Provided::FastmailCalendarEventUpdate, "event") => {
                (as_update(all_day()), event_path_probes())
            }
            (Provided::FastmailCalendarEventUpdate, _) => {
                let (base, probes) = probes_of(key, bound);
                (as_update(base), probes)
            }
            (Provided::FastmailCalendarEventDelete, "event") => {
                (json!({ "event": EVENT }), event_path_probes())
            }
            (Provided::FastmailCalendarEventDelete, "series") => (
                json!({ "event": EVENT }),
                Probes {
                    admitted: vec![json!(true), json!(false)],
                    refused: vec![json!("true"), json!(1), json!(null)],
                },
            ),
            (Provided::FastmailCalendarEventDelete, _) => {
                (json!({ "event": EVENT }), probes_of(key, bound).1)
            }
        }
    }

    /// Each create argument's probes, with the arguments they vary.
    fn probes_of(key: &str, bound: &Bound) -> (Value, Probes) {
        let strings = |list: &[&str]| list.iter().map(|t| json!(t)).collect::<Vec<_>>();
        match key {
            "calendar" => (all_day(), path_probes()),
            "summary" | "location" => (all_day(), text_probes(bound, &["a, b; c"], &["a\nb"])),
            "description" => (
                all_day(),
                text_probes(bound, &["a\nb\r\nc\rd", "\\"], &["a\u{7}b", "a\tb"]),
            ),
            "start" => (
                all_day(),
                Probes {
                    admitted: strings(&["2026-11-02", "0001-01-01"]),
                    refused: [
                        "2026-11-02T09:00:00Z",
                        "2026-11-02T09:00:00",
                        "2026-11-03",
                        "2026-11-02T09:00:00+01:00",
                        "2026-11-02T09:00:00.5Z",
                        "2026-11-2",
                        "20261102",
                        "",
                    ]
                    .map(|t| json!(t))
                    .into_iter()
                    .chain([json!(1)])
                    .collect(),
                },
            ),
            "end" => (
                all_day(),
                Probes {
                    admitted: strings(&["2026-11-03", "9999-12-31"]),
                    refused: ["2026-11-02", "2026-11-03T00:00:00Z", "2026-11-3", ""]
                        .map(|t| json!(t))
                        .into_iter()
                        .chain([json!(1)])
                        .collect(),
                },
            ),
            "time_zone" => (
                zoned(),
                Probes {
                    admitted: strings(&[
                        "Europe/London",
                        "America/Argentina/Buenos_Aires",
                        "Etc/GMT+5",
                        "UTC",
                        &"a".repeat(TIME_ZONE_MAX_CHARS),
                    ]),
                    refused: [
                        "",
                        &"a".repeat(TIME_ZONE_MAX_CHARS + 1),
                        "Europe//London",
                        "/UTC",
                        "UTC/",
                        "Europe/London ",
                        "Europe/L\u{f6}ndon",
                    ]
                    .map(|t| json!(t))
                    .into_iter()
                    .chain([json!(1)])
                    .collect(),
                },
            ),
            "recurrence" => {
                let rule = |i: usize| format!("RRULE:FREQ=WEEKLY;INTERVAL={i}");
                let widest = format!("RRULE:{}", "A".repeat(RULE_MAX_CHARS));
                (
                    zoned(),
                    Probes {
                        admitted: vec![
                            rules(&[rule(1)]),
                            rules(&[rule(1), rule(2), rule(3)]),
                            rules(&[widest]),
                            json!(["RRULE:FREQ=DAILY;UNTIL=20261231T235959Z;BYDAY=-1MO,+2TU"]),
                        ],
                        refused: vec![
                            json!([]),
                            rules(&[rule(1), rule(2), rule(3), rule(4)]),
                            rules(&[rule(1), rule(1)]),
                            json!(["RRULE:"]),
                            json!([format!("RRULE:{}", "A".repeat(RULE_MAX_CHARS + 1))]),
                            json!(["EXDATE:20261102T090000Z"]),
                            json!(["rrule:FREQ=DAILY"]),
                            json!(["RRULE:FREQ=DAILY\r\nATTENDEE:mailto:bob@example.com"]),
                            json!(["RRULE:FREQ=DAILY;UNTIL=20261231T23:59:59Z"]),
                            json!([1]),
                            json!("RRULE:FREQ=DAILY"),
                        ],
                    },
                )
            }
            ACCOUNT_ARG => (all_day(), account_probes()),
            _ => panic!("no probes for `{key}`"),
        }
    }

    fn bounds(tool: Provided) -> ToolBounds<'static> {
        let text = |max_chars| Bound::Text {
            max_chars,
            format: None,
        };
        let pattern = |pattern: &str| Bound::Pattern(pattern.to_owned());
        if tool == DELETE {
            return ToolBounds {
                properties: vec![
                    ("event", pattern(EVENT_PATTERN)),
                    ("series", Bound::Bool),
                    (ACCOUNT_ARG, pattern(ACCOUNT_PATTERN)),
                ],
                required: &["event"],
            };
        }
        if tool == UPDATE {
            return ToolBounds {
                properties: vec![
                    ("event", pattern(EVENT_PATTERN)),
                    ("summary", text(TEXT_MAX_CHARS)),
                    ("description", text(DESCRIPTION_MAX_CHARS)),
                    ("location", text(TEXT_MAX_CHARS)),
                    ("start", pattern(TIME_PATTERN)),
                    ("end", pattern(TIME_PATTERN)),
                    ("time_zone", pattern(ZONE_PATTERN)),
                    (ACCOUNT_ARG, pattern(ACCOUNT_PATTERN)),
                ],
                required: &["event"],
            };
        }
        ToolBounds {
            properties: vec![
                ("calendar", pattern(CALENDAR_PATTERN)),
                ("summary", text(TEXT_MAX_CHARS)),
                ("start", pattern(TIME_PATTERN)),
                ("end", pattern(TIME_PATTERN)),
                ("time_zone", pattern(ZONE_PATTERN)),
                ("description", text(DESCRIPTION_MAX_CHARS)),
                ("location", text(TEXT_MAX_CHARS)),
                (
                    "recurrence",
                    Bound::Array {
                        min: 1,
                        max: RECURRENCE_MAX,
                        unique: true,
                        item: Box::new(pattern(RULE_PATTERN)),
                    },
                ),
                (ACCOUNT_ARG, pattern(ACCOUNT_PATTERN)),
            ],
            required: &["calendar", "summary", "start", "end"],
        }
    }

    #[test]
    fn every_tools_schema_states_what_its_parser_enforces() {
        let path = std::env::var("FASTMAIL_CALENDAR_WRITE_SPEC").expect("the spec's path");
        let spec = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        for tool in Provided::ALL {
            let name = tool.name();
            let bounds = bounds(tool);
            if let Err(error) = check(&input_schema(&spec, name), &bounds) {
                panic!("`{name}`'s input_schema: {error}");
            }
            let plan_with = |args: &Value| plan(tool, args, &context()).map(drop);
            let (base, keys) = match tool {
                Provided::FastmailCalendarEventCreate => (all_day(), CreateArgs::KEYS),
                Provided::FastmailCalendarEventUpdate => (as_update(all_day()), UpdateArgs::KEYS),
                Provided::FastmailCalendarEventDelete => {
                    (json!({ "event": EVENT }), DeleteArgs::KEYS)
                }
            };
            for key in bounds.required {
                hold_required(name, key, &base, plan_with);
            }
            let held: Vec<&str> = bounds.properties.iter().map(|(key, _)| *key).collect();
            assert_eq!(held, keys);
            for (key, bound) in &bounds.properties {
                let (base, probes) = tool_probes(tool, key, bound);
                hold_probes(name, key, &probes, &base, plan_with);
            }
        }
    }

    #[test]
    fn create_puts_one_new_event_and_never_over_a_stored_one() {
        let recurring = with(
            with(
                with(zoned(), "recurrence", json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"])),
                "description",
                json!("Notes:\nbring a, b; and c\\d"),
            ),
            "location",
            json!("Room 1"),
        );
        let instant = json!({ "calendar": CALENDAR, "summary": "Call", "account": "alice-work",
                              "start": "2026-11-02T14:00:00Z", "end": "2026-11-02T15:00:00Z" });
        for (args, lines) in [
            (
                all_day(),
                vec![
                    "DTSTART;VALUE=DATE:20261102",
                    "DTEND;VALUE=DATE:20261103",
                    "SUMMARY:Offsite",
                ],
            ),
            (
                instant.clone(),
                vec![
                    "DTSTART:20261102T140000Z",
                    "DTEND:20261102T150000Z",
                    "SUMMARY:Call",
                ],
            ),
            (
                zoned(),
                vec![
                    "DTSTART;TZID=Europe/London:20261102T090000",
                    "DTEND;TZID=Europe/London:20261102T091500",
                    "SUMMARY:Standup",
                ],
            ),
            (
                recurring,
                vec![
                    "DTSTART;TZID=Europe/London:20261102T090000",
                    "DTEND;TZID=Europe/London:20261102T091500",
                    "SUMMARY:Standup",
                    r"DESCRIPTION:Notes:\nbring a\, b\; and c\\d",
                    "LOCATION:Room 1",
                    "RRULE:FREQ=WEEKLY;BYDAY=MO",
                ],
            ),
        ] {
            let call = call(&args);
            assert_eq!(call.step, CREATE_STEP);
            let api = &call.api;
            let uid = event_uid(&args, NOW);
            assert_eq!(api.method, "PUT");
            assert_eq!(api.path, format!("{CALENDAR}{uid}.ics"));
            assert_eq!(api.account.as_deref(), args["account"].as_str());
            assert_eq!(
                (api.provider.as_str(), api.host.as_str()),
                ("fastmail", "api.fastmail.example.com")
            );
            let header = |name: &str| api.headers.get(name).map(String::as_str);
            assert_eq!(header("If-None-Match"), Some("*"));
            assert_eq!(header("If-Match"), None);
            assert_eq!(header("Content-Type"), Some(CALENDAR_CONTENT_TYPE));
            assert_eq!(text_body(api), event_text(&args, &lines));
            let read = ical::parse(text_body(api).as_bytes()).unwrap();
            let event = &read.components[0];
            assert_eq!(event.property("UID").unwrap().value, uid);
            assert!(
                ["ORGANIZER", "ATTENDEE", "VTIMEZONE"]
                    .iter()
                    .all(|name| event.property(name).is_none())
            );
        }
    }

    #[test]
    fn a_new_events_name_is_the_clock_and_a_hash_of_the_arguments() {
        let uid = event_uid(&all_day(), NOW);
        let hash = uid
            .strip_prefix(&format!("brenn-{NOW}-"))
            .unwrap_or_else(|| panic!("{uid}"));
        assert_eq!(hash.len(), 16);
        assert!(hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(event_uid(&all_day(), NOW), uid);
        assert_ne!(event_uid(&all_day(), NOW + 1), uid);
        let other = with(all_day(), "summary", json!("Offsite!"));
        assert_ne!(event_uid(&other, NOW), uid);
        assert_eq!(event_uid(&json!({}), 0), "brenn-0-08f44b07b5901a25");
        let later = Context {
            settings: settings(),
            now_ms: NOW + 1,
        };
        assert_ne!(
            plan(CREATE, &all_day(), &later).unwrap().api.path,
            call(&all_day()).api.path
        );
    }

    #[test]
    fn create_answers_the_new_events_path_and_uid() {
        let args = zoned();
        let uid = event_uid(&args, NOW);
        for status in [201, 204, 200] {
            assert_eq!(
                completed(answered(&args, written(status, Some("\"e1\"")))),
                json!({ "event": format!("{CALENDAR}{uid}.ics"), "uid": uid })
            );
        }
    }

    #[test]
    fn a_refused_create_says_what_fastmail_refused_and_that_nothing_was_made() {
        let args = all_day();
        let path = &call(&args).api.path;
        assert_eq!(
            failed(answered(&args, http(412, ""))),
            internal(format!("{NAME_TAKEN} (HTTP 412)"))
        );
        assert_eq!(
            failed(answered(&args, http(404, ""))),
            failure(
                ErrorKind::InvalidArgs,
                &format!("no calendar at `{CALENDAR}`")
            )
        );
        assert_eq!(
            failed(answered(&args, http(403, VALID_DATA))),
            internal(format!(
                "Fastmail answered HTTP 403 to PUT `{path}`: precondition `valid-calendar-data` \
                 failed"
            ))
        );
        let refused = failed(answered(&args, http(401, "")));
        assert!(refused.detail.contains("HTTP 401"), "{refused:?}");
        for outcome in [
            http(400, ""),
            http(401, ""),
            http(403, VALID_DATA),
            http(429, ""),
        ] {
            let refused = failed(answered(&args, outcome));
            assert!(!refused.detail.contains("may have"), "{refused:?}");
        }
    }

    #[test]
    fn a_lost_answer_to_a_create_says_the_event_may_exist() {
        let args = all_day();
        for outcome in [
            http(500, "<html>secret</html>"),
            http(507, ""),
            nested("internal", "result exceeds [messaging] max_body_bytes"),
            truncated(201, json!(300000), ""),
            written(202, None),
        ] {
            let shown = format!("{outcome:?}");
            let lost = failed(answered(&args, outcome));
            assert_eq!(lost.kind, ErrorKind::Internal, "{shown}");
            assert!(
                lost.detail.ends_with(&format!("; {CREATE_UNKNOWN_HINT}")),
                "{shown}: {lost:?}"
            );
            assert!(!lost.detail.contains("secret"), "{lost:?}");
        }
        assert_eq!(write_hint(CREATE, CREATE_STEP), Some(CREATE_UNKNOWN_HINT));
    }

    #[test]
    fn create_refuses_times_of_two_kinds_a_bare_local_time_and_a_zone_it_cannot_use() {
        let times = |start: &str, end: &str| {
            with(with(all_day(), "start", json!(start)), "end", json!(end))
        };
        let one_kind = "`start` and `end` must be of one kind: both dates, both UTC instants or \
                        both local times";
        assert_eq!(
            refused(times("2026-11-02", "2026-11-02T10:00:00Z")),
            one_kind
        );
        assert_eq!(
            refused(times("2026-11-02T09:00:00Z", "2026-11-02T10:00:00")),
            one_kind
        );
        assert_eq!(
            refused(times("2026-11-02T09:00:00", "2026-11-02T10:00:00")),
            "`start` and `end` are local times, which need `time_zone`: give it, or write them \
             as UTC instants ending in Z"
        );
        let zone_refusal = "`time_zone` is only for local times (2026-11-02T09:00:00): dates and \
                            UTC instants take none";
        assert_eq!(
            refused(with(all_day(), "time_zone", json!("Europe/London"))),
            zone_refusal
        );
        let instant = times("2026-11-02T09:00:00Z", "2026-11-02T10:00:00Z");
        assert_eq!(
            refused(with(instant.clone(), "time_zone", json!("UTC"))),
            zone_refusal
        );
        for (start, end) in [
            ("2026-11-02", "2026-11-02"),
            ("2026-11-02T09:00:00Z", "2026-11-02T08:59:59Z"),
        ] {
            assert_eq!(
                refused(times(start, end)),
                "`end` must be later than `start`"
            );
        }
        let zoned_back = with(zoned(), "end", json!("2026-11-02T09:00:00"));
        assert_eq!(refused(zoned_back), "`end` must be later than `start`");
        let weekly = json!(["RRULE:FREQ=WEEKLY"]);
        assert!(
            refused(with(instant, "recurrence", weekly.clone()))
                .starts_with("`recurrence` on an event with a time needs `time_zone`")
        );
        call(&with(all_day(), "recurrence", weekly.clone()));
        call(&with(zoned(), "recurrence", weekly));
        assert_eq!(
            refused(with(all_day(), "colour", json!("red"))),
            "unknown argument `colour`; this tool takes `calendar`, `summary`, `start`, `end`, \
             `time_zone`, `description`, `location`, `recurrence`, `account`"
        );
    }

    #[test]
    fn a_calendar_too_long_to_hold_a_new_events_path_is_refused() {
        let name = event_uid(&all_day(), NOW).len() + NAME_SUFFIX.len();
        let calendar = |len: usize| format!("/{}/", "c".repeat(len - 2));
        let fits = calendar(href::MAX_PATH - name);
        let args = with(all_day(), "calendar", json!(fits));
        assert_eq!(call(&args).api.path.len(), href::MAX_PATH);
        let over = calendar(href::MAX_PATH - name + 1);
        let error = refused(with(all_day(), "calendar", json!(over)));
        assert!(
            error.starts_with("`calendar` is too long to hold a new event's path"),
            "{error}"
        );
    }

    #[test]
    fn a_step_a_tool_does_not_have_is_refused_as_such() {
        let args = all_day();
        let mut state = state_for(&args, &call(&args));
        state.step = CREATE_STEP + 1;
        assert_eq!(
            advance(&state, written(201, None), &context()),
            Next::Fail(no_such_step(CREATE, CREATE_STEP + 1))
        );
        assert_eq!(write_hint(CREATE, CREATE_STEP + 1), None);
    }

    #[test]
    fn the_widest_create_fits_the_args_cap() {
        let name = event_uid(&all_day(), NOW).len() + NAME_SUFFIX.len();
        let rule = |i: usize| format!("RRULE:{i}{}", "A".repeat(RULE_MAX_CHARS - 1));
        // A backslash is the widest character once escaped twice: two in the
        // event's text, four in the call's arguments.
        let args = json!({
            "calendar": format!("/{}/", "c".repeat(href::MAX_PATH - name - 2)),
            "summary": "\\".repeat(TEXT_MAX_CHARS),
            "location": "\\".repeat(TEXT_MAX_CHARS),
            "description": "\\".repeat(DESCRIPTION_MAX_CHARS),
            "start": "2026-11-02T09:00:00", "end": "2026-11-02T10:00:00",
            "time_zone": "Z".repeat(TIME_ZONE_MAX_CHARS),
            "recurrence": [rule(1), rule(2), rule(3)],
            "account": "a".repeat(255),
        });
        let width = serde_json::to_vec(&call(&args).api).unwrap().len();
        assert!(width <= HOST_ARGS_CAP_BYTES, "{width} bytes");
    }

    #[test]
    fn a_create_runs_one_put_then_completes_and_excess_waits() {
        let host = MemoryHost::new();
        let id = "q".repeat(32);
        let activate = |requests, results| {
            host.activate::<Fastmail>(
                &context(),
                Input {
                    now_ms: NOW,
                    requests,
                    results,
                },
            )
            .unwrap();
        };
        let args = all_day();
        activate(
            vec![incoming(
                &id,
                "fastmail-calendar-event-create",
                args.clone(),
            )],
            vec![],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        let put: &ApiCall = &calls[0].call;
        assert_eq!(put.method, "PUT");
        let uid = event_uid(&args, NOW);
        assert_eq!(put.path, format!("{CALENDAR}{uid}.ics"));
        activate(
            vec![],
            vec![Delivered {
                call_id: calls[0].call_id.clone(),
                serving: Some(id.clone()),
                outcome: written(201, None),
            }],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, id);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(
            value,
            &json!({ "event": format!("{CALENDAR}{uid}.ics"), "uid": uid })
        );
        assert!(host.take_calls().is_empty());

        let requests = (0..=CALL_BUDGET)
            .map(|i| {
                let args = with(all_day(), "summary", json!(format!("Offsite {i}")));
                incoming(&format!("{i:032}"), "fastmail-calendar-event-create", args)
            })
            .collect();
        activate(requests, vec![]);
        assert_eq!(host.take_calls().len(), CALL_BUDGET);
        assert_eq!(host.rows(BACKLOG_NS).len(), 1);
    }

    /// An event as Fastmail might store it: a zoned time with a duration,
    /// text with parameters and restated copies, a property this repository
    /// does not know, and an alarm.
    const STORED_EVENT: &str = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Example//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:e1\r\n",
        "DTSTAMP:20260101T000000Z\r\n",
        "DTSTART;TZID=Europe/London:20261102T090000\r\n",
        "DURATION:PT1H\r\n",
        "SUMMARY;LANGUAGE=en:Old\r\n",
        "DESCRIPTION:old\r\n",
        "X-ALT-DESC;FMTTYPE=text/html:<p>old</p>\r\n",
        "LOCATION:Room 1\r\n",
        "GEO:1.5;2.5\r\n",
        "X-APPLE-STRUCTURED-LOCATION;VALUE=URI:geo:1.5,2.5\r\n",
        "x-custom;X-P=\"a:b\":kept\r\n",
        "BEGIN:VALARM\r\n",
        "TRIGGER:-PT5M\r\n",
        "ACTION:DISPLAY\r\n",
        "END:VALARM\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );
    const TAG: &str = "\"t1\"";

    /// `events`, each a `VEVENT`'s lines, as a stored resource.
    fn resource_of(events: &[&str]) -> String {
        let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
        for lines in events {
            text.push_str(&format!(
                "BEGIN:VEVENT\r\nUID:e1\r\nDTSTART:20261102T090000Z\r\n{lines}END:VEVENT\r\n"
            ));
        }
        text.push_str("END:VCALENDAR\r\n");
        text
    }

    fn update(changes: Value) -> Value {
        let mut args = json!({ "event": EVENT });
        for (key, value) in changes.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    }

    /// `tool`'s row with `args` at `call`'s step, carrying `derived`.
    fn row_at(tool: Provided, args: &Value, call: &Call, derived: Args) -> InFlight {
        InFlight::new(&"r".repeat(32), tool, args, call, derived, 1_000, 1_000)
    }

    /// An update with `args` whose read was answered `outcome`.
    fn read_answered(args: &Value, outcome: Result<Value, NestedError>) -> Next {
        read_answered_by(UPDATE, args, outcome)
    }

    /// `tool` with `args`, its read answered `outcome`.
    fn read_answered_by(tool: Provided, args: &Value, outcome: Result<Value, NestedError>) -> Next {
        let read = plan(tool, args, &context()).unwrap();
        advance(&row_at(tool, args, &read, Map::new()), outcome, &context())
    }

    /// The write an update with `args` issues after reading `stored`, and its
    /// `derived`.
    fn write_after(args: &Value, stored: &str) -> (Call, Args) {
        match read_answered(args, ics_tagged(stored, Some(TAG))) {
            Next::Call { call, derived } => (call, derived),
            other => panic!("the read issues the write: {other:?}"),
        }
    }

    /// An update with `args` of `stored`, its write answered `outcome`.
    fn write_answered(args: &Value, stored: &str, outcome: Result<Value, NestedError>) -> Next {
        let (call, derived) = write_after(args, stored);
        advance(&row_at(UPDATE, args, &call, derived), outcome, &context())
    }

    /// The unfolded lines of the event an update with `args` writes back.
    fn written_lines(args: &Value, stored: &str) -> Vec<String> {
        let (call, _) = write_after(args, stored);
        text_body(&call.api)
            .split("\r\n")
            .map(str::to_owned)
            .collect()
    }

    fn stored_lines() -> Vec<String> {
        STORED_EVENT.split("\r\n").map(str::to_owned).collect()
    }

    /// `lines` without any line beginning with one of `prefixes`.
    fn without(lines: Vec<String>, prefixes: &[&str]) -> Vec<String> {
        lines
            .into_iter()
            .filter(|line| !prefixes.iter().any(|p| line.starts_with(p)))
            .collect()
    }

    const STAMP: &str = "20261102T090000Z";

    #[test]
    fn update_reads_the_event_then_puts_it_back_only_if_unchanged() {
        let args = update(json!({ "summary": "New", "account": "alice-work" }));
        let read = plan(UPDATE, &args, &context()).unwrap();
        assert_eq!(read.step, READ_STEP);
        assert_eq!(
            (read.api.method.as_str(), read.api.path.as_str()),
            ("GET", EVENT)
        );
        assert_eq!(read.api.body, None);
        assert_eq!(read.api.account.as_deref(), Some("alice-work"));
        assert_eq!(
            read.api.headers.get("Accept").map(String::as_str),
            Some("text/calendar")
        );
        let (write, derived) = write_after(&args, STORED_EVENT);
        assert_eq!(write.step, WRITE_STEP);
        let api = &write.api;
        assert_eq!((api.method.as_str(), api.path.as_str()), ("PUT", EVENT));
        assert_eq!(api.account.as_deref(), Some("alice-work"));
        let header = |name: &str| api.headers.get(name).map(String::as_str);
        assert_eq!(header("If-Match"), Some(TAG));
        assert_eq!(header("If-None-Match"), None);
        assert_eq!(header("Content-Type"), Some(CALENDAR_CONTENT_TYPE));
        assert_eq!(
            derived,
            Args::from_iter([
                (UPDATED_KEY.into(), json!(["summary"])),
                (OVERRIDES_KEY.into(), json!(0)),
            ])
        );
        let mut expected = stored_lines();
        expected[5] = format!("DTSTAMP:{STAMP}");
        expected[8] = "SUMMARY:New".to_owned();
        expected.insert(15, format!("LAST-MODIFIED:{STAMP}"));
        assert_eq!(written_lines(&args, STORED_EVENT), expected);
    }

    #[test]
    fn a_new_description_or_location_drops_what_restated_the_old_one() {
        let lines = written_lines(
            &update(json!({ "description": "Line one\nline two, three" })),
            STORED_EVENT,
        );
        assert!(lines.contains(&r"DESCRIPTION:Line one\nline two\, three".to_owned()));
        assert!(!lines.iter().any(|l| l.starts_with("X-ALT-DESC")));
        assert!(lines.contains(&"GEO:1.5;2.5".to_owned()));
        let lines = written_lines(&update(json!({ "location": "Room 2" })), STORED_EVENT);
        assert!(lines.contains(&"LOCATION:Room 2".to_owned()));
        assert_eq!(
            without(lines, &["LOCATION", "DTSTAMP", "LAST-MODIFIED"]),
            without(
                stored_lines(),
                &["LOCATION", "GEO", "X-APPLE-STRUCTURED-LOCATION", "DTSTAMP"]
            )
        );
    }

    #[test]
    fn a_time_change_replaces_the_start_and_a_duration_with_an_end() {
        for (times, start, end) in [
            (
                json!({ "start": "2026-11-03T10:00:00", "end": "2026-11-03T11:30:00",
                        "time_zone": "America/New_York" }),
                "DTSTART;TZID=America/New_York:20261103T100000",
                "DTEND;TZID=America/New_York:20261103T113000",
            ),
            (
                json!({ "start": "2026-11-03T10:00:00Z", "end": "2026-11-03T11:00:00Z" }),
                "DTSTART:20261103T100000Z",
                "DTEND:20261103T110000Z",
            ),
            (
                json!({ "start": "2026-11-03", "end": "2026-11-04" }),
                "DTSTART;VALUE=DATE:20261103",
                "DTEND;VALUE=DATE:20261104",
            ),
        ] {
            let args = update(times);
            let lines = written_lines(&args, STORED_EVENT);
            let at = lines.iter().position(|l| l.starts_with("DTSTART")).unwrap();
            assert_eq!(lines[at], start);
            assert!(lines.contains(&end.to_owned()), "{lines:?}");
            assert!(!lines.iter().any(|l| l.starts_with("DURATION")));
            let (_, derived) = write_after(&args, STORED_EVENT);
            assert_eq!(derived[UPDATED_KEY], json!(["start", "end"]));
        }
        let stored = resource_of(&["DTEND:20261102T100000Z\r\nSUMMARY:a\r\n"]);
        let lines = written_lines(
            &update(json!({ "start": "2026-11-03T10:00:00Z", "end": "2026-11-03T11:00:00Z" })),
            &stored,
        );
        assert_eq!(
            lines[3..6],
            [
                "UID:e1",
                "DTSTART:20261103T100000Z",
                "DTEND:20261103T110000Z"
            ]
        );
    }

    #[test]
    fn update_answers_the_event_and_what_changed() {
        let args = update(json!({ "summary": "New", "location": "Room 2" }));
        for status in [204, 200, 201] {
            assert_eq!(
                completed(write_answered(&args, STORED_EVENT, written(status, None))),
                json!({ "event": EVENT, "updated": ["summary", "location"] })
            );
        }
    }

    #[test]
    fn a_series_text_change_reaches_the_series_and_counts_the_overrides() {
        let stored = resource_of(&[
            "RECURRENCE-ID:20261109T090000Z\r\nSUMMARY:Moved\r\n",
            "SUMMARY:Weekly\r\nRRULE:FREQ=WEEKLY\r\n",
            "RECURRENCE-ID:20261116T090000Z\r\nSUMMARY:Moved too\r\n",
        ]);
        let args = update(json!({ "summary": "Renamed" }));
        let (call, derived) = write_after(&args, &stored);
        assert_eq!(derived[OVERRIDES_KEY], json!(2));
        let body = text_body(&call.api);
        for kept in [
            "SUMMARY:Moved\r\n",
            "SUMMARY:Moved too\r\n",
            "SUMMARY:Renamed\r\n",
        ] {
            assert!(body.contains(kept), "{body}");
        }
        assert!(!body.contains("SUMMARY:Weekly"));
        assert_eq!(
            completed(write_answered(&args, &stored, written(204, None))),
            json!({ "event": EVENT, "updated": ["summary"], "overrides_unchanged": 2 })
        );
        let rule_only = resource_of(&["SUMMARY:Daily\r\nRRULE:FREQ=DAILY\r\n"]);
        assert_eq!(
            completed(write_answered(&args, &rule_only, written(204, None))),
            json!({ "event": EVENT, "updated": ["summary"] })
        );
    }

    #[test]
    fn a_read_without_a_strong_entity_tag_writes_nothing() {
        let args = update(json!({ "summary": "New" }));
        for tag in [None, Some("W/\"t1\""), Some("t1")] {
            assert_eq!(
                failed(read_answered(&args, ics_tagged(STORED_EVENT, tag))),
                internal(NO_ETAG),
                "{tag:?}"
            );
        }
        let cut = failed(read_answered(
            &args,
            truncated(200, json!(300000), "BEGIN:VCALENDAR"),
        ));
        assert_eq!(cut.kind, ErrorKind::Internal);
        assert!(!cut.detail.contains("may already"), "{cut:?}");
    }

    #[test]
    fn an_event_update_and_delete_may_not_change_is_refused_before_any_write() {
        let text = update(json!({ "summary": "New" }));
        let times = update(json!({ "start": "2026-11-03", "end": "2026-11-04" }));
        let people = failure(ErrorKind::InvalidArgs, resource::INVOLVES_PEOPLE);
        let recurring = failure(
            ErrorKind::InvalidArgs,
            "this is a recurring event: these tools change its summary, description and location \
             and not its time or rule; one occurrence cannot be changed either. Change it in \
             Fastmail",
        );
        let only = failure(
            ErrorKind::InvalidArgs,
            &format!(
                "`{EVENT}` holds only changed occurrences of a series, which these tools do not \
                 change"
            ),
        );
        let override_line = "RECURRENCE-ID:20261109T090000Z\r\n";
        let remove = json!({ "event": EVENT, "series": true });
        for (args, stored, refusal) in [
            (
                &remove,
                resource_of(&["ORGANIZER:mailto:alice@example.com\r\n"]),
                people.clone(),
            ),
            (
                &remove,
                resource_of(&[
                    "RRULE:FREQ=DAILY\r\n",
                    &format!("{override_line}ATTENDEE:mailto:bob@example.com\r\n"),
                ]),
                people.clone(),
            ),
            (
                &remove,
                resource_of(&[]),
                failure(ErrorKind::InvalidArgs, &format!("`{EVENT}` holds no event")),
            ),
            (
                &text,
                resource_of(&["ORGANIZER:mailto:alice@example.com\r\n"]),
                people.clone(),
            ),
            (
                &text,
                resource_of(&["ATTENDEE:mailto:bob@example.com\r\n"]),
                people.clone(),
            ),
            (
                &text,
                resource_of(&[
                    "RRULE:FREQ=DAILY\r\n",
                    &format!("{override_line}ATTENDEE:mailto:bob@example.com\r\n"),
                ]),
                people,
            ),
            (
                &times,
                resource_of(&["RRULE:FREQ=DAILY\r\n"]),
                recurring.clone(),
            ),
            (
                &times,
                resource_of(&["RDATE:20261110T090000Z\r\n"]),
                recurring.clone(),
            ),
            (&times, resource_of(&["", override_line]), recurring),
            (&text, resource_of(&[override_line]), only.clone()),
            (&times, resource_of(&[override_line]), only),
            (
                &text,
                resource_of(&[]),
                failure(ErrorKind::InvalidArgs, &format!("`{EVENT}` holds no event")),
            ),
        ] {
            let tool = if args == &remove { DELETE } else { UPDATE };
            assert_eq!(
                failed(read_answered_by(tool, args, ics_tagged(&stored, Some(TAG)))),
                refusal,
                "{stored}"
            );
        }
        for stored in [
            resource_of(&["", ""]),
            "BEGIN:VCALENDAR\r\nnot a line\r\nEND:VCALENDAR\r\n".to_owned(),
            "BEGIN:VCALENDAR\r\n".to_owned(),
        ] {
            let refused = failed(read_answered(&text, ics_tagged(&stored, Some(TAG))));
            assert_eq!(refused.kind, ErrorKind::Internal, "{stored}");
            assert!(
                refused
                    .detail
                    .starts_with(&format!("Fastmail's calendar data at `{EVENT}`: ")),
                "{refused:?}"
            );
            assert!(!refused.detail.contains("not a line"), "{refused:?}");
        }
    }

    #[test]
    fn an_event_too_large_to_write_back_is_refused() {
        let args = update(json!({ "summary": "New" }));
        let sized = |chars: usize| {
            STORED_EVENT.replace(
                "x-custom;X-P=\"a:b\":kept\r\n",
                &format!("X-BIG:{}\r\n", "a".repeat(chars)),
            )
        };
        let fits = sized(55_000);
        let (call, _) = write_after(&args, &fits);
        assert!(json_width(text_body(&call.api)) <= BODY_MAX_WIRE_BYTES);
        let over = failed(read_answered(&args, ics_tagged(&sized(60_000), Some(TAG))));
        assert_eq!(over.kind, ErrorKind::InvalidArgs);
        assert!(
            over.detail
                .starts_with("this event is too large for these tools to write back ("),
            "{over:?}"
        );
        assert!(
            over.detail
                .ends_with(" bytes as sent, of 60000); change it in Fastmail"),
            "{over:?}"
        );
    }

    #[test]
    fn the_widest_write_back_fits_the_args_cap() {
        let path = format!("/{}", "e".repeat(href::MAX_PATH - 1));
        let tag = format!("\"{}\"", "t".repeat(ETAG_MAX - 2));
        let account = "a".repeat(255);
        let api = dav_call(
            &settings(),
            Some(&account),
            &path,
            DavRequest::Put {
                content_type: CALENDAR_CONTENT_TYPE,
                body: "a".repeat(BODY_MAX_WIRE_BYTES),
                when: Precondition::Unchanged(&tag),
            },
        );
        let width = serde_json::to_vec(&api).unwrap().len();
        assert!(width <= HOST_ARGS_CAP_BYTES, "{width} bytes");
    }

    #[test]
    fn a_refused_update_says_what_fastmail_refused_and_that_nothing_changed() {
        let args = update(json!({ "summary": "New" }));
        let missing = failure(ErrorKind::InvalidArgs, &format!("no event at `{EVENT}`"));
        assert_eq!(failed(read_answered(&args, http(404, ""))), missing);
        assert_eq!(
            failed(write_answered(&args, STORED_EVENT, http(404, ""))),
            missing
        );
        assert_eq!(
            failed(write_answered(&args, STORED_EVENT, http(412, ""))),
            internal(format!("{CHANGED_SINCE_READ} (HTTP 412)"))
        );
        for outcome in [http(401, ""), http(403, VALID_DATA), http(429, "")] {
            let refused = failed(write_answered(&args, STORED_EVENT, outcome));
            assert!(!refused.detail.contains("may already"), "{refused:?}");
        }
        let refused = failed(read_answered(&args, http(401, "")));
        assert!(refused.detail.contains("HTTP 401"), "{refused:?}");
    }

    #[test]
    fn a_lost_answer_to_an_update_says_the_change_may_be_applied() {
        let args = update(json!({ "summary": "New" }));
        for outcome in [
            || http(500, "<html>secret</html>"),
            || nested("internal", "result exceeds [messaging] max_body_bytes"),
            || written(202, None),
        ] {
            let lost = failed(write_answered(&args, STORED_EVENT, outcome()));
            assert!(
                lost.detail.ends_with(&format!("; {UPDATE_UNKNOWN_HINT}")),
                "{lost:?}"
            );
            assert!(!lost.detail.contains("secret"), "{lost:?}");
            let read = failed(read_answered(&args, outcome()));
            assert!(!read.detail.contains(UPDATE_UNKNOWN_HINT), "{read:?}");
        }
        assert_eq!(write_hint(UPDATE, READ_STEP), None);
        assert_eq!(write_hint(UPDATE, WRITE_STEP), Some(UPDATE_UNKNOWN_HINT));
    }

    #[test]
    fn update_refuses_no_change_a_lone_time_and_a_calendar_path() {
        let refused = |args: Value| plan(UPDATE, &args, &context()).unwrap_err();
        assert_eq!(
            refused(update(json!({}))),
            "give at least one change: `summary`, `description`, `location`, or `start` and `end`"
        );
        assert_eq!(
            refused(update(json!({ "start": "2026-11-03" }))),
            "`start` and `end` are given together"
        );
        assert_eq!(
            refused(update(json!({ "end": "2026-11-03", "summary": "a" }))),
            "`start` and `end` are given together"
        );
        assert_eq!(
            refused(update(json!({ "time_zone": "UTC", "summary": "a" }))),
            "`time_zone` is given only with `start` and `end`"
        );
        assert_eq!(
            refused(json!({ "event": CALENDAR, "summary": "a" })),
            "`event` must be an `event` path from `FastmailCalendarEvents`, beginning with `/` \
             and not ending in `/`"
        );
        assert!(
            refused(update(
                json!({ "summary": "a", "recurrence": ["RRULE:FREQ=DAILY"] })
            ))
            .starts_with("unknown argument `recurrence`")
        );
    }

    #[test]
    fn an_update_step_it_does_not_have_is_refused_as_such() {
        let args = update(json!({ "summary": "New" }));
        let (call, derived) = write_after(&args, STORED_EVENT);
        let mut state = row_at(UPDATE, &args, &call, derived);
        state.step = WRITE_STEP + 1;
        assert_eq!(
            advance(&state, written(204, None), &context()),
            Next::Fail(no_such_step(UPDATE, WRITE_STEP + 1))
        );
        let mut state = row_at(UPDATE, &args, &call, Map::new());
        state.step = WRITE_STEP;
        assert_eq!(
            failed(advance(&state, written(204, None), &context())),
            internal(format!(
                "in-flight row at fastmail-calendar-event-update step {WRITE_STEP} carries no \
                 `{UPDATED_KEY}`; {UPDATE_UNKNOWN_HINT}"
            ))
        );
    }

    #[test]
    fn an_update_runs_a_get_then_a_put_then_completes() {
        let host = MemoryHost::new();
        let id = "u".repeat(32);
        let activate = |requests, results| {
            host.activate::<Fastmail>(
                &context(),
                Input {
                    now_ms: NOW,
                    requests,
                    results,
                },
            )
            .unwrap();
        };
        let args = update(json!({ "location": "Room 2" }));
        activate(
            vec![incoming(&id, "fastmail-calendar-event-update", args)],
            vec![],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.method, "GET");
        activate(
            vec![],
            vec![Delivered {
                call_id: calls[0].call_id.clone(),
                serving: Some(id.clone()),
                outcome: ics_tagged(STORED_EVENT, Some(TAG)),
            }],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.method, "PUT");
        assert_eq!(
            calls[0].call.headers.get("If-Match").map(String::as_str),
            Some(TAG)
        );
        activate(
            vec![],
            vec![Delivered {
                call_id: calls[0].call_id.clone(),
                serving: Some(id.clone()),
                outcome: written(204, None),
            }],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(value, &json!({ "event": EVENT, "updated": ["location"] }));
    }

    fn delete(series: Option<bool>) -> Value {
        let mut args = json!({ "event": EVENT });
        if let Some(series) = series {
            args["series"] = json!(series);
        }
        args
    }

    /// The `DELETE` a delete with `args` issues after reading `stored`, and
    /// its `derived`.
    fn delete_after(args: &Value, stored: &str) -> (Call, Args) {
        match read_answered_by(DELETE, args, ics_tagged(stored, Some(TAG))) {
            Next::Call { call, derived } => (call, derived),
            other => panic!("the read issues the delete: {other:?}"),
        }
    }

    /// A delete with `args` of `stored`, its `DELETE` answered `outcome`.
    fn delete_answered(args: &Value, stored: &str, outcome: Result<Value, NestedError>) -> Next {
        let (call, derived) = delete_after(args, stored);
        advance(&row_at(DELETE, args, &call, derived), outcome, &context())
    }

    #[test]
    fn delete_reads_the_event_then_deletes_it_only_if_unchanged() {
        let args = json!({ "event": EVENT, "account": "alice-work" });
        let read = plan(DELETE, &args, &context()).unwrap();
        assert_eq!(read.step, READ_STEP);
        assert_eq!(
            (read.api.method.as_str(), read.api.path.as_str()),
            ("GET", EVENT)
        );
        assert_eq!(read.api.account.as_deref(), Some("alice-work"));
        assert_eq!(
            read.api.headers.get("Accept").map(String::as_str),
            Some("text/calendar")
        );
        let (call, derived) = delete_after(&args, STORED_EVENT);
        assert_eq!(call.step, WRITE_STEP);
        let api = &call.api;
        assert_eq!((api.method.as_str(), api.path.as_str()), ("DELETE", EVENT));
        assert_eq!(api.body, None);
        assert_eq!(api.account.as_deref(), Some("alice-work"));
        assert_eq!(api.headers.get("If-Match").map(String::as_str), Some(TAG));
        assert_eq!(api.headers.get("If-None-Match"), None);
        assert_eq!(
            derived,
            Args::from_iter([
                (SUMMARY_KEY.into(), json!("Old")),
                (
                    START_KEY.into(),
                    json!({ "date_time": "2026-11-02T09:00:00", "time_zone": "Europe/London" })
                ),
                (SERIES_KEY.into(), json!(false)),
            ])
        );
    }

    #[test]
    fn delete_answers_what_it_removed() {
        for status in [204, 200] {
            assert_eq!(
                completed(delete_answered(
                    &delete(None),
                    STORED_EVENT,
                    written(status, None)
                )),
                json!({ "deleted": true, "event": EVENT, "summary": "Old",
                        "start": { "date_time": "2026-11-02T09:00:00",
                                   "time_zone": "Europe/London" } })
            );
        }
        let untitled = resource_of(&[""]);
        assert_eq!(
            completed(delete_answered(
                &delete(None),
                &untitled,
                written(204, None)
            )),
            json!({ "deleted": true, "event": EVENT,
                    "start": { "date_time": "2026-11-02T09:00:00Z" } })
        );
        let long = resource_of(&[&format!("SUMMARY:{}\r\n", "a".repeat(BRIEF_TEXT_MAX + 5))]);
        let (_, derived) = delete_after(&delete(None), &long);
        assert_eq!(
            derived[SUMMARY_KEY],
            json!(format!("{}…", "a".repeat(BRIEF_TEXT_MAX)))
        );
        let lost = failed(delete_answered(
            &delete(None),
            STORED_EVENT,
            written(201, None),
        ));
        assert!(lost.detail.contains("HTTP 201"), "{lost:?}");
    }

    #[test]
    fn a_recurring_event_is_deleted_only_as_a_whole_series() {
        let series_needed = failure(ErrorKind::InvalidArgs, SERIES_NEEDED);
        let override_line = "RECURRENCE-ID:20261109T090000Z\r\n";
        let weekly = resource_of(&[
            "SUMMARY:Weekly\r\nRRULE:FREQ=WEEKLY\r\n",
            &format!("{override_line}SUMMARY:Moved\r\n"),
        ]);
        let only = resource_of(&[
            "RECURRENCE-ID:20261116T090000Z\r\nSUMMARY:Later\r\n",
            &format!("{override_line}SUMMARY:Earlier\r\n"),
        ]);
        let dated = resource_of(&["RDATE:20261110T090000Z\r\n"]);
        for stored in [&weekly, &only, &dated] {
            for args in [delete(None), delete(Some(false))] {
                assert_eq!(
                    failed(read_answered_by(
                        DELETE,
                        &args,
                        ics_tagged(stored, Some(TAG))
                    )),
                    series_needed,
                    "{stored}"
                );
            }
        }
        assert_eq!(
            completed(delete_answered(
                &delete(Some(true)),
                &weekly,
                written(204, None)
            )),
            json!({ "deleted": true, "event": EVENT, "summary": "Weekly",
                    "start": { "date_time": "2026-11-02T09:00:00Z" }, "series": true })
        );
        assert_eq!(
            completed(delete_answered(
                &delete(Some(true)),
                &only,
                written(204, None)
            )),
            json!({ "deleted": true, "event": EVENT, "summary": "Earlier",
                    "start": { "date_time": "2026-11-02T09:00:00Z" }, "series": true })
        );
        assert_eq!(
            completed(delete_answered(
                &delete(Some(true)),
                STORED_EVENT,
                written(204, None)
            ))["series"],
            Value::Null
        );
    }

    #[test]
    fn a_delete_of_an_event_without_a_start_or_a_strong_tag_sends_nothing() {
        let no_start =
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:e1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let refused = failed(read_answered_by(
            DELETE,
            &delete(None),
            ics_tagged(no_start, Some(TAG)),
        ));
        assert_eq!(
            refused,
            malformed(EVENT, "an event has no DTSTART"),
            "{refused:?}"
        );
        for tag in [None, Some("W/\"t1\"")] {
            assert_eq!(
                failed(read_answered_by(
                    DELETE,
                    &delete(None),
                    ics_tagged(STORED_EVENT, tag)
                )),
                internal(NO_ETAG)
            );
        }
    }

    #[test]
    fn a_refused_or_lost_delete_says_what_happened() {
        let args = delete(None);
        let missing = failure(ErrorKind::InvalidArgs, &format!("no event at `{EVENT}`"));
        assert_eq!(
            failed(read_answered_by(DELETE, &args, http(404, ""))),
            missing
        );
        assert_eq!(
            failed(delete_answered(&args, STORED_EVENT, http(404, ""))),
            missing
        );
        assert_eq!(
            failed(delete_answered(&args, STORED_EVENT, http(412, ""))),
            internal(format!("{CHANGED_SINCE_READ} (HTTP 412)"))
        );
        for outcome in [http(401, ""), http(403, ""), http(429, "")] {
            let refused = failed(delete_answered(&args, STORED_EVENT, outcome));
            assert!(!refused.detail.contains("may already"), "{refused:?}");
        }
        for outcome in [
            || http(500, "<html>secret</html>"),
            || nested("internal", "api-request could not be issued"),
            || written(202, None),
        ] {
            let lost = failed(delete_answered(&args, STORED_EVENT, outcome()));
            assert!(
                lost.detail.ends_with(&format!("; {DELETE_UNKNOWN_HINT}")),
                "{lost:?}"
            );
            assert!(!lost.detail.contains("secret"), "{lost:?}");
            let read = failed(read_answered_by(DELETE, &args, outcome()));
            assert!(!read.detail.contains(DELETE_UNKNOWN_HINT), "{read:?}");
        }
        assert_eq!(write_hint(DELETE, READ_STEP), None);
        assert_eq!(write_hint(DELETE, WRITE_STEP), Some(DELETE_UNKNOWN_HINT));
        let refused = |args: Value| plan(DELETE, &args, &context()).unwrap_err();
        assert_eq!(
            refused(json!({ "event": EVENT, "series": "yes" })),
            "`series` must be true or false"
        );
        assert!(
            refused(json!({ "event": EVENT, "summary": "a" }))
                .starts_with("unknown argument `summary`")
        );
    }

    #[test]
    fn a_delete_step_it_does_not_have_or_a_row_without_its_answer_fails() {
        let args = delete(None);
        let (call, derived) = delete_after(&args, STORED_EVENT);
        let mut state = row_at(DELETE, &args, &call, derived.clone());
        state.step = WRITE_STEP + 1;
        assert_eq!(
            advance(&state, written(204, None), &context()),
            Next::Fail(no_such_step(DELETE, WRITE_STEP + 1))
        );
        for key in [START_KEY, SERIES_KEY] {
            let mut partial = derived.clone();
            partial.remove(key);
            let state = row_at(DELETE, &args, &call, partial);
            assert_eq!(
                failed(advance(&state, written(204, None), &context())),
                internal(format!(
                    "in-flight row at fastmail-calendar-event-delete step {WRITE_STEP} carries no \
                     `{key}`; {DELETE_UNKNOWN_HINT}"
                ))
            );
        }
    }

    #[test]
    fn a_delete_runs_a_get_then_a_delete_then_completes() {
        let host = MemoryHost::new();
        let id = "d".repeat(32);
        let activate = |requests, results| {
            host.activate::<Fastmail>(
                &context(),
                Input {
                    now_ms: NOW,
                    requests,
                    results,
                },
            )
            .unwrap();
        };
        activate(
            vec![incoming(
                &id,
                "fastmail-calendar-event-delete",
                delete(None),
            )],
            vec![],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.method, "GET");
        activate(
            vec![],
            vec![Delivered {
                call_id: calls[0].call_id.clone(),
                serving: Some(id.clone()),
                outcome: ics_tagged(STORED_EVENT, Some(TAG)),
            }],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.method, "DELETE");
        activate(
            vec![],
            vec![Delivered {
                call_id: calls[0].call_id.clone(),
                serving: Some(id.clone()),
                outcome: written(204, None),
            }],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(value["deleted"], json!(true));
        assert_eq!(value["summary"], json!("Old"));
    }
}
