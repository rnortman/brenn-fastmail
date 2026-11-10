//! The contacts write provider's decisions, in `std`, `serde_json`, the JMAP
//! crate, the shared provider crate and `brenn-provider-kit`: validating a
//! tool's arguments, building each JMAP request as `api-request` arguments,
//! and reading Fastmail's answer into the tool's result. Nothing here names
//! the guest SDK, so the whole module runs on the host under test. Tools are
//! the generated `Provided` enum, whose SDK-facing parts are wasm32-only.
//!
//! Every tool asks for the JMAP session first (step 1), which names the
//! contacts account. A create then posts one `ContactCard/set` (step 2), the
//! write. An update reads the card's keys for the list it changes, and its
//! name (step 2), builds a patch that names only what the caller changes, and
//! posts it with `ifInState` set to the read's state (step 3), so a card
//! changed in between is refused rather than overwritten. A failure that
//! leaves the write's outcome unknown carries the tool's hint saying what may
//! already have happened; Fastmail's refusal, of the request, the method or
//! the change, does not.

// The host test's crate root is this file, so it declares the generated
// module itself; in the component, `lib.rs` does.
#[cfg(test)]
mod spec;

use brenn_provider_kit::{
    Args, COMPACT_RESULT_CEILING, Call, Failure, Logic, NestedError, Next, Tool, bounded, choice,
    flag, integer, internal, no_such_step, object,
};
use fastmail_jmap::session::CONTACTS;
use fastmail_jmap::{
    AnniversaryDate, CallId, CardKeys, ContactCardGet, ContactCardSet, EntryChange, EntryFields,
    EntryList, EntryValue, Error as JmapError, Ids, NameChange, NameRefusal, NewCard, Patch,
    Request, Response,
};
use fastmail_provider::args::{
    ACCOUNT_ARG, Lines, account_arg, distinct_list, held_text, id_list, one_line, token, wire_width,
};
use fastmail_provider::contacts::list_name;
use fastmail_provider::failure::{
    StepFailure, derived_str, invalid, jmap_failure, set_failure, step_answer, stored_args,
    write_method_failure,
};
use fastmail_provider::results::the_one;
use fastmail_provider::settings::{API_NAME, Settings};
use fastmail_provider::steps::{ACCOUNT_KEY, SESSION_STEP, after_session, api_step, session_call};
use serde_json::{Value, json};

use crate::spec::Provided;

/// The most characters a name, a short entry value or an entry's other text
/// may hold.
const TEXT_MAX_CHARS: usize = 255;
/// The most characters a URI or a postal address may hold.
const LONG_TEXT_MAX_CHARS: usize = 1000;
/// The most characters a note may hold.
const NOTE_MAX_CHARS: usize = 4000;
/// The most values in each of create's lists, and in each of update's
/// address-book lists.
const LIST_MAX: usize = 10;
/// The most addresses and notes create takes.
const SHORT_LIST_MAX: usize = 5;
/// The highest `pref` (RFC 9553 section 1.5.4), the least preferred.
const PREF_MAX: u64 = 100;
/// The most bytes create's strings may take once escaped twice, as
/// `wire_width` counts them. Every string sits in the JMAP body, itself a
/// string in the nested call's arguments; with under 50 bytes of structure
/// for each of at most 71 entries, the call stays well under the host's
/// 64 KiB cap on its arguments.
const CARD_MAX_WIRE_BYTES: usize = 30_000;

/// The contexts an entry may be in (RFC 9553 section 1.5.1); only an
/// address takes the last two.
const CONTEXTS: &[&str] = &["private", "work", "billing", "delivery"];
const PERSONAL_CONTEXTS: &[&str] = &["private", "work"];
/// The features a phone may have (RFC 9553 section 2.3.3).
const FEATURES: &[&str] = &[
    "mobile",
    "voice",
    "text",
    "video",
    "main-number",
    "textphone",
    "fax",
    "pager",
];
/// The kinds a title or an anniversary may be, the update's `kind`.
const KINDS: &[&str] = &["title", "role", "birth", "death", "wedding"];

/// The update's entry arguments that change a sub-property, but `value`.
const OTHER_FIELD_KEYS: &[&str] = &[
    "label",
    "contexts",
    "features",
    "pref",
    "kind",
    "service",
    "user",
    "country_code",
];

/// What this component writes, as a refusal for a read-only token names it.
const DATA: &str = "contacts";

/// A create's write, and an update's read.
const CREATE_STEP: u8 = 2;
const READ_STEP: u8 = 2;
/// An update's write.
const UPDATE_STEP: u8 = 3;

/// The creation id of create's one new card.
const CREATE_ID: &str = "contact";
/// The `derived` key an add's new entry key is kept under.
const NEW_KEY: &str = "new_key";

/// What a lost answer at each write step leaves the caller to know.
const CREATE_UNKNOWN_HINT: &str = "the contact may have been created before its answer was \
     lost: look for it with FastmailContactSearch before asking again, or the user has two";
const UPDATE_UNKNOWN_HINT: &str = "the change may already have been applied: read the contact \
     with FastmailContactRead before asking again";
/// An update whose read is out of date: the server changed nothing.
const STATE_MISMATCH: &str = "the account's contacts changed while this update was being \
     prepared, so nothing was changed; ask again";

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
    Create(CreateArgs),
    Update(Box<UpdateArgs>),
}

/// An argument that is 1 to `max_items` distinct texts, each as
/// [`held_text`] admits it; absent is `None`.
fn text_list(
    map: &Args,
    key: &str,
    max_items: usize,
    max_chars: usize,
    lines: Lines,
) -> Result<Option<Vec<String>>, String> {
    distinct_list(map, key, max_items, "texts", |item| {
        let Value::String(text) = item else {
            return Err(format!("each of `{key}` must be a string"));
        };
        held_text(&format!("each of `{key}`"), text, max_chars, lines)?;
        Ok(text.clone())
    })
}

/// An argument that is 1 to `values.len()` distinct members of `values`;
/// absent is `None`.
fn member_list(map: &Args, key: &str, values: &[&str]) -> Result<Option<Vec<String>>, String> {
    let what = format!("of {}", spelled(values));
    distinct_list(map, key, values.len(), &what, |item| {
        item.as_str()
            .filter(|member| values.contains(member))
            .map(str::to_owned)
            .ok_or_else(|| format!("`{key}` must be an array of 1 to {} {what}", values.len()))
    })
}

/// A partial date as `what`, in the forms `birthday` takes.
fn date(what: &str, text: &str) -> Result<AnniversaryDate, String> {
    AnniversaryDate::parse_partial(text).ok_or_else(|| {
        format!(
            "{what} must be a date: 1990-04-15, 1990-04, 1990 or --04-15, month 1 to 12, day 1 \
             to 31"
        )
    })
}

/// The name arguments both tools take, as one change; `None` when none is
/// given.
fn name_change(map: &Args) -> Result<Option<NameChange>, String> {
    let change = NameChange {
        full: one_line(map, "name", TEXT_MAX_CHARS)?,
        given: one_line(map, "given", TEXT_MAX_CHARS)?,
        surname: one_line(map, "surname", TEXT_MAX_CHARS)?,
    };
    Ok((change != NameChange::default()).then_some(change))
}

/// What one list's entries take from an update.
struct ListRules {
    /// `value`'s bound; `None` for an anniversary's date.
    value: Option<(usize, Lines)>,
    /// The other entry fields the list takes.
    takes: &'static [&'static str],
    contexts: &'static [&'static str],
    kinds: &'static [&'static str],
}

impl ListRules {
    fn of(list: EntryList) -> ListRules {
        let text = Some((TEXT_MAX_CHARS, Lines::One));
        let (value, takes, contexts, kinds): (_, &[&str], &[&str], &[&str]) = match list {
            EntryList::Emails => (text, &["label", "contexts", "pref"], PERSONAL_CONTEXTS, &[]),
            EntryList::Phones => (
                text,
                &["label", "contexts", "features", "pref"],
                PERSONAL_CONTEXTS,
                &[],
            ),
            EntryList::OnlineServices => (
                Some((LONG_TEXT_MAX_CHARS, Lines::One)),
                &["service", "user", "label", "contexts", "pref"],
                PERSONAL_CONTEXTS,
                &[],
            ),
            EntryList::Addresses => (
                Some((LONG_TEXT_MAX_CHARS, Lines::Many)),
                &["country_code", "contexts", "pref"],
                CONTEXTS,
                &[],
            ),
            EntryList::Organizations => (text, &[], &[], &[]),
            EntryList::Titles => (text, &["kind"], &[], &["title", "role"]),
            EntryList::Nicknames => (text, &["contexts", "pref"], PERSONAL_CONTEXTS, &[]),
            EntryList::Anniversaries => (None, &["kind"], &[], &["birth", "death", "wedding"]),
            EntryList::Notes => (Some((NOTE_MAX_CHARS, Lines::Many)), &[], &[], &[]),
        };
        ListRules {
            value,
            takes,
            contexts,
            kinds,
        }
    }
}

/// Create's arguments.
#[derive(Clone, Debug, PartialEq)]
struct CreateArgs {
    address_book: String,
    name: Option<NameChange>,
    lists: Vec<(EntryList, Vec<String>)>,
    birthday: Option<AnniversaryDate>,
}

