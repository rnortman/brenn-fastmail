//! The provider's decisions, in `std`, `serde_json`, the JMAP crate and
//! `brenn-provider-kit`: validating a tool's arguments, building each JMAP
//! request as `api-request` arguments, reading `api-request`'s answer, and
//! compacting Fastmail's JSON into the tool's result. Nothing here names the
//! guest SDK, so the whole module runs on the host under test. Tools are the
//! generated `Provided` enum, whose SDK-facing parts are wasm32-only.
//!
//! Every tool asks for the JMAP session first (step 1), which names the
//! contacts account, then posts one JMAP request (step 2).

// The host test's crate root is this file, so it declares the generated
// module and its sibling itself; in the component, `lib.rs` does.
#[cfg(test)]
mod card;
#[cfg(test)]
mod spec;

use brenn_provider_kit::{
    Args, COMPACT_RESULT_CEILING, Call, Failure, Logic, NestedError, Next, Tool, bounded, capped,
    choice, integer, internal, no_such_step, object,
};
use fastmail_jmap::contacts::{
    ADDRESS_BOOK_PROPERTIES, CARD_READ_PROPERTIES, CARD_SUMMARY_PROPERTIES,
};
use fastmail_jmap::session::CONTACTS;
use fastmail_jmap::{
    AddressBook, AddressBookGet, CallId, CardFilter, ContactCard, ContactCardGet, ContactCardQuery,
    Error as JmapError, Ids, QueryResult, Request, Response, Sort, result_ref,
};
use fastmail_provider::args::{ACCOUNT_ARG, account_arg, token};
use fastmail_provider::failure::{answer_body, derived_str, jmap_failure, stored_args};
use fastmail_provider::results::{in_order, page, the_one};
use fastmail_provider::settings::{API_NAME, Settings};
use fastmail_provider::steps::{ACCOUNT_KEY, SESSION_STEP, after_session, session_call};
use serde_json::{Value, json};

use crate::card::{ID_MAX, NAME_MAX, TEXT_MAX, cut_width};
use crate::spec::Provided;

/// The most characters a free-text search argument may hold.
const TEXT_MAX_CHARS: usize = 1024;

/// A search's page size: its range and default; and the furthest it skips.
const LIMIT_MAX: u64 = 25;
const LIMIT_DEFAULT: u64 = 10;
const POSITION_MAX: u64 = 10_000;

/// The kinds a card may be (RFC 9553 section 2.1.4).
const KINDS: &[&str] = &[
    "individual",
    "group",
    "org",
    "location",
    "device",
    "application",
];

/// The step after the session: the JMAP request.
const API_STEP: u8 = 2;

/// What to narrow when a search's answer is too large.
const LIMIT_HINT: &str = "ask for fewer with `limit`";

// TODO(contacts-query-unconfirmed-against-live-fastmail): unconfirmed against
// a live account that Fastmail sorts by these.
/// A search's order: by surname, then given name.
const NAME_SORT: [Sort; 2] = [
    Sort {
        property: "name/surname",
        is_ascending: true,
    },
    Sort {
        property: "name/given",
        is_ascending: true,
    },
];

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
    AddressBooks,
    Search(SearchArgs),
    Read { id: String },
}

#[derive(Clone, Debug, PartialEq)]
struct SearchArgs {
    /// Every argument given, as one condition: a card matches only when all
    /// its properties match, and an empty one matches every card.
    filter: CardFilter,
    limit: u64,
    position: u64,
}

impl SearchArgs {
    const KEYS: &[&str] = &[
        "text",
        "name",
        "email",
        "phone",
        "organization",
        "address_book",
        "uid",
        "kind",
        "limit",
        "position",
        ACCOUNT_ARG,
    ];

    fn parse(map: &Args) -> Result<SearchArgs, String> {
        let text =
            |key: &str| capped(map, key, TEXT_MAX_CHARS).map(|value| value.map(str::to_owned));
        Ok(SearchArgs {
            filter: CardFilter {
                in_address_book: token(map, "address_book")?,
                uid: text("uid")?,
                kind: choice(map, "kind", KINDS, |kind| kind)?.map(str::to_owned),
                text: text("text")?,
                name: text("name")?,
                email: text("email")?,
                phone: text("phone")?,
                organization: text("organization")?,
            },
            limit: integer(map, "limit", 1, LIMIT_MAX)?.unwrap_or(LIMIT_DEFAULT),
            position: integer(map, "position", 0, POSITION_MAX)?.unwrap_or(0),
        })
    }
}

const READ_KEYS: &[&str] = &["id", ACCOUNT_ARG];

fn parse(tool: Provided, args: &Value) -> Result<Parsed, String> {
    let keys = match tool {
        Provided::FastmailAddressBooks => &[ACCOUNT_ARG],
        Provided::FastmailContactSearch => SearchArgs::KEYS,
        Provided::FastmailContactRead => READ_KEYS,
    };
    let map = object(args, keys)?;
    let tool = match tool {
        Provided::FastmailAddressBooks => ToolArgs::AddressBooks,
        Provided::FastmailContactSearch => ToolArgs::Search(SearchArgs::parse(map)?),
        Provided::FastmailContactRead => ToolArgs::Read {
            id: token(map, "id")?.ok_or("`id` is required")?,
        },
    };
    Ok(Parsed {
        account: account_arg(map)?,
        tool,
    })
}

