//! The provider's decisions, in `std`, `serde_json`, the DAV crate and
//! `brenn-provider-kit`: validating a tool's arguments, building each CalDAV
//! request as `api-request` arguments, reading `api-request`'s answer, and
//! compacting Fastmail's multi-status and iCalendar into the tool's result.
//! Nothing here names the guest SDK, so the whole module runs on the host
//! under test. Tools are the generated `Provided` enum, whose SDK-facing
//! parts are wasm32-only.
//!
//! The calendar list asks who the credential is (step 1), where its
//! calendars are (step 2) and what they are (step 3), each step's path read
//! from the answer before it, nothing carried between them. The window and
//! the single event are one call each, to a path an earlier answer handed
//! the caller.

// The host test's crate root is this file, so it declares the generated
// module and its sibling itself; in the component, `lib.rs` does.
#[cfg(test)]
mod event;
#[cfg(test)]
mod spec;

use brenn_provider_kit::{
    Args, COMPACT_RESULT_CEILING, Call, Failure, InFlight as KitInFlight, Logic, NestedError, Next,
    Payload, Tool, bounded, integer, internal, no_such_step, object,
};
use fastmail_dav::caldav::{self, CALENDARS_PROPFIND, HOME_PROPFIND, PRINCIPAL_PROPFIND};
use fastmail_dav::{Error as DavError, href, ical};
use fastmail_provider::args::{ACCOUNT_ARG, account_arg, utc_date_arg};
use fastmail_provider::dav::{QUOTED_PATH_MAX, asked, dav_call, payload};
use fastmail_provider::failure::stored_args;
use fastmail_provider::results::{cut_value, cut_width};
use fastmail_provider::settings::{API_NAME, Settings};
use serde_json::{Value, json};

use crate::event::{BRIEF_TEXT_MAX, events, full, malformed, occurrence, start};
use crate::spec::Provided;

/// Where discovery starts: the collection that answers which principal the
/// credential is.
const PRINCIPALS_PATH: &str = "/dav/principals/";

/// The calendar list's steps.
const PRINCIPAL_STEP: u8 = 1;
const HOME_STEP: u8 = 2;
const CALENDARS_STEP: u8 = 3;
/// The one step of the window and the single event.
const ONLY_STEP: u8 = 1;

/// A DAV multi-status answer's status; a `GET`'s.
const MULTI_STATUS: u64 = 207;
const OK: u64 = 200;

/// The most calendars listed.
const CALENDARS_MAX: usize = 100;
/// The widest calendar `name` and `description`.
const CALENDAR_NAME_MAX: usize = 400;
const CALENDAR_DESCRIPTION_MAX: usize = 1000;

/// A window's page size: its range and default.
const LIMIT_MAX: u64 = 100;
const LIMIT_DEFAULT: u64 = 50;

/// What to narrow when a window's answer is too large.
const WINDOW_HINT: &str = "narrow the window with `from` and `to`";

/// Why a window answer carrying `RRULE` or `RDATE` is refused.
const UNEXPANDED: &str = "Fastmail answered a recurring event unexpanded; this tool relies on the \
     server to expand recurrences (RFC 4791 section 9.6.5)";

impl Tool for Provided {
    fn name(self) -> &'static str {
        Provided::name(self)
    }

    fn from_name(name: &str) -> Option<Provided> {
        Provided::from_name(name)
    }
}

/// A request's in-flight row, naming one of this component's tools.
pub type InFlight = KitInFlight<Provided>;

/// This component's step functions, for the kit's activation loop.
pub struct Fastmail;

impl Logic for Fastmail {
    type Tool = Provided;
    type Settings = Settings;

    fn plan(tool: Provided, args: &Value, settings: &Settings) -> Result<Call, String> {
        plan(tool, args, settings)
    }

    fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
        advance(state, outcome, settings)
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
    Calendars,
    Events(EventsArgs),
    Event { event: String },
}

#[derive(Clone, Debug, PartialEq)]
struct EventsArgs {
    calendar: String,
    /// The window's bounds as the caller wrote them, and as UTC instants.
    from: String,
    to: String,
    start: String,
    end: String,
    limit: u64,
}

const EVENTS_KEYS: &[&str] = &["calendar", "from", "to", "limit", ACCOUNT_ARG];
const EVENT_KEYS: &[&str] = &["event", ACCOUNT_ARG];

/// The path argument at `key`: a calendar's when `collection`, ending in
/// `/`, else an event's, not ending in `/`; held to the rule `api-request`
/// applies to a request path.
fn path_arg(map: &Args, key: &str, collection: bool) -> Result<String, String> {
    let rule = if collection {
        "a `calendar` path from `FastmailCalendars`, beginning and ending in `/`"
    } else {
        "an `event` path from `FastmailCalendarEvents`, beginning with `/` and not ending in `/`"
    };
    let Some(value) = map.get(key) else {
        return Err(format!("`{key}` is required"));
    };
    match value {
        Value::String(path) if path.starts_with('/') && href::is_collection(path) == collection => {
            // A path that begins with `/` is read without the host.
            href::path(path, "").map_err(|error| format!("`{key}` must be {rule}: {error}"))
        }
        _ => Err(format!("`{key}` must be {rule}")),
    }
}

// TODO(calendar-window-one-calendar-per-call): a window reads one calendar;
// several in one call need rules for a partial read, `limit` and order.
/// The window tool's arguments: one calendar, `from` before `to`.
fn parse_events(map: &Args) -> Result<EventsArgs, String> {
    let bound = |key: &str| -> Result<(String, String), String> {
        let start = utc_date_arg(map, key)?.ok_or_else(|| format!("`{key}` is required"))?;
        let written = map[key].as_str().expect("a date argument is a string");
        Ok((written.to_owned(), start))
    };
    let calendar = path_arg(map, "calendar", true)?;
    let (from, start) = bound("from")?;
    let (to, end) = bound("to")?;
    if end <= start {
        return Err("`to` must be later than `from`".to_owned());
    }
    Ok(EventsArgs {
        calendar,
        from,
        to,
        start,
        end,
        limit: integer(map, "limit", 1, LIMIT_MAX)?.unwrap_or(LIMIT_DEFAULT),
    })
}