/// Create's lists, each with its item cap; each value is held to the bound
/// an update holds the list's `value` to.
const CREATE_LISTS: [(EntryList, usize); 8] = [
    (EntryList::Emails, LIST_MAX),
    (EntryList::Phones, LIST_MAX),
    (EntryList::OnlineServices, LIST_MAX),
    (EntryList::Addresses, SHORT_LIST_MAX),
    (EntryList::Organizations, LIST_MAX),
    (EntryList::Titles, LIST_MAX),
    (EntryList::Nicknames, LIST_MAX),
    (EntryList::Notes, SHORT_LIST_MAX),
];

impl CreateArgs {
    const KEYS: &[&str] = &[
        "address_book",
        "name",
        "given",
        "surname",
        "emails",
        "phones",
        "online_services",
        "addresses",
        "organizations",
        "titles",
        "nicknames",
        "notes",
        "birthday",
        ACCOUNT_ARG,
    ];

    fn parse(map: &Args) -> Result<CreateArgs, String> {
        let address_book = token(map, "address_book")?.ok_or("`address_book` is required")?;
        let name = name_change(map)?;
        let mut lists = Vec::new();
        for (list, max_items) in CREATE_LISTS {
            let Some((max_chars, lines)) = ListRules::of(list).value else {
                unreachable!("every list create takes holds text")
            };
            if let Some(values) = text_list(map, list_name(list), max_items, max_chars, lines)? {
                lists.push((list, values));
            }
        }
        let birthday = match map.get("birthday") {
            None => None,
            Some(Value::String(text)) => Some(date("`birthday`", text)?),
            Some(_) => return Err("`birthday` must be a string".to_owned()),
        };
        if name.is_none() && lists.is_empty() && birthday.is_none() {
            return Err(
                "give at least one of `name`, `given`, `surname`, a list or `birthday`: \
                        a contact needs more than its address book"
                    .to_owned(),
            );
        }
        let wire: usize = map.values().map(strings_wire_width).sum();
        if wire > CARD_MAX_WIRE_BYTES {
            return Err(
                "the contact's text is too wide for one request: give fewer or shorter entries"
                    .to_owned(),
            );
        }
        Ok(CreateArgs {
            address_book,
            name,
            lists,
            birthday,
        })
    }

    fn card(&self, uid: String) -> NewCard {
        NewCard {
            uid,
            address_book: self.address_book.clone(),
            name: self.name.clone(),
            lists: self.lists.clone(),
            birthday: self.birthday.clone(),
        }
    }
}

/// The [`wire_width`] of every string in an argument's value.
fn strings_wire_width(value: &Value) -> usize {
    match value {
        Value::String(text) => wire_width(text),
        Value::Array(items) => items.iter().map(strings_wire_width).sum(),
        _ => 0,
    }
}

/// Update's arguments.
#[derive(Clone, Debug, PartialEq)]
struct UpdateArgs {
    id: String,
    name: Option<NameChange>,
    add_books: Vec<String>,
    remove_books: Vec<String>,
    /// The one entry change, and its list.
    entry: Option<(EntryList, EntryChange)>,
}

impl UpdateArgs {
    const KEYS: &[&str] = &[
        "id",
        "name",
        "given",
        "surname",
        "add_address_books",
        "remove_address_books",
        "list",
        "key",
        "remove",
        "value",
        "label",
        "contexts",
        "features",
        "pref",
        "kind",
        "service",
        "user",
        "country_code",
        ACCOUNT_ARG,
    ];

    // TODO(contacts-write-flat-arguments-limit-what-is-set): one entry per
    // call, as flat scalars, because the kit's argument schema has no object
    // type.
    fn parse(map: &Args) -> Result<UpdateArgs, String> {
        let id = token(map, "id")?.ok_or("`id` is required")?;
        let name = name_change(map)?;
        let books = |key| id_list(map, key, LIST_MAX, "address book ids");
        let add_books = books("add_address_books")?.unwrap_or_default();
        let remove_books = books("remove_address_books")?.unwrap_or_default();
        if let Some(both) = add_books.iter().find(|book| remove_books.contains(book)) {
            return Err(format!(
                "`add_address_books` and `remove_address_books` both name {both:?}"
            ));
        }
        let entry = entry_change(map)?;
        if name.is_none() && add_books.is_empty() && remove_books.is_empty() && entry.is_none() {
            return Err(
                "the call changes nothing: give `name`, `given`, `surname`, \
                        `add_address_books`, `remove_address_books` or `list`"
                    .to_owned(),
            );
        }
        Ok(UpdateArgs {
            id,
            name,
            add_books,
            remove_books,
            entry,
        })
    }

    /// What the call changes, as its answer's `updated` names them.
    fn changed(&self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.name.is_some() {
            changed.push("name");
        }
        if !self.add_books.is_empty() || !self.remove_books.is_empty() {
            changed.push("address_books");
        }
        if let Some((list, _)) = &self.entry {
            changed.push(list_name(*list));
        }
        changed
    }

    /// The properties the read asks for: the list's, and `name` when the
    /// call changes it.
    fn read_properties(&self) -> Vec<&'static str> {
        let mut properties = vec!["id"];
        if let Some((list, _)) = &self.entry {
            properties.push(list.property());
        }
        if self.name.is_some() {
            properties.push("name");
        }
        properties
    }

    // TODO(contacts-set-unconfirmed-against-live-fastmail): unconfirmed
    // against a live account that Fastmail accepts these patch paths.
    /// The patch for `card` as the read found it, naming only what the call
    /// changes, and an add's new entry key; or why the change cannot be
    /// written without guessing.
    fn patch(&self, card: &CardKeys) -> Result<(Patch, Option<String>), String> {
        let mut patch = Patch::new();
        if let Some(name) = &self.name {
            patch = name
                .patch(card.name.as_ref(), patch)
                .map_err(name_refusal)?;
        }
        for book in &self.add_books {
            patch = patch.at(&["addressBookIds", book.as_str()], json!(true));
        }
        for book in &self.remove_books {
            patch = patch.at(&["addressBookIds", book.as_str()], Value::Null);
        }
        let Some((list, change)) = &self.entry else {
            return Ok((patch, None));
        };
        change.patch(*list, card.keys(*list), patch).ok_or_else(|| {
            let key = match change {
                EntryChange::Edit { key, .. } | EntryChange::Remove { key } => key.as_str(),
                EntryChange::Add(_) => "",
            };
            format!(
                "contact `{}` has no {} entry with key `{key}`",
                self.id,
                list_name(*list)
            )
        })
    }
}

/// The update's one entry change, as its entry arguments give it; `None`
/// when `list` is absent, which no other entry argument may be without.
fn entry_change(map: &Args) -> Result<Option<(EntryList, EntryChange)>, String> {
    let Some(list) = choice(map, "list", &EntryList::ALL, list_name)? else {
        let given = ["key", "remove"]
            .into_iter()
            .find(|key| map.contains_key(*key))
            .or_else(|| field_given(map));
        if let Some(key) = given {
            return Err(format!("`{key}` requires `list`"));
        }
        return Ok(None);
    };
    let name = list_name(list);
    let rules = ListRules::of(list);
    if let Some(field) = OTHER_FIELD_KEYS
        .iter()
        .find(|field| map.contains_key(**field) && !rules.takes.contains(*field))
    {
        return Err(format!("`{name}` entries do not take `{field}`"));
    }
    let value = match map.get("value") {
        None => None,
        Some(Value::String(text)) => {
            let what = format!("`value` of `{name}`");
            Some(match rules.value {
                Some((max_chars, lines)) => {
                    held_text(&what, text, max_chars, lines)?;
                    EntryValue::Text(text.clone())
                }
                None => EntryValue::Date(date(&what, text)?),
            })
        }
        Some(_) => return Err("`value` must be a string".to_owned()),
    };
    let contexts = member_list(map, "contexts", CONTEXTS)?;
    if let Some(context) = contexts
        .iter()
        .flatten()
        .find(|context| !rules.contexts.contains(&context.as_str()))
    {
        return Err(format!(
            "`contexts` of `{name}` may hold only {}, not `{context}`",
            spelled(rules.contexts)
        ));
    }
    let kind = choice(map, "kind", KINDS, |kind| kind)?;
    if let Some(kind) = kind.filter(|kind| !rules.kinds.contains(kind)) {
        return Err(format!(
            "`kind` of `{name}` must be one of {}, not `{kind}`",
            spelled(rules.kinds)
        ));
    }
    let fields = EntryFields {
        value,
        label: one_line(map, "label", TEXT_MAX_CHARS)?,
        contexts,
        features: member_list(map, "features", FEATURES)?,
        pref: integer(map, "pref", 1, PREF_MAX)?,
        kind: kind.map(str::to_owned),
        service: one_line(map, "service", TEXT_MAX_CHARS)?,
        user: one_line(map, "user", TEXT_MAX_CHARS)?,
        country_code: country_code(map)?,
    };
    let key = token(map, "key")?;
    // TODO(contact-update-entry-removal-unruled): whether an agent may remove
    // a contact's entry, which cannot be undone, awaits the owner's ruling.
    let remove = flag(map, "remove")?;
    let change = match key {
        None if map.contains_key("remove") => return Err("`remove` requires `key`".to_owned()),
        None => {
            let needs = match list {
                EntryList::OnlineServices if fields.value.is_none() && fields.user.is_none() => {
                    Some("`value` or `user`")
                }
                EntryList::OnlineServices => None,
                _ if fields.value.is_none() => Some("`value`"),
                EntryList::Anniversaries if fields.kind.is_none() => Some("`kind`"),
                _ => None,
            };
            if let Some(needs) = needs {
                return Err(format!("an added `{name}` entry needs {needs}"));
            }
            EntryChange::Add(fields)
        }
        Some(key) if remove => {
            if let Some(field) = field_given(map) {
                return Err(format!("`{field}` does not apply when removing an entry"));
            }
            EntryChange::Remove { key }
        }
        Some(key) => {
            if fields == EntryFields::default() {
                return Err(format!(
                    "an edit of a `{name}` entry needs a field to change; to remove the entry, \
                     give `remove: true`"
                ));
            }
            EntryChange::Edit { key, fields }
        }
    };
    Ok(Some((list, change)))
}