/// Validate a request's arguments and ask for the session. An error is the
/// request's `invalid_args` detail, naming the argument.
pub fn plan(tool: Provided, args: &Value, settings: &Settings) -> Result<Call, String> {
    let parsed = parse(tool, args)?;
    Ok(session_call(settings, parsed.account.as_deref()))
}

/// The step-2 request for `parsed` in `account_id`, and the call ids its
/// answer is read by, in push order.
fn api_request(parsed: &ToolArgs, account_id: &str) -> (Request, Vec<CallId>) {
    let mut request = Request::contacts();
    let ids = match parsed {
        ToolArgs::AddressBooks => vec![
            AddressBookGet {
                account_id,
                properties: ADDRESS_BOOK_PROPERTIES,
            }
            .push(&mut request),
        ],
        ToolArgs::Search(args) => {
            let query = ContactCardQuery {
                account_id,
                filter: args.filter.clone(),
                sort: NAME_SORT.to_vec(),
                position: args.position,
                limit: args.limit,
                calculate_total: true,
            }
            .push(&mut request);
            let get = ContactCardGet {
                account_id,
                ids: Ids::Ref(result_ref(&query, "/ids")),
                properties: CARD_SUMMARY_PROPERTIES,
            }
            .push(&mut request);
            vec![query, get]
        }
        ToolArgs::Read { id } => vec![
            ContactCardGet {
                account_id,
                ids: Ids::Listed(vec![id.clone()]),
                properties: CARD_READ_PROPERTIES,
            }
            .push(&mut request),
        ],
    };
    (request, ids)
}

/// What the caller narrows when `tool`'s answer is too large: only a search
/// has an argument that does.
fn hint(tool: Provided) -> Option<&'static str> {
    match tool {
        Provided::FastmailContactSearch => Some(LIMIT_HINT),
        Provided::FastmailAddressBooks | Provided::FastmailContactRead => None,
    }
}

/// What the request does with its nested call's answer: complete, fail, or
/// issue its next step. Every completion is held to the result ceiling.
pub fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
    if !matches!(state.step, SESSION_STEP | API_STEP) {
        return Next::Fail(no_such_step(state.tool, state.step));
    }
    match step(state, outcome, settings) {
        Ok(Next::Complete(value)) => bounded(value, COMPACT_RESULT_CEILING, hint(state.tool)),
        Ok(next) => next,
        Err(failure) => Next::Fail(failure),
    }
}

fn step(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    settings: &Settings,
) -> Result<Next, Failure> {
    let unreadable_hint = match state.step {
        SESSION_STEP => None,
        _ => hint(state.tool),
    };
    let body = answer_body(state, outcome, unreadable_hint)?;
    let parsed = stored_args(state, parse)?;
    if state.step == SESSION_STEP {
        return after_session(
            &body,
            CONTACTS,
            settings,
            parsed.account.as_deref(),
            API_STEP,
            |account_id| api_request(&parsed.tool, account_id).0,
        );
    }
    let account_id = derived_str(state, ACCOUNT_KEY)?;
    let response = Response::parse(body).map_err(jmap_failure)?;
    let (_, ids) = api_request(&parsed.tool, account_id);
    let result = |at: usize| response.result(&ids[at]).map_err(jmap_failure);
    match &parsed.tool {
        ToolArgs::AddressBooks => address_books(account_id, result(0)?),
        ToolArgs::Search(_) => {
            let query = response.result(&ids[0]).map_err(search_failure)?;
            search(query, result(1)?)
        }
        ToolArgs::Read { id } => read(id, result(0)?),
    }
}

/// A search's query error: a server that cannot sort by name is the
/// operator's, since the caller does not choose the sort; every other error
/// as [`jmap_failure`] maps it.
fn search_failure(error: JmapError) -> Failure {
    match &error {
        JmapError::Method { kind, .. } if kind == "unsupportedSort" => internal(format!(
            "{API_NAME} does not sort contacts by name (`unsupportedSort`); this tool cannot \
             page its results on this server"
        )),
        _ => jmap_failure(error),
    }
}

fn address_books(account_id: &str, args: &Value) -> Result<Next, Failure> {
    let mut list = AddressBook::list(args).map_err(jmap_failure)?;
    list.sort_by(|a, b| (a.sort_order, &a.name).cmp(&(b.sort_order, &b.name)));
    let books: Vec<Value> = list
        .iter()
        .map(|book| {
            let mut out = Args::new();
            out.insert("id".into(), json!(cut_width(&book.id, ID_MAX).0));
            out.insert("name".into(), json!(cut_width(&book.name, NAME_MAX).0));
            if let Some(description) = &book.description {
                out.insert(
                    "description".into(),
                    json!(cut_width(description, TEXT_MAX).0),
                );
            }
            out.insert("is_default".into(), json!(book.is_default));
            out.insert("is_subscribed".into(), json!(book.is_subscribed));
            Value::Object(out)
        })
        .collect();
    Ok(Next::Complete(
        json!({ "account_id": account_id, "address_books": books }),
    ))
}

fn search(query: &Value, get: &Value) -> Result<Next, Failure> {
    let query = QueryResult::read(query).map_err(jmap_failure)?;
    let cards = ContactCard::get(get).map_err(jmap_failure)?.list;
    let contacts: Vec<Value> = in_order(&query.ids, cards, card_id)
        .iter()
        .map(crate::card::summary)
        .collect();
    Ok(Next::Complete(page(&query, "contacts", contacts)))
}