fn parse(tool: Provided, args: &Value) -> Result<Parsed, String> {
    let keys = match tool {
        Provided::FastmailCalendars => &[ACCOUNT_ARG],
        Provided::FastmailCalendarEvents => EVENTS_KEYS,
        Provided::FastmailCalendarEvent => EVENT_KEYS,
    };
    let map = object(args, keys)?;
    let tool = match tool {
        Provided::FastmailCalendars => ToolArgs::Calendars,
        Provided::FastmailCalendarEvents => ToolArgs::Events(parse_events(map)?),
        Provided::FastmailCalendarEvent => ToolArgs::Event {
            event: path_arg(map, "event", false)?,
        },
    };
    Ok(Parsed {
        account: account_arg(map)?,
        tool,
    })
}

/// A UTC instant, `2026-11-02T09:00:00Z`, as CalDAV writes it,
/// `20261102T090000Z`.
fn compact(instant: &str) -> String {
    instant
        .chars()
        .filter(|c| !matches!(c, '-' | ':'))
        .collect()
}

/// A `PROPFIND` of `path` at `depth` asking `body`, as step `step`.
fn propfind(
    settings: &Settings,
    account: Option<&str>,
    step: u8,
    path: &str,
    depth: &str,
    body: &str,
) -> Call {
    Call {
        step,
        api: dav_call(
            settings,
            account,
            "PROPFIND",
            path,
            Some(depth),
            Some(body.to_owned()),
            None,
        ),
    }
}

/// Validate a request's arguments and issue its first call. An error is the
/// request's `invalid_args` detail, naming the argument.
pub fn plan(tool: Provided, args: &Value, settings: &Settings) -> Result<Call, String> {
    let parsed = parse(tool, args)?;
    let account = parsed.account.as_deref();
    Ok(match &parsed.tool {
        ToolArgs::Calendars => propfind(
            settings,
            account,
            PRINCIPAL_STEP,
            PRINCIPALS_PATH,
            "0",
            PRINCIPAL_PROPFIND,
        ),
        ToolArgs::Events(args) => Call {
            step: ONLY_STEP,
            api: dav_call(
                settings,
                account,
                "REPORT",
                &args.calendar,
                Some("1"),
                Some(caldav::events_report(
                    &compact(&args.start),
                    &compact(&args.end),
                )),
                None,
            ),
        },
        ToolArgs::Event { event } => Call {
            step: ONLY_STEP,
            api: dav_call(
                settings,
                account,
                "GET",
                event,
                None,
                None,
                Some("text/calendar"),
            ),
        },
    })
}

/// What the caller narrows when `tool`'s answer is too large: only the
/// window has arguments that do.
fn hint(tool: Provided) -> Option<&'static str> {
    match tool {
        Provided::FastmailCalendarEvents => Some(WINDOW_HINT),
        Provided::FastmailCalendars | Provided::FastmailCalendarEvent => None,
    }
}

/// What the request does with its nested call's answer: complete, fail, or
/// issue its next step. Every completion is held to the result ceiling.
pub fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
    let last = match state.tool {
        Provided::FastmailCalendars => CALENDARS_STEP,
        Provided::FastmailCalendarEvents | Provided::FastmailCalendarEvent => ONLY_STEP,
    };
    if !(1..=last).contains(&state.step) {
        return Next::Fail(no_such_step(state.tool, state.step));
    }
    match step(state, outcome, settings) {
        Ok(Next::Complete(value)) => bounded(value, COMPACT_RESULT_CEILING, hint(state.tool)),
        Ok(next) => next,
        Err(failure) => Next::Fail(failure),
    }
}

/// The payload's body as text; a body that is not UTF-8 is internal.
fn text<'a>(state: &InFlight, payload: &'a Payload) -> Result<&'a str, Failure> {
    payload.text().ok_or_else(|| {
        internal(format!(
            "{API_NAME} answered {} with a body that is not UTF-8",
            asked(state)
        ))
    })
}

/// A multi-status answer to `state`'s call that cannot be read.
fn unreadable(state: &InFlight, error: DavError) -> Failure {
    internal(format!("{API_NAME} {}: {error}", asked(state)))
}

/// A discovered `href`, the `what` of the answer, as the collection path the
/// next step asks; one that is not a collection brenn will send is internal.
fn discovered(href: &str, what: &str, host: &str) -> Result<String, Failure> {
    let path = href::path(href, host)
        .map_err(|error| internal(format!("{API_NAME}'s {what}: {error}")))?;
    if !href::is_collection(&path) {
        return Err(internal(format!(
            "{API_NAME}'s {what} `{}` is not a collection",
            cut_width(&path, QUOTED_PATH_MAX).0
        )));
    }
    Ok(path)
}

/// `href` as a handle an agent passes back: the path brenn will send, a
/// collection or not as `collection` says, no wider than a path may be once
/// serialized; `None` when it is not.
fn handle(href: &str, host: &str, collection: bool) -> Option<String> {
    let path = href::path(href, host).ok()?;
    let fits = !cut_width(&path, href::MAX_PATH).1;
    (fits && href::is_collection(&path) == collection).then_some(path)
}

/// `out` with `key` set to `href`'s handle, or `unreachable: true` in its
/// place.
fn with_handle(mut out: Args, key: &str, handle: Option<String>) -> Value {
    match handle {
        Some(path) => out.insert(key.to_owned(), json!(path)),
        None => out.insert("unreachable".to_owned(), json!(true)),
    };
    Value::Object(out)
}

fn step(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    settings: &Settings,
) -> Result<Next, Failure> {
    let parsed = stored_args(state, parse)?;
    let account = parsed.account.as_deref();
    let host = settings.host.as_str();
    match &parsed.tool {
        ToolArgs::Calendars => {
            let answer = payload(state, outcome, MULTI_STATUS, None, None)?;
            let body = text(state, &answer)?;
            let read = |error| unreadable(state, error);
            Ok(match state.step {
                PRINCIPAL_STEP => {
                    let href = caldav::principal(body).map_err(read)?;
                    let path = discovered(&href, "current-user-principal", host)?;
                    let call = propfind(settings, account, HOME_STEP, &path, "0", HOME_PROPFIND);
                    Next::Call {
                        call,
                        derived: Args::new(),
                    }
                }
                HOME_STEP => {
                    let href = caldav::calendar_home(body).map_err(read)?;
                    let path = discovered(&href, "calendar-home-set", host)?;
                    let call = propfind(
                        settings,
                        account,
                        CALENDARS_STEP,
                        &path,
                        "1",
                        CALENDARS_PROPFIND,
                    );
                    Next::Call {
                        call,
                        derived: Args::new(),
                    }
                }
                _ => calendars(caldav::calendars(body).map_err(read)?, host),
            })
        }
        ToolArgs::Events(args) => {
            let missing = format!("calendar at `{}`", args.calendar);
            let answer = payload(
                state,
                outcome,
                MULTI_STATUS,
                Some(&missing),
                Some(WINDOW_HINT),
            )?;
            let body = text(state, &answer)?;
            let resources = caldav::event_resources(body).map_err(|e| unreadable(state, e))?;
            window(args, resources, host)
        }
        ToolArgs::Event { event } => {
            let missing = format!("event at `{event}`");
            let answer = payload(state, outcome, OK, Some(&missing), None)?;
            let calendar = ical::parse(&answer.body).map_err(|error| malformed(event, error))?;
            full(event, &calendar).map(Next::Complete)
        }
    }
}