/// The first entry field `map` gives: `value`, then [`OTHER_FIELD_KEYS`] in
/// order.
fn field_given(map: &Args) -> Option<&'static str> {
    std::iter::once("value")
        .chain(OTHER_FIELD_KEYS.iter().copied())
        .find(|field| map.contains_key(*field))
}

/// `values` as a refusal lists them: "`a`, `b`".
fn spelled(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `country_code`: two capital letters (ISO 3166-1 alpha-2).
fn country_code(map: &Args) -> Result<Option<String>, String> {
    match map.get("country_code") {
        None => Ok(None),
        Some(Value::String(code))
            if code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()) =>
        {
            Ok(Some(code.clone()))
        }
        Some(_) => {
            Err("`country_code` must be two capital letters, an ISO 3166-1 country code".to_owned())
        }
    }
}

/// A name the patch cannot place, in the caller's terms.
fn name_refusal(refusal: NameRefusal) -> String {
    match refusal {
        NameRefusal::FullHidesParts => "the contact has a full name, which is shown instead of \
             its parts: give `name` as well as `given` or `surname`"
            .to_owned(),
        NameRefusal::Ambiguous(kind) => format!(
            "the contact's name holds more than one `{kind}` part, so which to change cannot be \
             said here: set `name` and edit the parts in Fastmail"
        ),
        NameRefusal::Unplaceable(kind) => format!(
            "the contact's name has no `{kind}` part and its parts are in a set order, so the \
             new part has no place: set `name` and edit the parts in Fastmail"
        ),
    }
}