fn card_id(card: &ContactCard) -> &str {
    &card.id
}

fn read(id: &str, get: &Value) -> Result<Next, Failure> {
    let got = ContactCard::get(get).map_err(jmap_failure)?;
    let card = the_one(got, id, card_id, ContactCardGet::METHOD, "contact")?;
    Ok(Next::Complete(crate::card::read(&card)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use brenn_provider_kit::activation::BACKLOG_NS;
    use brenn_provider_kit::testutil::{Answered, MemoryHost, failure, http, nested, truncated};
    use brenn_provider_kit::{
        ApiCall, Bound, CALL_BUDGET, Delivered, ErrorKind, Input, Query, ToolBounds, Unreadable,
        check, hold_probes, hold_required, input_schema, probes,
    };
    use fastmail_jmap::session::{CORE, MAIL};
    use fastmail_provider::args::ID_MAX_CHARS;
    use fastmail_provider::failure::SESSION_SETUP;
    use fastmail_provider::testutil::{
        ACCOUNT_PATTERN, account_probes, completed, incoming, jmap, row, session_body,
        session_with, settings,
    };
    use serde_json::Map;

    use super::*;

    fn at_api(tool: Provided, args: Value) -> InFlight {
        row(tool, args, API_STEP, json!({ ACCOUNT_KEY: "a1" }))
    }

    fn answered(state: &InFlight, outcome: Result<Value, NestedError>) -> Next {
        advance(state, outcome, &settings())
    }

    fn contacts_session() -> Result<Value, NestedError> {
        session_with(&[CONTACTS])
    }

    /// A search answered with `ids` from the query and `list` from the get.
    fn search_of(args: Value, ids: Value, total: u64, list: Value) -> Next {
        answered(
            &at_api(Provided::FastmailContactSearch, args),
            jmap(json!([
                ["ContactCard/query", { "accountId": "a1", "ids": ids, "position": 0,
                                        "total": total }, "c0"],
                ["ContactCard/get", { "accountId": "a1", "list": list, "notFound": [] }, "c1"],
            ])),
        )
    }

    /// A read of `c1` answered with `card`.
    fn read_of(card: Value) -> Next {
        answered(
            &at_api(Provided::FastmailContactRead, json!({ "id": "c1" })),
            jmap(json!([["ContactCard/get",
                { "accountId": "a1", "list": [card], "notFound": [] }, "c0"]])),
        )
    }

    fn bounds(tool: Provided) -> ToolBounds<'static> {
        let (keys, required): (&'static [&'static str], &'static [&'static str]) = match tool {
            Provided::FastmailAddressBooks => (&[ACCOUNT_ARG], &[]),
            Provided::FastmailContactSearch => (SearchArgs::KEYS, &[]),
            Provided::FastmailContactRead => (READ_KEYS, &["id"]),
        };
        let property = |key: &'static str| {
            let bound = match key {
                "text" | "name" | "email" | "phone" | "organization" | "uid" => Bound::Text {
                    max_chars: TEXT_MAX_CHARS,
                    format: None,
                },
                "address_book" | "id" => Bound::Token {
                    max_chars: ID_MAX_CHARS,
                },
                "kind" => Bound::Enum(KINDS.to_vec()),
                ACCOUNT_ARG => Bound::Pattern(ACCOUNT_PATTERN.to_owned()),
                "limit" => Bound::Integer {
                    min: 1,
                    max: LIMIT_MAX,
                },
                "position" => Bound::Integer {
                    min: 0,
                    max: POSITION_MAX,
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

    fn admitted_args(tool: Provided) -> Value {
        match tool {
            Provided::FastmailContactRead => json!({ "id": "c1" }),
            _ => json!({}),
        }
    }

    #[test]
    fn every_tools_schema_states_what_its_parser_enforces() {
        let path = std::env::var("FASTMAIL_CONTACTS_SPEC").expect("FASTMAIL_CONTACTS_SPEC");
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
                    _ => {
                        probes(bound).unwrap_or_else(|| panic!("`{name}`'s `{key}` has no probes"))
                    }
                };
                hold_probes(name, key, &probes, &base, plan_with);
            }
        }
    }

    /// Every tool asks for the session first, and every nested call of a
    /// request names the request's `account`, or none when it is omitted.
    #[test]
    fn every_tool_asks_for_the_session_first_with_the_requests_account() {
        for account in [Some("alice-work"), None] {
            let expected = account.map(str::to_owned);
            for tool in Provided::ALL {
                let mut args = admitted_args(tool);
                if let Some(account) = account {
                    args["account"] = json!(account);
                }
                let call = plan(tool, &args, &settings()).unwrap();
                assert_eq!(call.step, SESSION_STEP);
                assert_eq!(
                    call.api,
                    ApiCall {
                        provider: "fastmail".to_owned(),
                        account: expected.clone(),
                        method: "GET".to_owned(),
                        host: "api.fastmail.example.com".to_owned(),
                        path: "/jmap/session".to_owned(),
                        query: Query::new(),
                        headers: BTreeMap::new(),
                        body: None,
                    }
                );
                let state = row(tool, args, SESSION_STEP, json!({}));
                let Next::Call { call, .. } = answered(&state, contacts_session()) else {
                    panic!("{tool:?}: the session step issues the request");
                };
                assert_eq!(call.api.account, expected, "{tool:?} request");
            }
        }
        let refused = |tool, args: Value| plan(tool, &args, &settings()).unwrap_err();
        assert_eq!(
            refused(Provided::FastmailContactRead, json!({})),
            "`id` is required"
        );
        assert_eq!(
            refused(Provided::FastmailContactSearch, json!({ "kind": "person" })),
            "`kind` must be one of `individual`, `group`, `org`, `location`, `device`, \
             `application`"
        );
        assert!(refused(Provided::FastmailAddressBooks, json!({ "x": 1 })).contains("`x`"));
    }

    /// The step-2 envelope `tool` posts for `args` from a contacts-only
    /// session, and its `derived`.
    fn posted(tool: Provided, args: Value) -> (Value, Args) {
        let state = row(tool, args, SESSION_STEP, json!({}));
        let Next::Call { call, derived } = answered(&state, contacts_session()) else {
            panic!("the session step issues the request");
        };
        assert_eq!(call.step, API_STEP);
        assert_eq!(
            (call.api.method.as_str(), call.api.path.as_str()),
            ("POST", "/jmap/api")
        );
        let body = serde_json::from_str(call.api.body.as_deref().unwrap()).unwrap();
        (body, derived)
    }

    #[test]
    fn the_session_step_posts_the_tools_request_to_the_contacts_account() {
        let (body, derived) = posted(
            Provided::FastmailContactSearch,
            json!({ "text": "t", "name": "n", "email": "e", "phone": "p", "organization": "o",
                    "address_book": "b1", "uid": "urn:uuid:1", "kind": "group", "limit": 5,
                    "position": 20 }),
        );
        assert_eq!(
            derived,
            Map::from_iter([(ACCOUNT_KEY.to_owned(), json!("a1"))])
        );
        assert_eq!(
            body,
            json!({
                "using": [CORE, CONTACTS],
                "methodCalls": [
                    ["ContactCard/query", {
                        "accountId": "a1",
                        "filter": {
                            "inAddressBook": "b1", "uid": "urn:uuid:1", "kind": "group",
                            "text": "t", "name": "n", "email": "e", "phone": "p",
                            "organization": "o",
                        },
                        "sort": [
                            { "property": "name/surname", "isAscending": true },
                            { "property": "name/given", "isAscending": true },
                        ],
                        "position": 20, "limit": 5, "calculateTotal": true,
                    }, "c0"],
                    ["ContactCard/get", {
                        "accountId": "a1",
                        "#ids": { "resultOf": "c0", "name": "ContactCard/query", "path": "/ids" },
                        "properties": CARD_SUMMARY_PROPERTIES,
                    }, "c1"],
                ],
            })
        );
        let (body, _) = posted(Provided::FastmailContactSearch, json!({}));
        assert_eq!(body["methodCalls"][0][1]["filter"], json!({}));
        assert_eq!(body["methodCalls"][0][1]["limit"], json!(LIMIT_DEFAULT));
        assert_eq!(body["methodCalls"][0][1]["position"], json!(0));
        let (body, _) = posted(Provided::FastmailAddressBooks, json!({}));
        assert_eq!(
            body["methodCalls"],
            json!([["AddressBook/get", { "accountId": "a1", "ids": null,
                "properties": ADDRESS_BOOK_PROPERTIES }, "c0"]])
        );
        let (body, _) = posted(Provided::FastmailContactRead, json!({ "id": "c1" }));
        assert_eq!(
            body["methodCalls"],
            json!([["ContactCard/get", { "accountId": "a1", "ids": ["c1"],
                "properties": CARD_READ_PROPERTIES }, "c0"]])
        );
    }

    #[test]
    fn a_token_that_cannot_read_contacts_is_the_operators() {
        let state = row(
            Provided::FastmailAddressBooks,
            json!({}),
            SESSION_STEP,
            json!({}),
        );
        let refusal = |why: &str| {
            Next::Fail(internal(format!(
                "the Fastmail token for this account cannot be used here: JMAP session: {why}; \
                 {SESSION_SETUP}"
            )))
        };
        assert_eq!(
            answered(&state, session_body()),
            refusal("the account does not advertise urn:ietf:params:jmap:contacts")
        );
        let no_account = json!({ "capabilities": { CORE: {}, CONTACTS: {}, MAIL: {} },
                                 "primaryAccounts": { MAIL: "a1" } });
        assert_eq!(
            answered(&state, http(200, &no_account.to_string())),
            refusal("no primary account for urn:ietf:params:jmap:contacts")
        );
        for status in [401, 403] {
            assert_eq!(
                answered(&state, http(status, "{\"secret\":\"x\"}")),
                Next::Fail(internal(format!(
                    "Fastmail refused the token for this account (HTTP {status}): the \
                     operator must replace its key file and restart Brenn"
                )))
            );
        }
    }

    #[test]
    fn address_books_are_listed_in_sort_order_then_by_name() {
        let state = at_api(Provided::FastmailAddressBooks, json!({}));
        let list = json!([
            { "id": "b3", "name": "Work", "sortOrder": 1, "isDefault": false,
              "isSubscribed": true },
            { "id": "b2", "name": "Shared", "description": "Family", "sortOrder": 0,
              "isDefault": false, "isSubscribed": false },
            { "id": "b1", "name": "Personal", "isDefault": true, "isSubscribed": true },
        ]);
        let result = completed(answered(
            &state,
            jmap(json!([["AddressBook/get",
                { "accountId": "a1", "state": "s", "list": list, "notFound": [] }, "c0"]])),
        ));
        assert_eq!(
            result,
            json!({ "account_id": "a1", "address_books": [
                { "id": "b1", "name": "Personal", "is_default": true, "is_subscribed": true },
                { "id": "b2", "name": "Shared", "description": "Family", "is_default": false,
                  "is_subscribed": false },
                { "id": "b3", "name": "Work", "is_default": false, "is_subscribed": true },
            ] })
        );
    }

    #[test]
    fn a_search_answers_summaries_in_query_order() {
        let list = json!([
            { "id": "c2", "addressBookIds": { "b1": true } },
            { "id": "c1", "kind": "individual", "name": { "full": "Alice Example" },
              "addressBookIds": { "b2": true, "b1": true, "b0": false },
              "emails": { "e1": { "address": "alice@work.example.com" },
                          "e2": { "address": "alice@example.com", "pref": 1 },
                          "e3": { "address": "a@example.com", "pref": 5 } },
              "phones": { "p1": { "number": "tel:+1-555-0100" } },
              "organizations": { "a": { "units": [{ "name": "Volunteers" }] },
                                 "b": { "name": "Example, Inc." },
                                 "c": { "name": "Other Org" } } },
        ]);
        // `c9` was deleted between the query and the get.
        let result = completed(search_of(
            json!({ "limit": 3 }),
            json!(["c1", "c9", "c2"]),
            8,
            list,
        ));
        assert_eq!(
            result,
            json!({ "position": 0, "total": 8, "next_position": 3, "contacts": [
                { "id": "c1", "kind": "individual", "name": "Alice Example",
                  "email": "alice@example.com", "emails_omitted": 2,
                  "phone": "tel:+1-555-0100", "organization": "Example, Inc.",
                  "organizations_omitted": 2, "address_book_ids": ["b1", "b2"] },
                { "id": "c2", "kind": "individual", "address_book_ids": ["b1"] },
            ] })
        );
        // No organization with a name: none is shown, every one is omitted.
        let units_only = json!([{ "id": "c1", "organizations": {
            "a": { "units": [{ "name": "Volunteers" }] } } }]);
        let result = completed(search_of(json!({}), json!(["c1"]), 1, units_only));
        assert_eq!(
            result["contacts"],
            json!([{ "id": "c1", "kind": "individual", "organizations_omitted": 1 }])
        );
        assert_eq!(result.get("next_position"), None);
        let books: Map<String, Value> =
            (0..12).map(|i| (format!("b{i:02}"), json!(true))).collect();
        let many = json!([{ "id": "c1", "addressBookIds": books }]);
        let result = completed(search_of(json!({}), json!(["c1"]), 1, many));
        assert_eq!(
            result["contacts"][0]["address_book_ids"]
                .as_array()
                .unwrap()
                .len(),
            10
        );
        assert_eq!(result["contacts"][0]["address_book_ids_omitted"], json!(2));
    }

    #[test]
    fn a_search_that_cannot_sort_by_name_is_the_operators_and_a_bad_filter_the_callers() {
        let state = at_api(Provided::FastmailContactSearch, json!({}));
        let error =
            |kind: &str| jmap(json!([["error", { "type": kind, "description": "why" }, "c0"]]));
        assert_eq!(
            answered(&state, error("unsupportedSort")),
            Next::Fail(internal(
                "Fastmail does not sort contacts by name (`unsupportedSort`); this tool cannot \
                 page its results on this server"
            ))
        );
        for kind in ["unsupportedFilter", "invalidArguments"] {
            assert_eq!(
                answered(&state, error(kind)),
                Next::Fail(failure(
                    ErrorKind::InvalidArgs,
                    &format!("Fastmail: ContactCard/query failed: {kind}: why")
                ))
            );
        }
        assert_eq!(
            answered(&state, error("unknownMethod")),
            Next::Fail(internal(
                "Fastmail: ContactCard/query failed: unknownMethod: why"
            ))
        );
    }

    /// A card with every property the read shows.
    fn full_card() -> Value {
        json!({
            "@type": "Card", "id": "c1", "uid": "urn:uuid:1", "kind": "individual",
            "created": "2022-09-30T14:35:10Z", "updated": "2022-11-23T15:01:32Z",
            "addressBookIds": { "b1": true },
            "name": { "components": [
                { "kind": "given", "value": "John" }, { "kind": "surname", "value": "Public" },
                { "kind": "separator", "value": ", " }, { "kind": "generation", "value": "Esq." },
            ], "isOrdered": true },
            "nicknames": { "k1": { "name": "Johnny" } },
            "organizations": {
                "o1": { "units": [{ "name": "Volunteers" }] },
                "o2": { "name": "ABC, Inc.", "units": [{ "name": "Marketing" }] },
            },
            "titles": { "t1": { "name": "Project Leader", "organizationId": "o2" },
                        "t2": { "name": "Treasurer", "kind": "role", "organizationId": "o1" } },
            "emails": { "e1": { "address": "alice@example.com", "label": "home",
                                "contexts": { "private": true }, "pref": 1 } },
            "phones": { "p1": { "number": "tel:+1-555-0100",
                                "features": { "voice": true, "text": true } } },
            "onlineServices": { "s1": { "service": "Mastodon", "user": "@alice@example.com",
                                        "uri": "https://example.com/@alice" } },
            "addresses": { "a1": { "components": [
                { "kind": "number", "value": "54321" }, { "kind": "name", "value": "Oak St" },
            ], "countryCode": "US", "contexts": { "work": true } } },
            "anniversaries": {
                "d1": { "kind": "birth", "date": { "year": 1953, "month": 4, "day": 15 } },
                "d2": { "kind": "death", "date": { "@type": "Timestamp",
                                                   "utc": "2019-10-15T23:10:00Z" } },
                "d3": { "kind": "wedding", "date": { "month": 6, "day": 21 } },
                "d4": { "kind": "x-graduation", "date": { "year": 1975 } },
                "d5": { "kind": "x-hire", "date": { "year": 2001, "month": 9 } },
            },
            "notes": { "n1": { "note": "Prefers \"JQ\".\nCall after 10.",
                               "created": "2022-11-23T15:01:32Z" } },
            "media": { "m1": { "kind": "photo", "uri": "data:image/png;base64,AAAA" } },
            "example.com:favoriteColor": "blue",
        })
    }

    #[test]
    fn a_read_answers_every_key_the_card_has() {
        assert_eq!(
            completed(read_of(full_card())),
            json!({
                "id": "c1", "uid": "urn:uuid:1", "kind": "individual",
                "created": "2022-09-30T14:35:10Z", "updated": "2022-11-23T15:01:32Z",
                "name": "John Public, Esq.",
                "name_components": [
                    { "kind": "given", "value": "John" }, { "kind": "surname", "value": "Public" },
                    { "kind": "generation", "value": "Esq." },
                ],
                "nicknames": [{ "key": "k1", "name": "Johnny" }],
                "organizations": [{ "key": "o1", "units": ["Volunteers"] },
                                  { "key": "o2", "name": "ABC, Inc.", "units": ["Marketing"] }],
                "titles": [
                    { "key": "t1", "name": "Project Leader", "kind": "title",
                      "organization": "ABC, Inc." },
                    { "key": "t2", "name": "Treasurer", "kind": "role" },
                ],
                "emails": [{ "key": "e1", "address": "alice@example.com",
                             "contexts": ["private"], "label": "home", "pref": 1 }],
                "phones": [{ "key": "p1", "number": "tel:+1-555-0100",
                             "features": ["text", "voice"] }],
                "online_services": [{ "key": "s1", "service": "Mastodon",
                                      "user": "@alice@example.com",
                                      "uri": "https://example.com/@alice" }],
                "addresses": [{ "key": "a1", "text": "54321 Oak St", "country_code": "US",
                                "contexts": ["work"] }],
                "anniversaries": [
                    { "key": "d1", "kind": "birth", "date": "1953-04-15" },
                    { "key": "d2", "kind": "death", "date": "2019-10-15T23:10:00Z" },
                    { "key": "d3", "kind": "wedding", "date": "--06-21" },
                    { "key": "d4", "kind": "x-graduation", "date": "1975" },
                    { "key": "d5", "kind": "x-hire", "date": "2001-09" },
                ],
                "notes": [{ "key": "n1", "note": "Prefers \"JQ\".\nCall after 10.",
                            "created": "2022-11-23T15:01:32Z", "truncated": false }],
                "address_book_ids": ["b1"],
            })
        );
        assert_eq!(
            completed(read_of(
                json!({ "id": "c1", "addressBookIds": { "b1": true } })
            )),
            json!({ "id": "c1", "kind": "individual", "address_book_ids": ["b1"] })
        );
        let group = json!({ "id": "c1", "kind": "group", "name": { "full": "The Doe family" },
            "members": { "urn:uuid:2": true, "urn:uuid:1": true, "urn:uuid:3": false } });
        assert_eq!(
            completed(read_of(group)),
            json!({ "id": "c1", "kind": "group", "name": "The Doe family",
                    "members": ["urn:uuid:1", "urn:uuid:2"] })
        );
        let fits = "x".repeat(crate::card::ID_MAX);
        let over = "y".repeat(crate::card::ID_MAX + 1);
        let group = json!({ "id": "c1", "kind": "group",
            "members": { "urn:uuid:1": true, fits.clone(): true, over: true } });
        assert_eq!(
            completed(read_of(group)),
            json!({ "id": "c1", "kind": "group", "members": ["urn:uuid:1", fits],
                    "members_omitted": 1 })
        );
        let group = json!({ "id": "c1", "kind": "group",
            "members": { "z".repeat(crate::card::ID_MAX + 1): true } });
        assert_eq!(
            completed(read_of(group)),
            json!({ "id": "c1", "kind": "group", "members_omitted": 1 })
        );
    }

    #[test]
    fn a_read_caps_every_list_and_cuts_a_long_note() {
        let map = |n: usize, entry: &dyn Fn(usize) -> Value| -> Value {
            Value::Object((0..n).map(|i| (format!("k{i:03}"), entry(i))).collect())
        };
        let set = |n: usize| map(n, &|_| json!(true));
        let card = json!({
            "id": "c1",
            "name": { "components": (0..21).map(|i| json!({ "kind": "given",
                                                              "value": format!("n{i}") }))
                                           .collect::<Vec<_>>() },
            "nicknames": map(11, &|i| json!({ "name": format!("n{i}") })),
            "organizations": map(11, &|_| json!({ "units": (0..6).map(|u| json!({
                "name": format!("u{u}") })).collect::<Vec<_>>() })),
            "titles": map(11, &|i| json!({ "name": format!("t{i}") })),
            "emails": map(21, &|i| json!({ "address": format!("u{i}@example.com"),
                                           "contexts": set(9) })),
            "phones": map(21, &|i| json!({ "number": format!("{i}"), "features": set(9) })),
            "onlineServices": map(11, &|_| json!({ "service": "x" })),
            "addresses": map(11, &|_| json!({ "full": "x" })),
            "anniversaries": map(11, &|_| json!({ "kind": "birth", "date": { "year": 2000 } })),
            "notes": map(11, &|_| json!({ "note": "x".repeat(4001) })),
            "members": set(101),
            "addressBookIds": set(21),
        });
        let result = completed(read_of(card));
        for (key, cap, omitted) in [
            ("name_components", 20, 1),
            ("nicknames", 10, 1),
            ("organizations", 10, 1),
            ("titles", 10, 1),
            ("emails", 20, 1),
            ("phones", 20, 1),
            ("online_services", 10, 1),
            ("addresses", 10, 1),
            ("anniversaries", 10, 1),
            ("notes", 10, 1),
            ("members", 100, 1),
            ("address_book_ids", 20, 1),
        ] {
            assert_eq!(result[key].as_array().unwrap().len(), cap, "{key}");
            assert_eq!(result[format!("{key}_omitted")], json!(omitted), "{key}");
        }
        assert_eq!(
            result["organizations"][0]["units"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert_eq!(result["organizations"][0]["units_omitted"], json!(1));
        assert_eq!(result["emails"][0]["contexts"].as_array().unwrap().len(), 8);
        assert_eq!(result["emails"][0]["contexts_omitted"], json!(1));
        assert_eq!(result["phones"][0]["features_omitted"], json!(1));
        assert_eq!(
            result["notes"][0],
            json!({ "key": "k000", "note": format!("{}…", "x".repeat(4000)),
                    "truncated": true })
        );
    }

    #[test]
    fn an_entry_key_too_wide_to_show_whole_is_left_out_and_the_entry_shown() {
        let fits = "k".repeat(crate::card::ID_MAX);
        let over = "o".repeat(crate::card::ID_MAX + 1);
        let card = json!({ "id": "c1",
            "emails": { fits.clone(): { "address": "alice@example.com", "pref": 1 },
                        over.clone(): { "address": "bob@example.com", "pref": 2 } },
            "nicknames": { over: { "name": "Al" } } });
        let result = completed(read_of(card));
        assert_eq!(
            result["emails"],
            json!([{ "key": fits, "address": "alice@example.com", "pref": 1 },
                   { "address": "bob@example.com", "pref": 2 }])
        );
        assert_eq!(result["nicknames"], json!([{ "name": "Al" }]));
    }

    #[test]
    fn a_read_of_an_unknown_contact_is_the_callers_and_of_neither_the_operators() {
        let state = at_api(Provided::FastmailContactRead, json!({ "id": "c1" }));
        let get = |not_found: Value| {
            jmap(json!([["ContactCard/get",
                { "accountId": "a1", "list": [], "notFound": not_found }, "c0"]]))
        };
        assert_eq!(
            answered(&state, get(json!(["c1"]))),
            Next::Fail(failure(ErrorKind::InvalidArgs, "no contact with id `c1`"))
        );
        assert_eq!(
            answered(&state, get(json!([]))),
            Next::Fail(internal(
                "Fastmail's ContactCard/get answered neither contact `c1` nor its absence"
            ))
        );
    }

    /// A card with every list over its cap and every string over its bound,
    /// each string `fill` repeated: a note one character past the note bound,
    /// every other string one past the widest other bound, except the entry
    /// keys, as wide as a shown key can be, and the member uids, 101 as wide
    /// as a shown uid can be and one past it.
    fn worst_card(id: &str, fill: char) -> Value {
        let note: String = std::iter::repeat_n(fill, crate::card::NOTE_MAX + 1).collect();
        let long: String = std::iter::repeat_n(fill, TEXT_MAX + 1).collect();
        let s = || json!(long);
        let width = serde_json::to_string(&fill).unwrap().len() - 2;
        let uid: String = std::iter::repeat_n(fill, (crate::card::ID_MAX - 3) / width).collect();
        let set: Value = Value::Object(
            (0..9)
                .map(|i| (format!("{long}{i}"), json!(true)))
                .collect(),
        );
        let key = |i: usize| format!("{i:03}{uid}");
        let map = |n: usize, entry: Value| -> Value {
            Value::Object((0..n).map(|i| (key(i), entry.clone())).collect())
        };
        let components: Vec<Value> = (0..21)
            .map(|_| json!({ "kind": long, "value": long }))
            .collect();
        json!({
            "id": id, "uid": s(), "kind": s(), "created": s(), "updated": s(),
            "name": { "components": components, "defaultSeparator": s() },
            "nicknames": map(11, json!({ "name": s() })),
            "organizations": map(11, json!({ "name": s(),
                "units": (0..6).map(|_| json!({ "name": long })).collect::<Vec<_>>() })),
            "titles": map(11, json!({ "name": s(), "kind": s(), "organizationId": key(0) })),
            "emails": map(21, json!({ "address": s(), "contexts": set, "label": s(),
                                       "pref": 1 })),
            "phones": map(21, json!({ "number": s(), "features": set, "contexts": set,
                                       "label": s(), "pref": 1 })),
            "onlineServices": map(11, json!({ "service": s(), "uri": s(), "user": s(),
                "contexts": set, "label": s(), "pref": 1 })),
            "addresses": map(11, json!({ "full": s(), "countryCode": s(), "contexts": set,
                                          "pref": 1 })),
            "anniversaries": map(11, json!({ "kind": s(),
                "date": { "@type": "Timestamp", "utc": s() } })),
            "notes": map(11, json!({ "note": note, "created": s() })),
            "members": Value::Object((0..101).map(|i| (format!("{i:03}{uid}"), json!(true)))
                                             .chain([(long.clone(), json!(true))])
                                             .collect()),
            "addressBookIds": Value::Object((0..21).map(|i| (format!("{long}{i}"), json!(true)))
                                                   .collect()),
        })
    }

    #[test]
    fn the_worst_card_and_the_worst_page_fit_under_the_ceiling() {
        for fill in ['x', '\u{1}', '€'] {
            let Next::Complete(read) = read_of(worst_card("c1", fill)) else {
                panic!("{fill:?}: the worst card is refused");
            };
            let size = serde_json::to_vec(&read).unwrap().len();
            assert!(
                size <= COMPACT_RESULT_CEILING,
                "{fill:?}: read of {size} bytes"
            );
            let ids: Vec<String> = (0..25).map(|i| format!("c{i}")).collect();
            let list: Vec<Value> = ids.iter().map(|id| worst_card(id, fill)).collect();
            let Next::Complete(page) =
                search_of(json!({ "limit": 25 }), json!(ids), 25, json!(list))
            else {
                panic!("{fill:?}: the worst page is refused");
            };
            let size = serde_json::to_vec(&page).unwrap().len();
            assert!(
                size <= COMPACT_RESULT_CEILING,
                "{fill:?}: page of {size} bytes"
            );
            assert_eq!(page["contacts"].as_array().unwrap().len(), 25);
        }
    }

    #[test]
    fn every_step_answers_an_unreadable_read_by_its_shape() {
        let refusal = "result exceeds [messaging] max_body_bytes";
        let cases = [
            (
                row(
                    Provided::FastmailContactSearch,
                    json!({}),
                    SESSION_STEP,
                    json!({}),
                ),
                None,
            ),
            (at_api(Provided::FastmailAddressBooks, json!({})), None),
            (
                at_api(Provided::FastmailContactSearch, json!({})),
                Some(LIMIT_HINT),
            ),
            (
                at_api(Provided::FastmailContactRead, json!({ "id": "c1" })),
                None,
            ),
        ];
        for (state, hint) in cases {
            for (outcome, why) in [
                (
                    truncated(200, json!(300000), "{"),
                    Unreadable::Cut("300000 bytes".to_owned()),
                ),
                (
                    nested("internal", refusal),
                    Unreadable::Refused(refusal.to_owned()),
                ),
            ] {
                assert_eq!(
                    answered(&state, outcome),
                    Next::Fail(why.failure(API_NAME, hint)),
                    "{} step {}",
                    state.tool.name(),
                    state.step
                );
            }
        }
    }

    #[test]
    fn a_missing_step_or_a_corrupt_row_fails_loudly() {
        for tool in Provided::ALL {
            let state = row(tool, admitted_args(tool), 3, json!({ ACCOUNT_KEY: "a1" }));
            assert_eq!(
                answered(&state, jmap(json!([]))),
                Next::Fail(no_such_step(tool, 3))
            );
        }
        let empty = jmap(json!([["AddressBook/get",
            { "accountId": "a1", "list": [], "notFound": [] }, "c0"]]));
        for (state, says) in [
            (
                row(
                    Provided::FastmailAddressBooks,
                    json!({}),
                    API_STEP,
                    json!({}),
                ),
                "carries no `account_id`",
            ),
            (
                at_api(Provided::FastmailContactRead, json!({ "bogus": 1 })),
                "stored fastmail-contact-read arguments no longer parse",
            ),
        ] {
            let Next::Fail(failed) = answered(&state, empty.clone()) else {
                panic!("{says}: does not fail");
            };
            assert_eq!(failed.kind, ErrorKind::Internal, "{says}: {failed:?}");
            assert!(failed.detail.contains(says), "{says}: {failed:?}");
        }
    }

    #[test]
    fn a_request_runs_session_then_api_then_completes_and_excess_waits() {
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
        activate(
            vec![incoming(&id, "fastmail-address-books", json!({}))],
            vec![],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.path, "/jmap/session");
        activate(vec![], vec![deliver(&calls[0].call_id, contacts_session())]);
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].call.method.as_str(), calls[0].call.path.as_str()),
            ("POST", "/jmap/api")
        );
        let list = json!([{ "id": "b1", "name": "Personal", "isDefault": true,
                            "isSubscribed": true }]);
        activate(
            vec![],
            vec![deliver(
                &calls[0].call_id,
                jmap(json!([["AddressBook/get",
                    { "accountId": "a1", "list": list, "notFound": [] }, "c0"]])),
            )],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, id);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(value["address_books"][0]["name"], json!("Personal"));
        assert!(host.take_calls().is_empty());

        let requests = (0..=CALL_BUDGET)
            .map(|i| {
                incoming(
                    &format!("{i:032}"),
                    "fastmail-contact-read",
                    json!({ "id": "c1" }),
                )
            })
            .collect();
        activate(requests, vec![]);
        assert_eq!(host.take_calls().len(), CALL_BUDGET);
        assert_eq!(host.rows(BACKLOG_NS).len(), 1);
    }
}