fn calendars(mut list: Vec<caldav::Calendar>, host: &str) -> Next {
    list.sort_by(|a, b| {
        (a.name.is_none(), &a.name, &a.href).cmp(&(b.name.is_none(), &b.name, &b.href))
    });
    let shown: Vec<Value> = list
        .iter()
        .take(CALENDARS_MAX)
        .map(|calendar| {
            let mut out = Args::new();
            if let Some(name) = &calendar.name {
                out.insert("name".into(), cut_value(name, CALENDAR_NAME_MAX));
            }
            if let Some(description) = &calendar.description {
                out.insert(
                    "description".into(),
                    cut_value(description, CALENDAR_DESCRIPTION_MAX),
                );
            }
            with_handle(out, "calendar", handle(&calendar.href, host, true))
        })
        .collect();
    let mut out = Args::new();
    out.insert("calendars".into(), Value::Array(shown));
    if list.len() > CALENDARS_MAX {
        out.insert(
            "calendars_omitted".into(),
            json!(list.len() - CALENDARS_MAX),
        );
    }
    Next::Complete(Value::Object(out))
}

/// The window's answer: every event of every resource one occurrence, the
/// first `limit` by the start as written, then path.
fn window(
    args: &EventsArgs,
    resources: Vec<caldav::Resource>,
    host: &str,
) -> Result<Next, Failure> {
    let mut found: Vec<(String, &str, Value)> = Vec::new();
    for resource in &resources {
        let calendar = ical::parse(resource.data.as_bytes())
            .map_err(|error| malformed(&resource.href, error))?;
        let event = handle(&resource.href, host, false);
        for vevent in events(&calendar) {
            if ["RRULE", "RDATE"]
                .iter()
                .any(|name| vevent.property(name).is_some())
            {
                return Err(internal(UNEXPANDED));
            }
            let out = occurrence(vevent, &resource.href, BRIEF_TEXT_MAX)?;
            let start = start(vevent, &resource.href)?.value.clone();
            let shown = with_handle(out, "event", event.clone());
            found.push((start, &resource.href, shown));
        }
    }
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let total = found.len();
    let limit = usize::try_from(args.limit).expect("a limit of at most 100 fits");
    let events: Vec<Value> = found
        .into_iter()
        .take(limit)
        .map(|(_, _, shown)| shown)
        .collect();
    let mut out = Args::new();
    out.insert("from".into(), json!(args.from));
    out.insert("to".into(), json!(args.to));
    out.insert("total".into(), json!(total));
    out.insert("events".into(), Value::Array(events));
    if total > limit {
        out.insert("events_omitted".into(), json!(total - limit));
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
        ApiCall, Bound, CALL_BUDGET, Delivered, ErrorKind, Input, Probes, ToolBounds, Unreadable,
        check, hold_probes, hold_required, input_schema, probes,
    };
    use fastmail_provider::dav::XML_CONTENT_TYPE;
    use fastmail_provider::testutil::{
        ACCOUNT_PATTERN, DATE_PATTERN, account_probes, completed, date_probes, ics, incoming,
        multistatus, settings,
    };
    use serde_json::Map;

    use super::*;

    const CALENDAR: &str = "/dav/calendars/user/alice@example.com/work/";
    const EVENT: &str = "/dav/calendars/user/alice@example.com/work/e1.ics";
    const PRINCIPAL: &str = "/dav/principals/user/alice@example.com/";
    const HOME: &str = "/dav/calendars/user/alice@example.com/";
    const CALENDAR_PATTERN: &str = "^/([^/?#]+/)*$";
    const EVENT_PATTERN: &str = "^(/[^/?#]+)+$";

    fn window_args() -> Value {
        json!({ "calendar": CALENDAR, "from": "2026-11-02", "to": "2026-11-16T00:00:00Z" })
    }

    fn admitted_args(tool: Provided) -> Value {
        match tool {
            Provided::FastmailCalendars => json!({}),
            Provided::FastmailCalendarEvents => {
                json!({ "calendar": CALENDAR, "from": "0000-01-01", "to": "9999-12-31" })
            }
            Provided::FastmailCalendarEvent => json!({ "event": EVENT }),
        }
    }

    fn state_for(tool: Provided, args: &Value, call: &Call) -> InFlight {
        InFlight::new(&"r".repeat(32), tool, args, call, Map::new(), 1_000, 1_000)
    }

    /// `tool` planned with `args`, each call answered in turn from
    /// `answers`: the calls issued and what the last answer led to.
    fn run(
        tool: Provided,
        args: Value,
        answers: Vec<Result<Value, NestedError>>,
    ) -> (Vec<ApiCall>, Next) {
        let mut call = plan(tool, &args, &settings()).unwrap();
        let mut calls = vec![call.api.clone()];
        for answer in answers {
            match advance(&state_for(tool, &args, &call), answer, &settings()) {
                Next::Call {
                    call: next,
                    derived,
                } => {
                    assert!(derived.is_empty(), "{derived:?}");
                    calls.push(next.api.clone());
                    call = next;
                }
                done => return (calls, done),
            }
        }
        panic!("{tool:?}: the answers ran out at {calls:?}");
    }

    fn ms(responses: &str) -> Result<Value, NestedError> {
        multistatus(&format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">{responses}</D:multistatus>"#
        ))
    }

    fn response(href: &str, props: &str) -> String {
        format!(
            "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{props}</D:prop>\
             <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
        )
    }

    fn principal_answer(href: &str) -> Result<Value, NestedError> {
        ms(&response(
            PRINCIPALS_PATH,
            &format!(
                "<D:current-user-principal><D:href>{href}</D:href></D:current-user-principal>"
            ),
        ))
    }

    fn home_answer(hrefs: &[&str]) -> Result<Value, NestedError> {
        let hrefs: String = hrefs
            .iter()
            .map(|h| format!("<D:href>{h}</D:href>"))
            .collect();
        ms(&response(
            PRINCIPAL,
            &format!("<C:calendar-home-set>{hrefs}</C:calendar-home-set>"),
        ))
    }

    /// A calendar of the home's listing, named when `name` is not empty.
    fn calendar_entry(href: &str, name: &str, description: &str) -> String {
        let name = match name {
            "" => String::new(),
            name => format!("<D:displayname>{name}</D:displayname>"),
        };
        let description = match description {
            "" => String::new(),
            text => format!("<C:calendar-description>{text}</C:calendar-description>"),
        };
        response(
            href,
            &format!(
                "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>{name}{description}"
            ),
        )
    }

    /// A `VCALENDAR` holding one `VEVENT` per entry of `events`, each the
    /// content lines given, CRLF-terminated.
    fn vcal(events: &[&[&str]]) -> String {
        let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\n");
        for lines in events {
            text.push_str("BEGIN:VEVENT\r\n");
            for line in *lines {
                text.push_str(line);
                text.push_str("\r\n");
            }
            text.push_str("END:VEVENT\r\n");
        }
        text.push_str("END:VCALENDAR\r\n");
        text
    }

    fn data(href: &str, ical: &str) -> String {
        response(
            href,
            &format!("<C:calendar-data><![CDATA[{ical}]]></C:calendar-data>"),
        )
    }

    fn window_of(args: Value, responses: &str) -> Next {
        run(Provided::FastmailCalendarEvents, args, vec![ms(responses)]).1
    }

    fn event_of(ical: &str) -> Next {
        run(
            Provided::FastmailCalendarEvent,
            json!({ "event": EVENT }),
            vec![ics(ical)],
        )
        .1
    }

    fn path_probes(collection: bool) -> Probes {
        let (admitted, refused) = if collection {
            (
                vec![CALENDAR, "/", "/a%40b/"],
                vec![
                    "dav/x/", "/a/b", "/a//", "/a/../", "/a%2Fb/", "/a?b/", "/a#b/", "",
                ],
            )
        } else {
            (
                vec![EVENT, "/e.ics"],
                vec![
                    "e.ics", "/a/", "/", "/a/./e", "/a%5Cb", "/a%25b", "/e?x", "",
                ],
            )
        };
        let long = format!("/{}/", "x".repeat(href::MAX_PATH - 1));
        Probes {
            admitted: admitted.into_iter().map(|p| json!(p)).collect(),
            refused: refused
                .into_iter()
                .map(|p| json!(p))
                .chain([json!(long), json!(1)])
                .collect(),
        }
    }

    fn bounds(tool: Provided) -> ToolBounds<'static> {
        let (keys, required): (&'static [&'static str], &'static [&'static str]) = match tool {
            Provided::FastmailCalendars => (&[ACCOUNT_ARG], &[]),
            Provided::FastmailCalendarEvents => (EVENTS_KEYS, &["calendar", "from", "to"]),
            Provided::FastmailCalendarEvent => (EVENT_KEYS, &["event"]),
        };
        let property = |key: &'static str| {
            let bound = match key {
                "calendar" => Bound::Pattern(CALENDAR_PATTERN.to_owned()),
                "event" => Bound::Pattern(EVENT_PATTERN.to_owned()),
                "from" | "to" => Bound::Pattern(DATE_PATTERN.to_owned()),
                ACCOUNT_ARG => Bound::Pattern(ACCOUNT_PATTERN.to_owned()),
                "limit" => Bound::Integer {
                    min: 1,
                    max: LIMIT_MAX,
                },
                _ => panic!("{tool:?} takes `{key}`, which no bound here states"),
            };
            (key, bound)
        };
        ToolBounds {
            properties: keys.iter().copied().map(property).collect(),
            required,
        }
    }

    #[test]
    fn every_tools_schema_states_what_its_parser_enforces() {
        let path = std::env::var("FASTMAIL_CALENDAR_SPEC").expect("FASTMAIL_CALENDAR_SPEC");
        let spec = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        for tool in Provided::ALL {
            let name = tool.name();
            let bounds = bounds(tool);
            if let Err(error) = check(&input_schema(&spec, name), &bounds) {
                panic!("`{name}`'s input_schema: {error}");
            }
            let plan_with = |args: &Value| plan(tool, args, &settings()).map(drop);
            let base = admitted_args(tool);
            for key in bounds.required {
                hold_required(name, key, &base, plan_with);
            }
            for (key, bound) in &bounds.properties {
                let probes = match *key {
                    ACCOUNT_ARG => account_probes(),
                    "from" | "to" => date_probes(),
                    "calendar" => path_probes(true),
                    "event" => path_probes(false),
                    _ => {
                        probes(bound).unwrap_or_else(|| panic!("`{name}`'s `{key}` has no probes"))
                    }
                };
                hold_probes(name, key, &probes, &base, plan_with);
            }
        }
    }

    #[test]
    fn each_tools_first_call_names_its_method_path_headers_and_account() {
        let header = |call: &ApiCall, name: &str| call.headers.get(name).cloned();
        for account in [Some("alice-work"), None] {
            for tool in Provided::ALL {
                let mut args = match tool {
                    Provided::FastmailCalendarEvents => window_args(),
                    _ => admitted_args(tool),
                };
                if let Some(account) = account {
                    args["account"] = json!(account);
                }
                let call = plan(tool, &args, &settings()).unwrap();
                assert_eq!(call.step, 1);
                let api = &call.api;
                assert_eq!(api.account.as_deref(), account);
                assert_eq!(
                    (api.provider.as_str(), api.host.as_str()),
                    ("fastmail", "api.fastmail.example.com")
                );
                let (method, path, depth, body) = match tool {
                    Provided::FastmailCalendars => (
                        "PROPFIND",
                        PRINCIPALS_PATH,
                        Some("0"),
                        Some(PRINCIPAL_PROPFIND.to_owned()),
                    ),
                    Provided::FastmailCalendarEvents => (
                        "REPORT",
                        CALENDAR,
                        Some("1"),
                        Some(caldav::events_report(
                            "20261102T000000Z",
                            "20261116T000000Z",
                        )),
                    ),
                    Provided::FastmailCalendarEvent => ("GET", EVENT, None, None),
                };
                assert_eq!((api.method.as_str(), api.path.as_str()), (method, path));
                assert_eq!(header(api, "Depth").as_deref(), depth, "{tool:?}");
                match body {
                    Some(body) => {
                        assert_eq!(text_body(api), body);
                        assert_eq!(
                            header(api, "Content-Type").as_deref(),
                            Some(XML_CONTENT_TYPE)
                        );
                        assert_eq!(header(api, "Accept"), None);
                    }
                    None => {
                        assert_eq!(api.body, None);
                        assert_eq!(header(api, "Accept").as_deref(), Some("text/calendar"));
                    }
                }
            }
        }
        let instant = json!({ "calendar": CALENDAR, "from": "2026-11-02T09:30:00Z",
                              "to": "2026-11-02T10:00:00Z" });
        let call = plan(Provided::FastmailCalendarEvents, &instant, &settings()).unwrap();
        assert_eq!(
            text_body(&call.api),
            caldav::events_report("20261102T093000Z", "20261102T100000Z")
        );
        let refused = |args: Value| plan(Provided::FastmailCalendarEvents, &args, &settings());
        for to in ["2026-11-02", "2026-11-01T23:59:59Z"] {
            let args = json!({ "calendar": CALENDAR, "from": "2026-11-02", "to": to });
            assert_eq!(refused(args).unwrap_err(), "`to` must be later than `from`");
        }
        let args = json!({ "calendar": "/a%2Fb/", "from": "2026-11-02", "to": "2026-11-03" });
        assert_eq!(
            refused(args).unwrap_err(),
            "`calendar` must be a `calendar` path from `FastmailCalendars`, beginning and \
             ending in `/`: href `/a%2Fb/` has a segment that decodes to hold `/`, `\\` or `%`"
        );
    }

    #[test]
    fn the_calendar_list_discovers_the_home_and_lists_its_calendars() {
        let listing = ms(&[
            response(HOME, "<D:resourcetype><D:collection/></D:resourcetype>"),
            calendar_entry(&format!("{HOME}work/"), "Work", "Office hours"),
            calendar_entry(
                &format!("https://API.fastmail.example.com{HOME}home/"),
                "Home",
                "",
            ),
            calendar_entry(&format!("{HOME}zz/"), "", ""),
            calendar_entry("https://other.example.com/dav/x/", "Elsewhere", ""),
            calendar_entry(&format!("{HOME}a%2Fb/"), "Bad", ""),
            response(
                &format!("{HOME}tasks/"),
                "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                 <C:supported-calendar-component-set><C:comp name=\"VTODO\"/>\
                 </C:supported-calendar-component-set>",
            ),
        ]
        .concat());
        let (calls, done) = run(
            Provided::FastmailCalendars,
            json!({ "account": "alice" }),
            vec![
                principal_answer(PRINCIPAL),
                home_answer(&[&format!("https://api.fastmail.example.com{HOME}")]),
                listing,
            ],
        );
        let asked: Vec<(&str, &str, Option<&str>, &str)> = calls
            .iter()
            .map(|c| {
                (
                    c.method.as_str(),
                    c.path.as_str(),
                    c.headers.get("Depth").map(String::as_str),
                    text_body(c),
                )
            })
            .collect();
        assert_eq!(
            asked,
            [
                ("PROPFIND", PRINCIPALS_PATH, Some("0"), PRINCIPAL_PROPFIND),
                ("PROPFIND", PRINCIPAL, Some("0"), HOME_PROPFIND),
                ("PROPFIND", HOME, Some("1"), CALENDARS_PROPFIND),
            ]
        );
        assert!(calls.iter().all(|c| c.account.as_deref() == Some("alice")));
        assert_eq!(
            completed(done),
            json!({ "calendars": [
                { "name": "Bad", "unreachable": true },
                { "name": "Elsewhere", "unreachable": true },
                { "name": "Home", "calendar": format!("{HOME}home/") },
                { "name": "Work", "description": "Office hours", "calendar": format!("{HOME}work/") },
                { "calendar": format!("{HOME}zz/") },
            ] })
        );
    }

    #[test]
    fn discovery_fails_on_an_answer_it_cannot_follow() {
        let fails = |answers: Vec<Result<Value, NestedError>>| -> Failure {
            match run(Provided::FastmailCalendars, json!({}), answers).1 {
                Next::Fail(failure) => failure,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            fails(vec![principal_answer("https://other.example.com/p/")]),
            internal(
                "Fastmail's current-user-principal: href `https://other.example.com/p/` is \
                 neither a path nor a URL on the host"
            )
        );
        assert_eq!(
            fails(vec![principal_answer("/dav/principals/user/alice")]),
            internal(
                "Fastmail's current-user-principal `/dav/principals/user/alice` is not a \
                 collection"
            )
        );
        assert_eq!(
            fails(vec![
                principal_answer(PRINCIPAL),
                home_answer(&[HOME, "/other/"])
            ]),
            internal(format!(
                "Fastmail PROPFIND `{PRINCIPAL}`: the answer's calendar-home-set {}",
                match caldav::calendar_home(
                    "<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
                     <D:response><D:href>/p/</D:href><D:propstat><D:prop><C:calendar-home-set>\
                     <D:href>/a/</D:href><D:href>/b/</D:href></C:calendar-home-set></D:prop>\
                     <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>\
                     </D:multistatus>"
                ) {
                    Err(DavError::Property { problem, .. }) => problem,
                    other => panic!("{other:?}"),
                }
            ))
        );
        let not_xml = fails(vec![multistatus("not xml")]);
        assert_eq!(not_xml.kind, ErrorKind::Internal);
        assert!(
            not_xml.detail.starts_with(
                "Fastmail PROPFIND `/dav/principals/`: the answer is not a multi-status"
            ),
            "{}",
            not_xml.detail
        );
        assert_eq!(
            fails(vec![http(404, "")]),
            internal("Fastmail answered HTTP 404 to PROPFIND `/dav/principals/`")
        );
        assert_eq!(
            fails(vec![http(401, "")]).detail,
            "Fastmail refused the app password for this account (HTTP 401): the operator \
             must replace its key file and restart Brenn"
        );
        assert_eq!(
            fails(vec![
                principal_answer(PRINCIPAL),
                nested("denied", "no clause")
            ])
            .kind,
            ErrorKind::Denied
        );
        assert_eq!(
            fails(vec![ics("BEGIN:VCALENDAR")]),
            internal(
                "Fastmail answered HTTP 200 to PROPFIND `/dav/principals/`, where 207 was expected"
            )
        );
    }

    #[test]
    fn a_window_lists_each_occurrence_by_start_then_path() {
        let weekly = |day: &str| {
            vcal(&[&[
                "UID:w1",
                "SUMMARY:Stand-up\\, daily",
                &format!("DTSTART:202611{day}T090000Z"),
                "DURATION:PT15M",
                &format!("RECURRENCE-ID:202611{day}T090000Z"),
            ]])
        };
        let responses = [
            data(&format!("{CALENDAR}w1.ics"), &weekly("09")),
            data(
                &format!("{CALENDAR}a.ics"),
                &vcal(&[&[
                    "UID:a",
                    "SUMMARY:Holiday",
                    "DTSTART;VALUE=DATE:20261103",
                    "DTEND;VALUE=DATE:20261104",
                    "STATUS:CONFIRMED",
                ]]),
            ),
            data(
                &format!("{CALENDAR}f.ics"),
                &vcal(&[&[
                    "UID:f",
                    "DTSTART:20261102T080000",
                    "DTEND:20261102T083000",
                    "LOCATION:Kitchen",
                ]]),
            ),
            data(
                &format!("{CALENDAR}z.ics"),
                &vcal(&[&["UID:z", "DTSTART;TZID=Europe/Berlin:20261110T100000"]]),
            ),
            data("https://other.example.com/x.ics", &weekly("02")),
        ]
        .concat();
        let result = completed(window_of(window_args(), &responses));
        assert_eq!(
            result,
            json!({ "from": "2026-11-02", "to": "2026-11-16T00:00:00Z", "total": 5, "events": [
                { "event": format!("{CALENDAR}f.ics"), "location": "Kitchen",
                  "start": { "date_time": "2026-11-02T08:00:00" },
                  "end": { "date_time": "2026-11-02T08:30:00" } },
                { "unreachable": true, "summary": "Stand-up, daily",
                  "start": { "date_time": "2026-11-02T09:00:00Z" }, "duration": "PT15M",
                  "recurrence_id": { "date_time": "2026-11-02T09:00:00Z" } },
                { "event": format!("{CALENDAR}a.ics"), "summary": "Holiday",
                  "start": { "date": "2026-11-03" }, "end": { "date": "2026-11-04" },
                  "status": "CONFIRMED" },
                { "event": format!("{CALENDAR}w1.ics"), "summary": "Stand-up, daily",
                  "start": { "date_time": "2026-11-09T09:00:00Z" }, "duration": "PT15M",
                  "recurrence_id": { "date_time": "2026-11-09T09:00:00Z" } },
                { "event": format!("{CALENDAR}z.ics"),
                  "start": { "date_time": "2026-11-10T10:00:00", "time_zone": "Europe/Berlin" } },
            ] })
        );
        let mut limited = window_args();
        limited["limit"] = json!(2);
        let result = completed(window_of(limited, &responses));
        assert_eq!(result["events"].as_array().unwrap().len(), 2);
        assert_eq!(
            (result["total"].clone(), result["events_omitted"].clone()),
            (json!(5), json!(3))
        );
        assert_eq!(
            completed(window_of(window_args(), "")),
            json!({ "from": "2026-11-02", "to": "2026-11-16T00:00:00Z", "total": 0, "events": [] })
        );
    }

    #[test]
    fn a_window_refuses_what_it_cannot_show_as_the_server_meant() {
        let fails = |responses: &str| match window_of(window_args(), responses) {
            Next::Fail(failure) => failure,
            other => panic!("{other:?}"),
        };
        let unexpanded = data(
            EVENT,
            &vcal(&[&["UID:w", "DTSTART:20261102T090000Z", "RRULE:FREQ=WEEKLY"]]),
        );
        assert_eq!(fails(&unexpanded), internal(UNEXPANDED));
        let dated = data(
            EVENT,
            &vcal(&[&[
                "UID:w",
                "DTSTART:20261026T090000Z",
                "RDATE:20261102T090000Z",
            ]]),
        );
        assert_eq!(fails(&dated), internal(UNEXPANDED));
        assert_eq!(
            fails(&data(EVENT, &vcal(&[&["UID:x", "SUMMARY:secret"]]))),
            internal(format!(
                "Fastmail's calendar data at `{EVENT}`: an event has no DTSTART"
            ))
        );
        assert_eq!(
            fails(&data(EVENT, &vcal(&[&["DTSTART:2026-11-02"]]))),
            internal(format!(
                "Fastmail's calendar data at `{EVENT}`: the DTSTART value is neither a date nor \
                 a date-time"
            ))
        );
        let broken = fails(&data(
            EVENT,
            "BEGIN:VCALENDAR\nSUMMARY secret\nEND:VCALENDAR",
        ));
        assert!(
            broken.detail.starts_with(&format!(
                "Fastmail's calendar data at `{EVENT}`: the calendar data's line 2"
            )),
            "{}",
            broken.detail
        );
        assert!(!broken.detail.contains("secret"));
        assert_eq!(
            fails(&response(EVENT, "<D:getetag>\"1\"</D:getetag>")).detail,
            format!("Fastmail REPORT `{CALENDAR}`: the answer for `{EVENT}` has no calendar-data")
        );
    }

    #[test]
    fn each_tool_maps_fastmails_refusals() {
        let fails = |tool, args: Value, answer| match run(tool, args, vec![answer]).1 {
            Next::Fail(failure) => failure,
            other => panic!("{other:?}"),
        };
        let window = |answer| fails(Provided::FastmailCalendarEvents, window_args(), answer);
        let event = |answer| {
            fails(
                Provided::FastmailCalendarEvent,
                json!({ "event": EVENT }),
                answer,
            )
        };
        assert_eq!(
            window(http(404, "")),
            failure(
                ErrorKind::InvalidArgs,
                &format!("no calendar at `{CALENDAR}`")
            )
        );
        assert_eq!(
            event(http(404, "")),
            failure(ErrorKind::InvalidArgs, &format!("no event at `{EVENT}`"))
        );
        let precondition = r#"<D:error xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:valid-filter/></D:error>"#;
        assert_eq!(
            window(http(403, precondition)).detail,
            format!(
                "Fastmail answered HTTP 403 to REPORT `{CALENDAR}`: precondition `valid-filter` failed"
            )
        );
        assert_eq!(
            window(http(507, "")).detail,
            format!(
                "Fastmail found more than it will return for REPORT `{CALENDAR}` (HTTP 507); {WINDOW_HINT}"
            )
        );
        let cut = Unreadable::Cut("300000 bytes".to_owned());
        assert_eq!(
            window(truncated(207, json!(300000), "<")),
            cut.failure(API_NAME, Some(WINDOW_HINT))
        );
        assert_eq!(
            event(truncated(200, json!(300000), "B")),
            cut.failure(API_NAME, None)
        );
        for (answer, kind) in [
            (http(401, ""), ErrorKind::Internal),
            (nested("denied", "no clause"), ErrorKind::Denied),
        ] {
            assert_eq!(window(answer.clone()).kind, kind, "{answer:?}");
            assert_eq!(event(answer.clone()).kind, kind, "{answer:?}");
        }
        assert_eq!(
            event(ics("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n")),
            failure(ErrorKind::InvalidArgs, &format!("`{EVENT}` holds no event"))
        );
        assert!(
            event(ics("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"))
                .detail
                .starts_with(&format!(
                    "Fastmail's calendar data at `{EVENT}`: the calendar data's line"
                ))
        );
    }

    #[test]
    fn an_event_is_read_in_full_with_its_overrides_in_brief() {
        let ical = vcal(&[
            &[
                "UID:w1@example.com",
                "RECURRENCE-ID;TZID=America/New_York:20261116T090000",
                "DTSTART;TZID=America/New_York:20261116T100000",
                "DTEND;TZID=America/New_York:20261116T110000",
                "SUMMARY:Moved",
                "STATUS:CONFIRMED",
            ],
            &[
                "UID:w1@example.com",
                "SUMMARY:Planning\\; weekly",
                "DESCRIPTION:Agenda:\\n1. Review\\, then plan. A long line that is fold",
                " ed here.",
                "LOCATION:Room 1",
                "DTSTART;TZID=America/New_York:20261102T090000",
                "DTEND;TZID=America/New_York:20261102T100000",
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;TZID=America/New_York:20261109T090000",
                "ORGANIZER;CN=\"Alice, Example\":mailto:alice@example.com",
                "ATTENDEE;CN=Bob;PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT:MAILTO:bob@example.com",
                "ATTENDEE;CN=^'Carol^';PARTSTAT=NEEDS-ACTION:mailto:carol@example.com",
                "BEGIN:VALARM",
                "ACTION:DISPLAY",
                "TRIGGER:-PT15M",
                "END:VALARM",
            ],
            &[
                "UID:w1@example.com",
                "RECURRENCE-ID;TZID=America/New_York:20261123T090000",
                "DTSTART;TZID=America/New_York:20261123T090000",
                "DURATION:PT1H",
                "STATUS:CANCELLED",
            ],
        ]);
        let zoned = |at: &str| json!({ "date_time": at, "time_zone": "America/New_York" });
        assert_eq!(
            completed(event_of(&ical)),
            json!({
                "event": EVENT, "uid": "w1@example.com", "summary": "Planning; weekly",
                "location": "Room 1",
                "description": "Agenda:\n1. Review, then plan. A long line that is folded here.",
                "start": zoned("2026-11-02T09:00:00"), "end": zoned("2026-11-02T10:00:00"),
                "organizer": { "email": "alice@example.com", "name": "Alice, Example" },
                "attendees": [
                    { "email": "bob@example.com", "name": "Bob", "response": "ACCEPTED",
                      "role": "REQ-PARTICIPANT" },
                    { "email": "carol@example.com", "name": "\"Carol\"",
                      "response": "NEEDS-ACTION" },
                ],
                "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO",
                               "EXDATE;TZID=America/New_York:20261109T090000"],
                "overrides": [
                    { "summary": "Moved", "status": "CONFIRMED",
                      "start": zoned("2026-11-16T10:00:00"), "end": zoned("2026-11-16T11:00:00"),
                      "recurrence_id": zoned("2026-11-16T09:00:00") },
                    { "status": "CANCELLED", "start": zoned("2026-11-23T09:00:00"),
                      "duration": "PT1H", "recurrence_id": zoned("2026-11-23T09:00:00") },
                ],
            })
        );
        let only_overrides = vcal(&[
            &[
                "UID:i",
                "RECURRENCE-ID:20261123T090000Z",
                "DTSTART:20261123T090000Z",
            ],
            &[
                "UID:i",
                "RECURRENCE-ID:20261116T090000Z",
                "DTSTART:20261116T090000Z",
            ],
        ]);
        let result = completed(event_of(&only_overrides));
        assert_eq!(
            result["recurrence_id"],
            json!({ "date_time": "2026-11-16T09:00:00Z" })
        );
        assert_eq!(result["overrides"].as_array().unwrap().len(), 1);

        let refused = |events: &[&[&str]], problem: &str| {
            assert_eq!(
                event_of(&vcal(events)),
                Next::Fail(internal(format!(
                    "Fastmail's calendar data at `{EVENT}`: {problem}"
                )))
            );
        };
        let master: &[&str] = &["UID:i", "DTSTART:20261102T090000Z"];
        let moved: &[&str] = &[
            "UID:i",
            "RECURRENCE-ID:20261109T090000Z",
            "DTSTART:20261110T090000Z",
        ];
        refused(
            &[master, moved, &["UID:j", "DTSTART:20261103T090000Z"]],
            "more than one event has no RECURRENCE-ID",
        );
        refused(
            &[master, moved, moved],
            "two events have the same RECURRENCE-ID",
        );
        refused(&[moved, moved], "two events have the same RECURRENCE-ID");
    }

    /// `fill` repeated `n` times.
    fn fill(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    #[test]
    fn every_list_is_capped_and_the_worst_answers_fit_under_the_ceiling() {
        for c in ['x', '€'] {
            let long = |n: usize| fill(c, n + 1);
            let zone = long(event::CODE_MAX);
            let at = |name: &str| format!("{name};TZID={zone}:20261102T090000");
            let status = format!("STATUS:{}", long(event::CODE_MAX));
            let brief = [
                format!("SUMMARY:{}", long(event::BRIEF_TEXT_MAX)),
                format!("LOCATION:{}", long(event::BRIEF_TEXT_MAX)),
                at("DTSTART"),
                at("DTEND"),
                at("RECURRENCE-ID"),
                status.clone(),
            ];
            let brief: Vec<&str> = brief.iter().map(String::as_str).collect();
            let path = |i: usize| format!("/{i:03}{}", "x".repeat(href::MAX_PATH - 4));
            let resources: String = (0..=LIMIT_MAX as usize)
                .map(|i| data(&path(i), &vcal(&[&brief])))
                .collect();
            let mut args = window_args();
            args["limit"] = json!(LIMIT_MAX);
            let window = completed(window_of(args, &resources));
            assert_eq!(window["events_omitted"], json!(1));
            assert_eq!(window["events"][0]["event"], json!(path(0)));
            assert!(serde_json::to_vec(&window).unwrap().len() <= COMPACT_RESULT_CEILING);

            let person = |kind: &str| {
                format!(
                    "{kind};CN={};PARTSTAT={};ROLE={}:mailto:{}",
                    long(event::NAME_MAX),
                    long(event::CODE_MAX),
                    long(event::CODE_MAX),
                    long(event::EMAIL_MAX)
                )
            };
            let mut main = vec![
                format!("UID:{}", long(event::UID_MAX)),
                format!("SUMMARY:{}", long(event::FULL_TEXT_MAX)),
                format!("LOCATION:{}", long(event::FULL_TEXT_MAX)),
                format!("DESCRIPTION:{}", long(event::DESCRIPTION_MAX)),
                at("DTSTART"),
                at("DTEND"),
                status.clone(),
                person("ORGANIZER"),
            ];
            main.extend((0..=event::ATTENDEES_MAX).map(|_| person("ATTENDEE")));
            main.extend(
                (0..=event::RULES_MAX).map(|_| format!("RRULE:FREQ={}", long(event::RULE_MAX))),
            );
            let main: Vec<&str> = main.iter().map(String::as_str).collect();
            let mut events = vec![main.as_slice()];
            let ids: Vec<String> = (0..=event::OVERRIDES_MAX)
                .map(|i| format!("RECURRENCE-ID;TZID={zone}:20261102T09{i:02}00"))
                .collect();
            let overrides: Vec<Vec<&str>> = ids
                .iter()
                .map(|id| {
                    let mut brief = brief.clone();
                    brief[4] = id;
                    brief
                })
                .collect();
            events.extend(overrides.iter().map(Vec::as_slice));
            let read = completed(event_of(&vcal(&events)));
            for (key, cap) in [
                ("attendees", event::ATTENDEES_MAX),
                ("recurrence", event::RULES_MAX),
                ("overrides", event::OVERRIDES_MAX),
            ] {
                assert_eq!(read[key].as_array().unwrap().len(), cap, "{key}");
                assert_eq!(read[format!("{key}_omitted")], json!(1), "{key}");
            }
            assert_eq!(read["description_truncated"], json!(true));
            assert!(read["description"].as_str().unwrap().ends_with('…'));
            assert!(serde_json::to_vec(&read).unwrap().len() <= COMPACT_RESULT_CEILING);

            let entries: String = (0..=CALENDARS_MAX)
                .map(|i| {
                    calendar_entry(
                        &format!("{}/", path(i)),
                        &long(CALENDAR_NAME_MAX),
                        &long(CALENDAR_DESCRIPTION_MAX),
                    )
                })
                .collect();
            let (_, done) = run(
                Provided::FastmailCalendars,
                json!({}),
                vec![
                    principal_answer(PRINCIPAL),
                    home_answer(&[HOME]),
                    ms(&entries),
                ],
            );
            let list = completed(done);
            assert_eq!(list["calendars"].as_array().unwrap().len(), CALENDARS_MAX);
            assert_eq!(list["calendars_omitted"], json!(1));
            assert!(
                list["calendars"][0].get("calendar").is_none(),
                "1025 bytes is no path"
            );
            assert!(serde_json::to_vec(&list).unwrap().len() <= COMPACT_RESULT_CEILING);
        }
    }

    #[test]
    fn an_unknown_step_fails_loudly() {
        for (tool, step) in [
            (Provided::FastmailCalendars, 4),
            (Provided::FastmailCalendarEvents, 2),
            (Provided::FastmailCalendarEvent, 2),
        ] {
            let mut call = plan(tool, &admitted_args(tool), &settings()).unwrap();
            call.step = step;
            assert_eq!(
                advance(
                    &state_for(tool, &admitted_args(tool), &call),
                    ms(""),
                    &settings()
                ),
                Next::Fail(no_such_step(tool, step))
            );
        }
    }

    #[test]
    fn a_request_runs_its_steps_then_completes_and_excess_waits() {
        let host = MemoryHost::new();
        let id = "q".repeat(32);
        let activate = |requests, results| {
            host.activate::<Fastmail>(
                &settings(),
                Input {
                    now_ms: 1_000,
                    requests,
                    results,
                },
            )
            .unwrap();
        };
        let deliver = |call_id: &str, outcome| Delivered {
            call_id: call_id.to_owned(),
            serving: Some(id.clone()),
            outcome,
        };
        activate(vec![incoming(&id, "fastmail-calendars", json!({}))], vec![]);
        let answers = [
            principal_answer(PRINCIPAL),
            home_answer(&[HOME]),
            ms(&calendar_entry(CALENDAR, "Work", "")),
        ];
        for answer in answers {
            let calls = host.take_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].call.method, "PROPFIND");
            activate(vec![], vec![deliver(&calls[0].call_id, answer)]);
        }
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(value["calendars"][0]["calendar"], json!(CALENDAR));
        assert!(host.take_calls().is_empty());

        let requests = (0..=CALL_BUDGET)
            .map(|i| {
                incoming(
                    &format!("{i:032}"),
                    "fastmail-calendar-event",
                    json!({ "event": EVENT }),
                )
            })
            .collect();
        activate(requests, vec![]);
        assert_eq!(host.take_calls().len(), CALL_BUDGET);
        assert_eq!(host.rows(BACKLOG_NS).len(), 1);
    }
}