fn parse(tool: Provided, args: &Value) -> Result<Parsed, String> {
    let keys = match tool {
        Provided::FastmailContactCreate => CreateArgs::KEYS,
        Provided::FastmailContactUpdate => UpdateArgs::KEYS,
    };
    let map = object(args, keys)?;
    let tool = match tool {
        Provided::FastmailContactCreate => ToolArgs::Create(CreateArgs::parse(map)?),
        Provided::FastmailContactUpdate => ToolArgs::Update(Box::new(UpdateArgs::parse(map)?)),
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

/// A new card's uid: the request's pending id, 32 lowercase hex characters
/// the host draws at random, shaped as a version-4 UUID (RFC 9562 section
/// 5.4). The kit calls the id opaque, so any other shape is refused rather
/// than turned into a malformed uid.
fn uid(pending: &str) -> Result<String, Failure> {
    if pending.len() != 32
        || !pending
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(internal(
            "the request's pending id is not 32 lowercase hex characters, the shape a new \
             contact's uid is made from",
        ));
    }
    let mut hex = pending.as_bytes().to_vec();
    hex[12] = b'4';
    let variant = (hex[16] as char).to_digit(16).expect("a hex digit");
    hex[16] = b"89ab"[(variant & 0x3) as usize];
    let hex = String::from_utf8(hex).expect("hex digits are ASCII");
    Ok(format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Create's write: the one new card.
fn create_request(args: &CreateArgs, account_id: &str, uid: String) -> Request {
    let mut request = Request::contacts();
    ContactCardSet {
        account_id,
        if_in_state: None,
        create: vec![(CREATE_ID.to_owned(), args.card(uid))],
        update: Vec::new(),
    }
    .push(&mut request);
    request
}

/// Update's read: the card's keys in the list it changes, and its name.
fn read_request(args: &UpdateArgs, account_id: &str) -> (Request, CallId) {
    let mut request = Request::contacts();
    let properties = args.read_properties();
    let call = ContactCardGet {
        account_id,
        ids: Ids::Listed(vec![args.id.clone()]),
        properties: &properties,
    }
    .push(&mut request);
    (request, call)
}

/// The call id of a write step's `ContactCard/set`, the one call of its
/// request.
fn set_call() -> CallId {
    Request::contacts().push(ContactCardSet::METHOD, Value::Null)
}

fn has_step(tool: Provided, step: u8) -> bool {
    match tool {
        Provided::FastmailContactCreate => matches!(step, SESSION_STEP | CREATE_STEP),
        Provided::FastmailContactUpdate => matches!(step, SESSION_STEP | READ_STEP | UPDATE_STEP),
    }
}

/// The hint on a failure that leaves `tool`'s write at `step` unknown; `None`
/// at a step that writes nothing.
fn write_hint(tool: Provided, step: u8) -> Option<&'static str> {
    match (tool, step) {
        (Provided::FastmailContactCreate, CREATE_STEP) => Some(CREATE_UNKNOWN_HINT),
        (Provided::FastmailContactUpdate, UPDATE_STEP) => Some(UPDATE_UNKNOWN_HINT),
        _ => None,
    }
}

/// What the request does with its nested call's answer: complete, fail, or
/// issue its next step. Every completion is held to the result ceiling; every
/// failure `step` leaves unknown carries the step's write hint.
pub fn advance(state: &InFlight, outcome: Result<Value, NestedError>, settings: &Settings) -> Next {
    if !has_step(state.tool, state.step) {
        return Next::Fail(no_such_step(state.tool, state.step));
    }
    match step(state, outcome, settings) {
        Ok(Next::Complete(value)) => bounded(value, COMPACT_RESULT_CEILING, None),
        Ok(next) => next,
        Err(failure) => Next::Fail(failure.with_hint(write_hint(state.tool, state.step))),
    }
}

/// One step. Fastmail's refusal of the card, which says nothing was changed,
/// is `Ok(Next::Fail)`; an `Err` says whether Fastmail refused the request or
/// the method or left the write's outcome unknown.
fn step(
    state: &InFlight,
    outcome: Result<Value, NestedError>,
    settings: &Settings,
) -> Result<Next, StepFailure> {
    let body = step_answer(state, outcome, None)?;
    let parsed = stored_args(state, parse)?;
    let account = parsed.account.as_deref();
    match (&parsed.tool, state.step) {
        (ToolArgs::Create(args), SESSION_STEP) => {
            let uid = uid(&state.id)?;
            Ok(after_session(
                &body,
                CONTACTS,
                settings,
                account,
                CREATE_STEP,
                |account_id| create_request(args, account_id, uid),
            )?)
        }
        (ToolArgs::Update(args), SESSION_STEP) => Ok(after_session(
            &body,
            CONTACTS,
            settings,
            account,
            READ_STEP,
            |account_id| read_request(args, account_id).0,
        )?),
        (ToolArgs::Create(_), _) => created(body),
        (ToolArgs::Update(args), READ_STEP) => {
            let account_id = derived_str(state, ACCOUNT_KEY)?;
            let (request, new_key) = update_request(args, account_id, body)?;
            let mut derived = Args::from_iter([(ACCOUNT_KEY.to_owned(), json!(account_id))]);
            if let Some(key) = new_key {
                derived.insert(NEW_KEY.to_owned(), json!(key));
            }
            Ok(Next::Call {
                call: api_step(settings, account, UPDATE_STEP, &request),
                derived,
            })
        }
        (ToolArgs::Update(args), _) => updated(state, args, body),
    }
}

/// Create's answer: the new card's id, or Fastmail's refusal of it.
fn created(body: Value) -> Result<Next, StepFailure> {
    let response = Response::parse(body).map_err(jmap_failure)?;
    let set = response
        .result(&set_call())
        .map_err(|error| write_method_failure(DATA, error))?;
    let result = ContactCardSet::result(set).map_err(jmap_failure)?;
    if let Some(created) = result.created.get(CREATE_ID) {
        return Ok(Next::Complete(json!({ "id": created.id })));
    }
    if let Some(error) = result.not_created.get(CREATE_ID) {
        return Ok(Next::Fail(set_failure("contact", CREATE_ID, error)));
    }
    Err(internal(format!(
        "{API_NAME}'s ContactCard/set answered neither the new contact nor its refusal"
    ))
    .into())
}

/// Update's write, from the read's answer `body`: the patch under
/// `ifInState`, and an add's new entry key. A card or key the read does not
/// find, or a name the patch cannot place, is the caller's, and nothing is
/// sent.
fn update_request(
    args: &UpdateArgs,
    account_id: &str,
    body: Value,
) -> Result<(Request, Option<String>), Failure> {
    let response = Response::parse(body).map_err(jmap_failure)?;
    let (_, call) = read_request(args, account_id);
    let got = response.result(&call).map_err(jmap_failure)?;
    let (card_state, got) = CardKeys::get(got).map_err(jmap_failure)?;
    let card = the_one(got, &args.id, card_id, ContactCardGet::METHOD, "contact")?;
    let (patch, new_key) = args.patch(&card).map_err(invalid)?;
    let mut request = Request::contacts();
    ContactCardSet {
        account_id,
        if_in_state: Some(&card_state),
        create: Vec::new(),
        update: vec![(args.id.clone(), patch)],
    }
    .push(&mut request);
    Ok((request, new_key))
}

fn card_id(card: &CardKeys) -> &str {
    &card.id
}

/// Update's answer: what changed, or Fastmail's refusal. A state that moved
/// since the read is refused before the server changes anything.
fn updated(state: &InFlight, args: &UpdateArgs, body: Value) -> Result<Next, StepFailure> {
    let response = Response::parse(body).map_err(jmap_failure)?;
    let set = response.result(&set_call()).map_err(|error| match &error {
        JmapError::Method { kind, .. } if kind == "stateMismatch" => {
            StepFailure::Refused(internal(STATE_MISMATCH))
        }
        _ => write_method_failure(DATA, error),
    })?;
    let result = ContactCardSet::result(set).map_err(jmap_failure)?;
    if result.updated.contains(&args.id) {
        let mut out = Args::new();
        out.insert("id".into(), json!(args.id));
        out.insert("updated".into(), json!(args.changed()));
        if let Some((_, EntryChange::Add(_))) = &args.entry {
            out.insert("key".into(), json!(derived_str(state, NEW_KEY)?));
        }
        return Ok(Next::Complete(Value::Object(out)));
    }
    if let Some(error) = result.not_updated.get(&args.id) {
        return Ok(Next::Fail(set_failure("contact", &args.id, error)));
    }
    Err(internal(format!(
        "{API_NAME}'s ContactCard/set answered neither contact `{}`'s update nor its refusal",
        args.id
    ))
    .into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use brenn_provider_kit::activation::BACKLOG_NS;
    use brenn_provider_kit::testutil::{Answered, MemoryHost, failure, http, nested, truncated};
    use brenn_provider_kit::{
        Bound, CALL_BUDGET, Delivered, ErrorKind, Input, Probes, ToolBounds, check, hold_probes,
        hold_required, input_schema, probes,
    };
    use fastmail_jmap::session::CORE;
    use fastmail_provider::args::ID_MAX_CHARS;
    use fastmail_provider::failure::{FORBIDDEN_CHANGE, read_only_token};
    use fastmail_provider::testutil::{
        ACCOUNT_PATTERN, HOST_ARGS_CAP_BYTES, account_probes, completed, incoming, jmap, row,
        session_with, settings,
    };

    use super::*;

    const BIRTHDAY_PATTERN: &str = r"^(\d{4}(-\d{2}(-\d{2})?)?|--\d{2}-\d{2})$";
    const COUNTRY_PATTERN: &str = "^[A-Z]{2}$";
    const LIST_NAMES: [&str; 9] = [
        "emails",
        "phones",
        "online_services",
        "addresses",
        "organizations",
        "titles",
        "nicknames",
        "anniversaries",
        "notes",
    ];
    /// A pending id as the host draws it, and the uid made from it.
    const PENDING_ID: &str = "0123456789abcdef0123456789abcdef";
    const PENDING_UID: &str = "urn:uuid:01234567-89ab-4def-8123-456789abcdef";

    const CREATE: Provided = Provided::FastmailContactCreate;
    const UPDATE: Provided = Provided::FastmailContactUpdate;

    fn answered(state: &InFlight, outcome: Result<Value, NestedError>) -> Next {
        advance(state, outcome, &settings())
    }

    /// `tool`'s row at `step` with `args`, under a pending id the host could
    /// have drawn.
    fn at(tool: Provided, args: Value, step: u8) -> InFlight {
        let derived = match step {
            SESSION_STEP => json!({}),
            _ => json!({ ACCOUNT_KEY: "a1" }),
        };
        let mut state = row(tool, args, step, derived);
        state.id = PENDING_ID.to_owned();
        state
    }

    fn admitted_args(tool: Provided) -> Value {
        match tool {
            Provided::FastmailContactCreate => json!({ "address_book": "b1", "name": "Alice" }),
            Provided::FastmailContactUpdate => json!({ "id": "c1", "name": "Alice" }),
        }
    }

    /// The call the session step issues for `tool` with `args`.
    fn after_the_session(tool: Provided, args: Value) -> Call {
        let next = answered(&at(tool, args, SESSION_STEP), session_with(&[CONTACTS]));
        let Next::Call { call, derived } = next else {
            panic!("{tool:?}: the session step issues the next call: {next:?}");
        };
        assert_eq!(
            derived,
            Args::from_iter([(ACCOUNT_KEY.into(), json!("a1"))])
        );
        call
    }

    fn body(call: &Call) -> Value {
        serde_json::from_str(brenn_provider_kit::testutil::text_body(&call.api)).unwrap()
    }

    /// A read's answer holding `card` at state `s7`.
    fn read_answer(card: Value) -> Result<Value, NestedError> {
        jmap(json!([["ContactCard/get",
            { "accountId": "a1", "state": "s7", "list": [card], "notFound": [] }, "c0"]]))
    }

    /// What the read step does with `args` and the read's `card`.
    fn after_the_read(args: Value, card: Value) -> Next {
        answered(&at(UPDATE, args, READ_STEP), read_answer(card))
    }

    /// The patch an update of `c1` with `args` posts after reading `card`,
    /// and the write's `derived`.
    fn posted_patch(args: Value, card: Value) -> (Value, Args) {
        let Next::Call { call, derived } = after_the_read(args, card) else {
            panic!("the read step issues the write");
        };
        assert_eq!(call.step, UPDATE_STEP);
        let body = body(&call);
        let set = &body["methodCalls"][0];
        assert_eq!(set[0], json!("ContactCard/set"));
        assert_eq!(set[1]["ifInState"], json!("s7"));
        assert_eq!(set[1]["update"].as_object().unwrap().len(), 1);
        (set[1]["update"]["c1"].clone(), derived)
    }

    fn set_answer(args: Value) -> Result<Value, NestedError> {
        jmap(json!([["ContactCard/set", args, "c0"]]))
    }

    fn text(max_chars: usize) -> Bound<'static> {
        Bound::Text {
            max_chars,
            format: None,
        }
    }

    fn array(max: usize, item: Bound<'static>) -> Bound<'static> {
        Bound::Array {
            min: 1,
            max,
            unique: true,
            item: Box::new(item),
        }
    }

    fn bound(key: &str) -> Bound<'static> {
        let token = Bound::Token {
            max_chars: ID_MAX_CHARS,
        };
        match key {
            "address_book" | "id" | "key" => token,
            "name" | "given" | "surname" | "label" | "service" | "user" => text(TEXT_MAX_CHARS),
            "emails" | "phones" | "organizations" | "titles" | "nicknames" => {
                array(LIST_MAX, text(TEXT_MAX_CHARS))
            }
            "online_services" => array(LIST_MAX, text(LONG_TEXT_MAX_CHARS)),
            "addresses" => array(SHORT_LIST_MAX, text(LONG_TEXT_MAX_CHARS)),
            "notes" => array(SHORT_LIST_MAX, text(NOTE_MAX_CHARS)),
            "birthday" => Bound::Pattern(BIRTHDAY_PATTERN.to_owned()),
            "add_address_books" | "remove_address_books" => array(LIST_MAX, token),
            "list" => Bound::Enum(LIST_NAMES.to_vec()),
            "remove" => Bound::Bool,
            "value" => text(NOTE_MAX_CHARS),
            "contexts" => array(CONTEXTS.len(), Bound::Enum(CONTEXTS.to_vec())),
            "features" => array(FEATURES.len(), Bound::Enum(FEATURES.to_vec())),
            "pref" => Bound::Integer {
                min: 1,
                max: PREF_MAX,
            },
            "kind" => Bound::Enum(KINDS.to_vec()),
            "country_code" => Bound::Pattern(COUNTRY_PATTERN.to_owned()),
            ACCOUNT_ARG => Bound::Pattern(ACCOUNT_PATTERN.to_owned()),
            _ => panic!("`{key}` has no bound here"),
        }
    }

    fn bounds(tool: Provided) -> ToolBounds<'static> {
        let (keys, required): (&[&str], &'static [&'static str]) = match tool {
            Provided::FastmailContactCreate => (CreateArgs::KEYS, &["address_book"]),
            Provided::FastmailContactUpdate => (UpdateArgs::KEYS, &["id"]),
        };
        ToolBounds {
            properties: keys.iter().map(|key| (*key, bound(key))).collect(),
            required,
        }
    }

    /// The kit's probes of a text bound, and the control characters its
    /// parser refuses beside them: every one on one line, and every one but a
    /// line break where breaks are allowed.
    fn text_probes(bound: &Bound, lines: Lines) -> Probes {
        let mut probes = probes(bound).expect("a text bound has probes");
        probes
            .refused
            .extend(["a\u{1}b", "a\tb", "a\u{85}b"].map(|p| json!(p)));
        match lines {
            Lines::One => probes.refused.push(json!("a\nb")),
            Lines::Many => probes.admitted.push(json!("a\r\nb")),
        }
        probes
    }

    fn list_probes(max_items: usize, max_chars: usize, lines: Lines) -> Probes {
        let items = |n: usize| json!((0..n).map(|i| format!("v{i}")).collect::<Vec<_>>());
        let mut probes = Probes {
            admitted: vec![items(1), items(max_items), json!(["é".repeat(max_chars)])],
            refused: vec![
                json!([]),
                items(max_items + 1),
                json!(["v", "v"]),
                json!([""]),
                json!([" \t"]),
                json!(["é".repeat(max_chars + 1)]),
                json!([1]),
                json!("v"),
                json!(["a\u{1}b"]),
            ],
        };
        match lines {
            Lines::One => probes.refused.push(json!(["a\nb"])),
            Lines::Many => probes.admitted.push(json!(["a\nb"])),
        }
        probes
    }

    fn member_probes(values: &[&str]) -> Probes {
        Probes {
            admitted: vec![json!([values[0]]), json!(values)],
            refused: vec![
                json!([]),
                json!([values[0], values[0]]),
                json!(["x"]),
                json!([1]),
                json!(values[0]),
            ],
        }
    }

    fn book_probes() -> Probes {
        let ids = |n: usize| json!((0..n).map(|i| format!("b{i}")).collect::<Vec<_>>());
        Probes {
            admitted: vec![ids(1), ids(LIST_MAX), json!(["b".repeat(ID_MAX_CHARS)])],
            refused: vec![
                json!([]),
                ids(LIST_MAX + 1),
                json!(["b", "b"]),
                json!(["a b"]),
                json!(["b".repeat(ID_MAX_CHARS + 1)]),
                json!("b"),
            ],
        }
    }

    fn pattern_probes(admitted: &[&str], refused: &[&str]) -> Probes {
        Probes {
            admitted: admitted.iter().map(|p| json!(p)).collect(),
            refused: refused.iter().map(|p| json!(p)).chain([json!(1)]).collect(),
        }
    }

    /// The probes holding `key` of `tool`, each with the arguments it is
    /// probed on: one an update's entry rules admit the probe on.
    fn probes_of(tool: Provided, key: &str) -> Vec<(Value, Probes)> {
        let on = |base: Value, probes: Probes| vec![(base, probes)];
        let edit = |list: &str| {
            let value = if list == "anniversaries" { "1990" } else { "v" };
            json!({ "id": "c1", "list": list, "key": "k1", "value": value })
        };
        let removal = json!({ "id": "c1", "list": "notes", "key": "k1", "remove": true });
        let kit = |key: &str| probes(&bound(key)).expect("a kit bound");
        let refused_kinds = || vec![json!("titlerolebirthdeathwedding-"), json!(1)];
        match (tool, key) {
            (_, ACCOUNT_ARG) => on(admitted_args(tool), account_probes()),
            (_, "name" | "given" | "surname") => {
                on(admitted_args(tool), text_probes(&bound(key), Lines::One))
            }
            (Provided::FastmailContactCreate, "birthday") => on(
                admitted_args(tool),
                pattern_probes(
                    &["1990", "1990-04", "1990-04-15", "--04-15", "1990-12-31"],
                    &[
                        "90",
                        "1990-4",
                        "1990-13",
                        "1990-04-32",
                        "--04",
                        "--00-15",
                        "",
                        "x",
                    ],
                ),
            ),
            (Provided::FastmailContactCreate, "address_book") => on(admitted_args(tool), kit(key)),
            (Provided::FastmailContactCreate, list) => {
                let (entry, max_items) = CREATE_LISTS
                    .into_iter()
                    .find(|(entry, _)| list_name(*entry) == list)
                    .expect("a create list");
                let (max_chars, lines) = ListRules::of(entry).value.expect("a text list");
                on(
                    admitted_args(tool),
                    list_probes(max_items, max_chars, lines),
                )
            }
            (_, "id") => on(admitted_args(tool), kit(key)),
            (_, "add_address_books" | "remove_address_books") => {
                on(admitted_args(tool), book_probes())
            }
            (_, "list") => on(removal, kit(key)),
            (_, "key") => on(
                json!({ "id": "c1", "list": "notes", "value": "x" }),
                kit(key),
            ),
            (_, "remove") => on(
                removal,
                Probes {
                    admitted: vec![json!(true)],
                    refused: vec![json!("true"), json!(1)],
                },
            ),
            (_, "value") => on(
                json!({ "id": "c1", "list": "notes", "value": "v" }),
                text_probes(&bound(key), Lines::Many),
            ),
            (_, "label") => on(edit("emails"), text_probes(&bound(key), Lines::One)),
            (_, "service" | "user") => on(
                edit("online_services"),
                text_probes(&bound(key), Lines::One),
            ),
            (_, "contexts") => on(edit("addresses"), member_probes(CONTEXTS)),
            (_, "features") => on(edit("phones"), member_probes(FEATURES)),
            (_, "pref") => on(edit("emails"), kit(key)),
            (_, "kind") => vec![
                (
                    edit("titles"),
                    Probes {
                        admitted: vec![json!("title"), json!("role")],
                        refused: refused_kinds(),
                    },
                ),
                (
                    edit("anniversaries"),
                    Probes {
                        admitted: vec![json!("birth"), json!("death"), json!("wedding")],
                        refused: refused_kinds(),
                    },
                ),
            ],
            (_, "country_code") => on(
                edit("addresses"),
                pattern_probes(&["US", "GB"], &["us", "USA", "U", "U1", "É1", ""]),
            ),
            _ => panic!("`{key}` of {tool:?} has no probes"),
        }
    }

    #[test]
    fn every_tools_schema_states_what_its_parser_enforces() {
        let path =
            std::env::var("FASTMAIL_CONTACTS_WRITE_SPEC").expect("FASTMAIL_CONTACTS_WRITE_SPEC");
        let spec = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        for tool in Provided::ALL {
            let name = tool.name();
            let bounds = bounds(tool);
            if let Err(error) = check(&input_schema(&spec, name), &bounds) {
                panic!("`{name}`'s input_schema: {error}");
            }
            let plan_with = |args: &Value| plan(tool, args, &settings()).map(drop);
            for key in bounds.required {
                hold_required(name, key, &admitted_args(tool), plan_with);
            }
            for (key, _) in &bounds.properties {
                for (base, probes) in probes_of(tool, key) {
                    hold_probes(name, key, &probes, &base, plan_with);
                }
            }
        }
    }

    #[test]
    fn every_tool_asks_for_the_session_then_writes_or_reads_for_the_account() {
        for tool in Provided::ALL {
            let mut args = admitted_args(tool);
            args["account"] = json!("alice-work");
            let call = plan(tool, &args, &settings()).unwrap();
            assert_eq!(call.step, SESSION_STEP);
            assert_eq!(call.api.path, "/jmap/session");
            assert_eq!(call.api.account.as_deref(), Some("alice-work"));
            let next = after_the_session(tool, args);
            assert_eq!(next.api.account.as_deref(), Some("alice-work"));
            assert_eq!(
                (next.api.method.as_str(), next.api.path.as_str()),
                ("POST", "/jmap/api")
            );
            assert_eq!(body(&next)["using"], json!([CORE, CONTACTS]));
        }
        let read = after_the_session(
            UPDATE,
            json!({ "id": "c1", "surname": "Example", "list": "online_services",
                    "value": "https://example.com/@alice" }),
        );
        assert_eq!(read.step, READ_STEP);
        assert_eq!(
            body(&read)["methodCalls"],
            json!([["ContactCard/get", { "accountId": "a1", "ids": ["c1"],
                "properties": ["id", "onlineServices", "name"] }, "c0"]])
        );
        let read = after_the_session(UPDATE, json!({ "id": "c1", "add_address_books": ["b2"] }));
        assert_eq!(
            body(&read)["methodCalls"][0][1]["properties"],
            json!(["id"])
        );
    }

    #[test]
    fn create_posts_one_card_with_a_uid_from_the_pending_id() {
        let call = after_the_session(
            CREATE,
            json!({ "address_book": "b1", "name": "Alice Example", "given": "Alice",
                    "surname": "Example", "emails": ["alice@example.com"],
                    "phones": ["+1 555 0100", "+1 555 0101"], "online_services": ["https://example.com/@alice"],
                    "addresses": ["1 Main St\nSpringfield"], "organizations": ["Example, Inc."],
                    "titles": ["Engineer"], "nicknames": ["Al"], "notes": ["Met at a \"conference\""],
                    "birthday": "1990-04-15" }),
        );
        assert_eq!(call.step, CREATE_STEP);
        assert_eq!(
            body(&call)["methodCalls"],
            json!([["ContactCard/set", { "accountId": "a1", "create": { "contact": {
                "@type": "Card", "version": "1.0", "uid": PENDING_UID,
                "addressBookIds": { "b1": true },
                "name": { "full": "Alice Example", "components": [
                    { "kind": "given", "value": "Alice" },
                    { "kind": "surname", "value": "Example" }] },
                "emails": { "k1": { "address": "alice@example.com" } },
                "phones": { "k1": { "number": "+1 555 0100" }, "k2": { "number": "+1 555 0101" } },
                "onlineServices": { "k1": { "uri": "https://example.com/@alice" } },
                "addresses": { "k1": { "full": "1 Main St\nSpringfield" } },
                "organizations": { "k1": { "name": "Example, Inc." } },
                "titles": { "k1": { "name": "Engineer" } },
                "nicknames": { "k1": { "name": "Al" } },
                "notes": { "k1": { "note": "Met at a \"conference\"" } },
                "anniversaries": { "k1": { "kind": "birth", "date": {
                    "@type": "PartialDate", "year": 1990, "month": 4, "day": 15 } } },
            } } }, "c0"]])
        );
        for (pending, made) in [
            (
                "ffffffffffffffffffffffffffffffff",
                "ffffffff-ffff-4fff-bfff-ffffffffffff",
            ),
            (
                "00000000000000000000000000000000",
                "00000000-0000-4000-8000-000000000000",
            ),
            (
                "0123456789abcdef4567c9abcdef0123",
                "01234567-89ab-4def-8567-c9abcdef0123",
            ),
        ] {
            assert_eq!(uid(pending).unwrap(), format!("urn:uuid:{made}"));
        }
        for pending in [
            "r".repeat(32),
            "0".repeat(31),
            "0".repeat(33),
            "A".repeat(32),
        ] {
            let mut state = at(CREATE, admitted_args(CREATE), SESSION_STEP);
            state.id = pending.clone();
            let Next::Fail(failed) = answered(&state, session_with(&[CONTACTS])) else {
                panic!("{pending}: a malformed pending id is refused");
            };
            assert_eq!(failed.kind, ErrorKind::Internal);
            assert!(failed.detail.contains("32 lowercase hex"), "{failed:?}");
        }
    }

    #[test]
    fn create_answers_the_new_id_or_fastmails_refusal() {
        let state = at(CREATE, admitted_args(CREATE), CREATE_STEP);
        assert_eq!(
            completed(answered(
                &state,
                set_answer(json!({ "accountId": "a1", "created": { "contact": { "id": "c9" } } }))
            )),
            json!({ "id": "c9" })
        );
        let refused = |error: Value| {
            answered(
                &state,
                set_answer(json!({ "accountId": "a1", "notCreated": { "contact": error } })),
            )
        };
        assert_eq!(
            refused(json!({ "type": "invalidProperties", "properties": ["addressBookIds"] })),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "Fastmail refused `contact`: invalidProperties (addressBookIds)"
            ))
        );
        assert_eq!(
            refused(json!({ "type": "forbidden" })),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                &format!("Fastmail refused `contact`: forbidden; {FORBIDDEN_CHANGE}")
            ))
        );
        assert_eq!(
            answered(&state, set_answer(json!({ "accountId": "a1" }))),
            Next::Fail(internal(format!(
                "Fastmail's ContactCard/set answered neither the new contact nor its refusal; \
                 {CREATE_UNKNOWN_HINT}"
            )))
        );
        for kind in ["accountReadOnly", "forbidden"] {
            assert_eq!(
                answered(
                    &state,
                    jmap(json!([["error", { "type": kind, "description": "why" }, "c0"]]))
                ),
                Next::Fail(internal(format!(
                    "Fastmail: ContactCard/set failed: {kind}: why; {}",
                    read_only_token("contacts")
                )))
            );
        }
    }

    #[test]
    fn every_entry_field_is_an_update_argument_and_each_lists_fields_are_entry_fields() {
        for field in OTHER_FIELD_KEYS.iter().chain(&["key", "remove", "value"]) {
            assert!(UpdateArgs::KEYS.contains(field), "{field}");
        }
        for list in EntryList::ALL {
            for field in ListRules::of(list).takes {
                assert!(OTHER_FIELD_KEYS.contains(field), "{list:?}: {field}");
            }
        }
    }

    #[test]
    fn create_refuses_an_empty_card_and_a_bad_date() {
        let refused = |args: Value| plan(CREATE, &args, &settings()).unwrap_err();
        assert!(refused(json!({ "address_book": "b1" })).starts_with("give at least one of"));
        assert_eq!(
            refused(json!({ "address_book": "b1", "birthday": "1990-02-00" })),
            "`birthday` must be a date: 1990-04-15, 1990-04, 1990 or --04-15, month 1 to 12, \
             day 1 to 31"
        );
        assert_eq!(
            refused(json!({ "address_book": "b1", "notes": ["a\u{7}b"] })),
            "each of `notes` must not hold a control character but a line break"
        );
    }

    #[test]
    fn update_holds_each_entry_change_to_its_lists_rules() {
        let refused = |args: Value| plan(UPDATE, &args, &settings()).unwrap_err();
        let admitted = |args: Value| plan(UPDATE, &args, &settings()).map(drop).unwrap();
        for (args, refusal) in [
            (
                json!({ "id": "c1" }),
                "the call changes nothing: give `name`, `given`, `surname`, \
                 `add_address_books`, `remove_address_books` or `list`",
            ),
            (
                json!({ "id": "c1", "name": "A", "key": "k1" }),
                "`key` requires `list`",
            ),
            (json!({ "id": "c1", "pref": 1 }), "`pref` requires `list`"),
            (
                json!({ "id": "c1", "list": "emails" }),
                "an added `emails` entry needs `value`",
            ),
            (
                json!({ "id": "c1", "list": "online_services", "label": "x" }),
                "an added `online_services` entry needs `value` or `user`",
            ),
            (
                json!({ "id": "c1", "list": "anniversaries", "value": "1990" }),
                "an added `anniversaries` entry needs `kind`",
            ),
            (
                json!({ "id": "c1", "list": "anniversaries", "value": "1990-13",
                        "kind": "birth" }),
                "`value` of `anniversaries` must be a date: 1990-04-15, 1990-04, 1990 or \
                 --04-15, month 1 to 12, day 1 to 31",
            ),
            (
                json!({ "id": "c1", "list": "emails", "value": "a", "remove": true }),
                "`remove` requires `key`",
            ),
            (
                json!({ "id": "c1", "list": "emails", "key": "k1" }),
                "an edit of a `emails` entry needs a field to change; to remove the entry, give \
                 `remove: true`",
            ),
            (
                json!({ "id": "c1", "list": "emails", "key": "k1", "remove": false }),
                "an edit of a `emails` entry needs a field to change; to remove the entry, give \
                 `remove: true`",
            ),
            (
                json!({ "id": "c1", "list": "emails", "key": "k1", "remove": true,
                        "label": "x" }),
                "`label` does not apply when removing an entry",
            ),
            (
                json!({ "id": "c1", "list": "emails", "value": "a", "features": ["voice"] }),
                "`emails` entries do not take `features`",
            ),
            (
                json!({ "id": "c1", "list": "organizations", "key": "k1", "pref": 1 }),
                "`organizations` entries do not take `pref`",
            ),
            (
                json!({ "id": "c1", "list": "notes", "key": "k1", "country_code": "US" }),
                "`notes` entries do not take `country_code`",
            ),
            (
                json!({ "id": "c1", "list": "emails", "value": "a", "contexts": ["billing"] }),
                "`contexts` of `emails` may hold only `private`, `work`, not `billing`",
            ),
            (
                json!({ "id": "c1", "list": "titles", "value": "a", "kind": "birth" }),
                "`kind` of `titles` must be one of `title`, `role`, not `birth`",
            ),
            (
                json!({ "id": "c1", "list": "anniversaries", "key": "k1", "kind": "role" }),
                "`kind` of `anniversaries` must be one of `birth`, `death`, `wedding`, not `role`",
            ),
            (
                json!({ "id": "c1", "list": "emails", "value": "é".repeat(256) }),
                "`value` of `emails` is longer than 255 characters",
            ),
            (
                json!({ "id": "c1", "list": "online_services", "value": "é".repeat(1001) }),
                "`value` of `online_services` is longer than 1000 characters",
            ),
            (
                json!({ "id": "c1", "list": "emails", "value": "a\nb" }),
                "`value` of `emails` must not hold a control character",
            ),
            (
                json!({ "id": "c1", "list": "addresses", "value": "a\tb" }),
                "`value` of `addresses` must not hold a control character but a line break",
            ),
            (
                json!({ "id": "c1", "add_address_books": ["b1", "b2"],
                        "remove_address_books": ["b2"] }),
                "`add_address_books` and `remove_address_books` both name \"b2\"",
            ),
        ] {
            assert_eq!(refused(args.clone()), refusal, "{args}");
        }
        admitted(json!({ "id": "c1", "list": "online_services", "user": "@alice" }));
        admitted(json!({ "id": "c1", "list": "online_services",
                         "value": "https://example.com/@alice" }));
        admitted(
            json!({ "id": "c1", "list": "addresses", "value": "1 Main St\r\nTown",
                         "contexts": ["billing", "delivery"] }),
        );
        admitted(
            json!({ "id": "c1", "list": "anniversaries", "value": "--06-21",
                         "kind": "wedding" }),
        );
        admitted(json!({ "id": "c1", "list": "notes", "key": "k1", "remove": true }));
        admitted(json!({ "id": "c1", "list": "notes", "value": "é".repeat(4000) }));
    }

    /// A card holding content no tool shows: an entry's vendor sub-property,
    /// its label and contexts, and a second entry.
    fn card_with_emails() -> Value {
        json!({ "id": "c1", "emails": {
            "e1": { "@type": "EmailAddress", "address": "alice@example.com", "label": "home",
                    "contexts": { "private": true }, "example.com:verified": true },
            "e2": { "address": "alice@work.example.com", "pref": 1 } } })
    }

    #[test]
    fn an_update_patches_only_what_it_names() {
        // An edit of one sub-property: the entry's label, contexts and vendor
        // member, and the other entry, are not in the patch.
        let (patch, derived) = posted_patch(
            json!({ "id": "c1", "list": "emails", "key": "e1", "value": "alice@example.org" }),
            card_with_emails(),
        );
        assert_eq!(patch, json!({ "emails/e1/address": "alice@example.org" }));
        assert_eq!(
            derived,
            Args::from_iter([(ACCOUNT_KEY.into(), json!("a1"))])
        );
        // An add to a present list: only the new entry, at a key not held.
        let (patch, derived) = posted_patch(
            json!({ "id": "c1", "list": "emails", "value": "al@example.com", "label": "old",
                    "contexts": ["private"], "pref": 3 }),
            card_with_emails(),
        );
        assert_eq!(
            patch,
            json!({ "emails/k1": { "address": "al@example.com", "label": "old",
                                   "contexts": { "private": true }, "pref": 3 } })
        );
        assert_eq!(derived[NEW_KEY], json!("k1"));
        let mut held = card_with_emails();
        held["emails"]["k1"] = json!({ "address": "x@example.com" });
        let (patch, derived) = posted_patch(
            json!({ "id": "c1", "list": "emails", "value": "al@example.com" }),
            held,
        );
        assert_eq!(
            patch,
            json!({ "emails/k2": { "address": "al@example.com" } })
        );
        assert_eq!(derived[NEW_KEY], json!("k2"));
        // An add to an absent list, and to a null one: the whole property.
        for card in [json!({ "id": "c1" }), json!({ "id": "c1", "phones": null })] {
            let (patch, _) = posted_patch(
                json!({ "id": "c1", "list": "phones", "value": "+1 555 0100",
                        "features": ["mobile", "text"] }),
                card,
            );
            assert_eq!(
                patch,
                json!({ "phones": { "k1": { "number": "+1 555 0100",
                                            "features": { "mobile": true, "text": true } } } })
            );
        }
        // A remove: only the entry.
        let (patch, _) = posted_patch(
            json!({ "id": "c1", "list": "emails", "key": "e2", "remove": true }),
            card_with_emails(),
        );
        assert_eq!(patch, json!({ "emails/e2": null }));
        // Address books: only their flags.
        let (patch, _) = posted_patch(
            json!({ "id": "c1", "add_address_books": ["b2"], "remove_address_books": ["b1"] }),
            json!({ "id": "c1" }),
        );
        assert_eq!(
            patch,
            json!({ "addressBookIds/b2": true, "addressBookIds/b1": null })
        );
    }

    #[test]
    fn an_edit_writes_each_field_at_its_lists_sub_property() {
        let edits: [(&str, Value, &[&str]); 9] = [
            (
                "emails",
                json!({ "label": "l", "contexts": ["work"], "pref": 1 }),
                &["address", "label", "contexts", "pref"],
            ),
            (
                "phones",
                json!({ "label": "l", "contexts": ["work"], "features": ["fax"], "pref": 1 }),
                &["number", "label", "contexts", "features", "pref"],
            ),
            (
                "online_services",
                json!({ "service": "s", "user": "u", "label": "l", "contexts": ["work"],
                        "pref": 1 }),
                &["uri", "service", "user", "label", "contexts", "pref"],
            ),
            (
                "addresses",
                json!({ "country_code": "US", "contexts": ["delivery"], "pref": 1 }),
                &[
                    "full",
                    "countryCode",
                    "contexts",
                    "pref",
                    "components",
                    "isOrdered",
                    "defaultSeparator",
                ],
            ),
            ("organizations", json!({}), &["name"]),
            ("titles", json!({ "kind": "role" }), &["name", "kind"]),
            (
                "nicknames",
                json!({ "contexts": ["private"], "pref": 1 }),
                &["name", "contexts", "pref"],
            ),
            (
                "anniversaries",
                json!({ "kind": "death" }),
                &["date", "kind"],
            ),
            ("notes", json!({}), &["note"]),
        ];
        for (list, fields, subs) in edits {
            let mut args = fields;
            args["id"] = json!("c1");
            args["list"] = json!(list);
            args["key"] = json!("x1");
            args["value"] = json!(if list == "anniversaries" {
                "--06-21"
            } else {
                "v"
            });
            let property = EntryList::ALL
                .into_iter()
                .find(|entry| list_name(*entry) == list)
                .unwrap()
                .property();
            let card = json!({ "id": "c1", property: { "x1": {}, "x2": {} } });
            let (patch, _) = posted_patch(args, card);
            let paths: BTreeSet<&str> = patch
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let expected: Vec<String> = subs
                .iter()
                .map(|sub| format!("{property}/x1/{sub}"))
                .collect();
            assert_eq!(
                paths,
                expected.iter().map(String::as_str).collect(),
                "{list}"
            );
        }
    }

    /// A name holding what no tool sets: `sortAs`, a vendor member, title and
    /// credential parts, and a part's `phonetic`.
    fn card_with_a_name() -> Value {
        json!({ "id": "c1", "name": {
            "full": "Dr. Alice Example, PhD", "sortAs": { "surname": "Example" },
            "isOrdered": true, "example.com:style": "formal",
            "components": [
                { "kind": "title", "value": "Dr." },
                { "kind": "given", "value": "Alice" },
                { "kind": "surname", "value": "Example", "phonetic": "ig-zam-pul" },
                { "kind": "credential", "value": "PhD" }] } })
    }

    #[test]
    fn a_name_change_patches_the_full_text_and_the_parts_it_names() {
        let (patch, _) = posted_patch(
            json!({ "id": "c1", "name": "Dr. Alice Sample, PhD", "surname": "Sample" }),
            card_with_a_name(),
        );
        assert_eq!(
            patch,
            json!({ "name/full": "Dr. Alice Sample, PhD", "name/components": [
                { "kind": "title", "value": "Dr." },
                { "kind": "given", "value": "Alice" },
                { "kind": "surname", "value": "Sample", "phonetic": "ig-zam-pul" },
                { "kind": "credential", "value": "PhD" }] })
        );
        let (patch, _) = posted_patch(json!({ "id": "c1", "name": "Al" }), card_with_a_name());
        assert_eq!(patch, json!({ "name/full": "Al" }));
        let (patch, _) = posted_patch(
            json!({ "id": "c1", "name": "Alice Example", "given": "Alice" }),
            json!({ "id": "c1" }),
        );
        assert_eq!(
            patch,
            json!({ "name": { "full": "Alice Example",
                              "components": [{ "kind": "given", "value": "Alice" }] } })
        );
        let (patch, _) = posted_patch(
            json!({ "id": "c1", "surname": "Example" }),
            json!({ "id": "c1", "name": { "full": "",
                "components": [{ "kind": "given", "value": "Alice" }] } }),
        );
        assert_eq!(
            patch,
            json!({ "name/components": [{ "kind": "given", "value": "Alice" },
                                        { "kind": "surname", "value": "Example" }] })
        );
    }

    #[test]
    fn a_change_the_card_cannot_take_without_guessing_is_refused_before_writing() {
        let refused = |args: Value, card: Value, why: String| {
            assert_eq!(
                after_the_read(args.clone(), card),
                Next::Fail(failure(ErrorKind::InvalidArgs, &why)),
                "{args}"
            );
        };
        refused(
            json!({ "id": "c1", "surname": "Sample" }),
            card_with_a_name(),
            name_refusal(NameRefusal::FullHidesParts),
        );
        let twice = json!({ "id": "c1", "name": { "components": [
            { "kind": "given", "value": "Alice" }, { "kind": "given", "value": "Beth" }] } });
        refused(
            json!({ "id": "c1", "given": "Ann" }),
            twice,
            name_refusal(NameRefusal::Ambiguous("given")),
        );
        let ordered = json!({ "id": "c1", "name": { "isOrdered": true,
            "components": [{ "kind": "given", "value": "Alice" }] } });
        refused(
            json!({ "id": "c1", "surname": "Example" }),
            ordered,
            name_refusal(NameRefusal::Unplaceable("surname")),
        );
        refused(
            json!({ "id": "c1", "list": "emails", "key": "e9", "label": "x" }),
            card_with_emails(),
            "contact `c1` has no emails entry with key `e9`".to_owned(),
        );
        refused(
            json!({ "id": "c1", "list": "phones", "key": "e1", "remove": true }),
            card_with_emails(),
            "contact `c1` has no phones entry with key `e1`".to_owned(),
        );
        let state = at(UPDATE, admitted_args(UPDATE), READ_STEP);
        let unknown = jmap(json!([["ContactCard/get",
            { "accountId": "a1", "state": "s7", "list": [], "notFound": ["c1"] }, "c0"]]));
        assert_eq!(
            answered(&state, unknown),
            Next::Fail(failure(ErrorKind::InvalidArgs, "no contact with id `c1`"))
        );
        let stateless = jmap(json!([["ContactCard/get",
            { "accountId": "a1", "list": [{ "id": "c1" }], "notFound": [] }, "c0"]]));
        let Next::Fail(failed) = answered(&state, stateless) else {
            panic!("a read without a state fails");
        };
        assert_eq!(failed.kind, ErrorKind::Internal);
        assert!(!failed.detail.contains(UPDATE_UNKNOWN_HINT), "{failed:?}");
    }

    fn at_write(args: Value, new_key: Option<&str>) -> InFlight {
        let mut state = at(UPDATE, args, UPDATE_STEP);
        if let Some(key) = new_key {
            state.derived.insert(NEW_KEY.into(), json!(key));
        }
        state
    }

    #[test]
    fn an_update_answers_what_changed_or_why_nothing_did() {
        let done = set_answer(json!({ "accountId": "a1", "updated": { "c1": null } }));
        let args = json!({ "id": "c1", "name": "A", "add_address_books": ["b2"],
                           "list": "emails", "value": "a@example.com" });
        assert_eq!(
            completed(answered(&at_write(args, Some("k3")), done.clone())),
            json!({ "id": "c1", "updated": ["name", "address_books", "emails"], "key": "k3" })
        );
        let args = json!({ "id": "c1", "list": "notes", "key": "n1", "remove": true });
        assert_eq!(
            completed(answered(&at_write(args, None), done)),
            json!({ "id": "c1", "updated": ["notes"] })
        );
        let state = at_write(admitted_args(UPDATE), None);
        let method_error =
            |kind: &str| jmap(json!([["error", { "type": kind, "description": "why" }, "c0"]]));
        assert_eq!(
            answered(&state, method_error("stateMismatch")),
            Next::Fail(internal(STATE_MISMATCH))
        );
        for kind in ["accountReadOnly", "forbidden"] {
            assert_eq!(
                answered(&state, method_error(kind)),
                Next::Fail(internal(format!(
                    "Fastmail: ContactCard/set failed: {kind}: why; {}",
                    read_only_token("contacts")
                )))
            );
        }
        let refused = |error: Value| {
            answered(
                &state,
                set_answer(json!({ "accountId": "a1", "notUpdated": { "c1": error } })),
            )
        };
        assert_eq!(
            refused(json!({ "type": "invalidPatch", "description": "no such path" })),
            Next::Fail(failure(
                ErrorKind::InvalidArgs,
                "Fastmail refused `c1`: invalidPatch: no such path"
            ))
        );
        assert_eq!(
            refused(json!({ "type": "notFound" })),
            Next::Fail(failure(ErrorKind::InvalidArgs, "no contact with id `c1`"))
        );
        assert_eq!(
            answered(&state, set_answer(json!({ "accountId": "a1" }))),
            Next::Fail(internal(format!(
                "Fastmail's ContactCard/set answered neither contact `c1`'s update nor its \
                 refusal; {UPDATE_UNKNOWN_HINT}"
            )))
        );
    }

    #[test]
    fn every_write_step_answers_a_lost_answer_with_its_hint_and_no_other_step_does() {
        let refusal = "result exceeds [messaging] max_body_bytes";
        let lost = || {
            [
                http(500, "<html>secret</html>"),
                nested("internal", refusal),
                truncated(200, json!(300000), "{"),
                http(204, ""),
                set_answer(json!({ "accountId": "a1", "updated": 1 })),
                jmap(json!([["error", { "type": "serverFail" }, "c0"]])),
            ]
        };
        let refused = || {
            [
                http(400, "{}"),
                http(401, ""),
                http(403, ""),
                http(429, ""),
                jmap(json!([["error", { "type": "accountReadOnly" }, "c0"]])),
                jmap(json!([["error", { "type": "forbidden" }, "c0"]])),
            ]
        };
        for (tool, step, hint) in [
            (CREATE, CREATE_STEP, CREATE_UNKNOWN_HINT),
            (UPDATE, UPDATE_STEP, UPDATE_UNKNOWN_HINT),
        ] {
            assert_eq!(write_hint(tool, step), Some(hint));
            let state = at(tool, admitted_args(tool), step);
            for outcome in lost() {
                let shown = format!("{outcome:?}");
                let Next::Fail(failed) = answered(&state, outcome) else {
                    panic!("{tool:?}: {shown} does not fail");
                };
                assert_eq!(failed.kind, ErrorKind::Internal, "{tool:?}: {failed:?}");
                assert!(
                    failed.detail.ends_with(&format!("; {hint}")),
                    "{tool:?}: {failed:?}"
                );
            }
            for outcome in refused() {
                let Next::Fail(failed) = answered(&state, outcome) else {
                    panic!("{tool:?}: a refusal does not fail");
                };
                assert!(!failed.detail.contains(hint), "{tool:?}: {failed:?}");
            }
        }
        for (tool, step) in [
            (CREATE, SESSION_STEP),
            (UPDATE, SESSION_STEP),
            (UPDATE, READ_STEP),
        ] {
            assert_eq!(write_hint(tool, step), None);
            let state = at(tool, admitted_args(tool), step);
            for outcome in lost() {
                let Next::Fail(failed) = answered(&state, outcome) else {
                    panic!("{tool:?} step {step}: a lost answer does not fail");
                };
                assert!(
                    !failed.detail.contains("before asking again"),
                    "{tool:?} step {step}: {failed:?}"
                );
            }
        }
    }

    #[test]
    fn a_step_a_tool_does_not_have_is_refused_as_such() {
        for (tool, step) in [(CREATE, UPDATE_STEP), (UPDATE, UPDATE_STEP + 1)] {
            let state = at(tool, admitted_args(tool), step);
            assert_eq!(
                answered(&state, jmap(json!([]))),
                Next::Fail(no_such_step(tool, step))
            );
        }
    }

    /// The serialised width of `call`'s `api-request` arguments.
    fn width(call: &Call) -> usize {
        serde_json::to_vec(&call.api).unwrap().len()
    }

    #[test]
    fn the_widest_call_of_every_tool_fits_the_args_cap() {
        let account = "a".repeat(ID_MAX_CHARS);
        let quotes = |n: usize| "\"".repeat(n);
        let items = |n: usize, len: usize| {
            json!(
                (0..n)
                    .map(|i| format!("{i}{}", quotes(len)))
                    .collect::<Vec<_>>()
            )
        };
        // Every list full of quotes, the widest character a one-line text
        // holds, and one note padded until the byte cap binds.
        let create = |pad: usize| {
            let mut notes = items(SHORT_LIST_MAX - 1, 60);
            notes.as_array_mut().unwrap().push(json!(quotes(pad)));
            json!({ "address_book": "b".repeat(ID_MAX_CHARS), "account": account,
                    "name": quotes(TEXT_MAX_CHARS), "given": quotes(TEXT_MAX_CHARS),
                    "surname": quotes(TEXT_MAX_CHARS), "emails": items(LIST_MAX, 60),
                    "phones": items(LIST_MAX, 60), "online_services": items(LIST_MAX, 60),
                    "addresses": items(SHORT_LIST_MAX, 60), "organizations": items(LIST_MAX, 60),
                    "titles": items(LIST_MAX, 60), "nicknames": items(LIST_MAX, 60),
                    "notes": notes, "birthday": "1990-04-15" })
        };
        let pads: Vec<usize> = (1..=NOTE_MAX_CHARS).collect();
        let n = pads.partition_point(|&pad| plan(CREATE, &create(pad), &settings()).is_ok());
        assert!(
            (1..NOTE_MAX_CHARS).contains(&n),
            "the byte cap binds at {n}"
        );
        assert_eq!(
            plan(CREATE, &create(n + 1), &settings()).unwrap_err(),
            "the contact's text is too wide for one request: give fewer or shorter entries"
        );
        let call = after_the_session(CREATE, create(n));
        assert!(
            width(&call) <= HOST_ARGS_CAP_BYTES,
            "create: {} bytes",
            width(&call)
        );
        // An update naming every field at its widest, on the list with the
        // most fields, against a card whose name it rewrites.
        let books = |prefix: &str| {
            json!(
                (0..LIST_MAX)
                    .map(|i| format!("{prefix}{i}{}", "b".repeat(ID_MAX_CHARS - 3)))
                    .collect::<Vec<_>>()
            )
        };
        let id = "i".repeat(ID_MAX_CHARS);
        let key = "k".repeat(ID_MAX_CHARS);
        let args = json!({ "id": id, "account": account, "name": quotes(TEXT_MAX_CHARS),
            "given": quotes(TEXT_MAX_CHARS), "surname": quotes(TEXT_MAX_CHARS),
            "add_address_books": books("a"), "remove_address_books": books("r"),
            "list": "online_services", "key": key, "value": quotes(LONG_TEXT_MAX_CHARS),
            "label": quotes(TEXT_MAX_CHARS), "service": quotes(TEXT_MAX_CHARS),
            "user": quotes(TEXT_MAX_CHARS), "contexts": ["private", "work"], "pref": PREF_MAX });
        let card = json!({ "id": id, "onlineServices": { key.clone(): {} }, "name": {
            "full": "x", "components": [{ "kind": "given", "value": "A" },
                                        { "kind": "surname", "value": "B" }] } });
        let Next::Call { call, .. } = after_the_read(args, card) else {
            panic!("the widest update is written");
        };
        assert!(
            width(&call) <= HOST_ARGS_CAP_BYTES,
            "update: {} bytes",
            width(&call)
        );
    }

    #[test]
    fn an_update_runs_session_then_read_then_write_then_completes_and_excess_waits() {
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
        let args = json!({ "id": "c1", "list": "emails", "value": "al@example.com" });
        activate(
            vec![incoming(&id, "fastmail-contact-update", args.clone())],
            vec![],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.path, "/jmap/session");
        activate(
            vec![],
            vec![deliver(&calls[0].call_id, session_with(&[CONTACTS]))],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert!(
            brenn_provider_kit::testutil::text_body(&calls[0].call).contains("ContactCard/get")
        );
        activate(
            vec![],
            vec![deliver(&calls[0].call_id, read_answer(card_with_emails()))],
        );
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        assert!(
            brenn_provider_kit::testutil::text_body(&calls[0].call)
                .contains("\"ifInState\":\"s7\"")
        );
        activate(
            vec![],
            vec![deliver(
                &calls[0].call_id,
                set_answer(json!({ "accountId": "a1", "updated": { "c1": null } })),
            )],
        );
        let completions = host.take_completions();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, id);
        let Answered::Value(value) = &completions[0].answer else {
            panic!("{completions:?}");
        };
        assert_eq!(
            value,
            &json!({ "id": "c1", "updated": ["emails"], "key": "k1" })
        );
        assert!(host.take_calls().is_empty());

        let requests = (0..=CALL_BUDGET)
            .map(|i| incoming(&format!("{i:032}"), "fastmail-contact-update", args.clone()))
            .collect();
        activate(requests, vec![]);
        assert_eq!(host.take_calls().len(), CALL_BUDGET);
        assert_eq!(host.rows(BACKLOG_NS).len(), 1);
    }
}
