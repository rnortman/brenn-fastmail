//! A contact card as this component answers it: a search's summary and a
//! read's full answer. Every list is capped, with an `_omitted` count when
//! entries were left out, and every string copied out of the card is cut at a
//! bound on its serialized width, so no card can drive an answer past the
//! result ceiling.

use brenn_provider_kit::Args;
use fastmail_jmap::{
    Anniversary, CardEmail, CardName, ContactCard, EntryList, Nickname, Note, OnlineService,
    Organization, Phone, PostalAddress, Title,
};
use fastmail_provider::contacts::list_name;
use fastmail_provider::results::{capped_list, cut_value, cut_width};
use serde_json::{Value, json};

/// Serialized-width bounds on strings copied out of a card, by what the
/// string is.
pub const NOTE_MAX: usize = 4000;
/// A URI, an address's text, an address book's description.
pub const TEXT_MAX: usize = 1000;
/// A display name, a name component's value, a nickname, an organization's,
/// unit's, title's or service's name, a service's user, an email address, a
/// phone number, an address book's name.
pub const NAME_MAX: usize = 400;
/// An id, a uid, a member uid, an address book id, an entry's key, a label.
pub const ID_MAX: usize = 255;
/// A timestamp, an anniversary's date.
pub const TIME_MAX: usize = 64;
/// A kind, a country code, a context or a feature.
pub const KIND_MAX: usize = 32;

/// The most entries of each list a read carries.
const NAME_COMPONENTS_MAX: usize = 20;
const NICKNAMES_MAX: usize = 10;
const ORGANIZATIONS_MAX: usize = 10;
const UNITS_MAX: usize = 5;
const TITLES_MAX: usize = 10;
const EMAILS_MAX: usize = 20;
const PHONES_MAX: usize = 20;
const ONLINE_SERVICES_MAX: usize = 10;
const ADDRESSES_MAX: usize = 10;
const ANNIVERSARIES_MAX: usize = 10;
const NOTES_MAX: usize = 10;
const MEMBERS_MAX: usize = 100;
const ADDRESS_BOOKS_MAX: usize = 20;
/// The most contexts, or features, of one entry.
const SET_MAX: usize = 8;
/// The most address book ids a summary carries.
const SUMMARY_ADDRESS_BOOKS_MAX: usize = 10;

/// `value` cut at `max` at `key`; nothing when absent.
fn put(out: &mut Args, key: &str, value: Option<&str>, max: usize) {
    if let Some(value) = value {
        out.insert(key.into(), cut_value(value, max));
    }
}

/// A list of strings, each cut at `max`, as [`capped_list`] carries it.
fn strings(out: &mut Args, key: &str, items: &[String], cap: usize, max: usize) {
    capped_list(out, key, items, cap, |item| cut_value(item, max));
}

/// `contexts` or `features`: at most `SET_MAX`, each cut at `KIND_MAX`.
fn set(out: &mut Args, key: &str, items: &[String]) {
    strings(out, key, items, SET_MAX, KIND_MAX);
}

fn pref(out: &mut Args, pref: Option<u64>) {
    if let Some(pref) = pref {
        out.insert("pref".into(), json!(pref));
    }
}

fn display_name(card: &ContactCard) -> Option<String> {
    card.name.as_ref().and_then(CardName::display)
}

/// A preferred entry at `key` and `<key>s_omitted` counting the others.
fn first(out: &mut Args, key: &str, value: Option<&str>, count: usize) {
    put(out, key, value, NAME_MAX);
    let omitted = count - usize::from(value.is_some());
    if omitted > 0 {
        out.insert(format!("{key}s_omitted"), json!(omitted));
    }
}

/// A search's entry for `card`: its id, kind and display name, its
/// preferred email and phone, its first named organization, and its address
/// books.
pub fn summary(card: &ContactCard) -> Value {
    let mut out = Args::new();
    out.insert("id".into(), cut_value(&card.id, ID_MAX));
    out.insert("kind".into(), cut_value(&card.kind, KIND_MAX));
    put(&mut out, "name", display_name(card).as_deref(), NAME_MAX);
    let email = card.emails.first().map(|email| email.address.as_str());
    first(&mut out, "email", email, card.emails.len());
    let phone = card.phones.first().map(|phone| phone.number.as_str());
    first(&mut out, "phone", phone, card.phones.len());
    let organization = card
        .organizations
        .iter()
        .find_map(|organization| organization.name.as_deref());
    first(
        &mut out,
        "organization",
        organization,
        card.organizations.len(),
    );
    strings(
        &mut out,
        "address_book_ids",
        &card.address_book_ids,
        SUMMARY_ADDRESS_BOOKS_MAX,
        ID_MAX,
    );
    Value::Object(out)
}

/// A read's answer for `card`: every key present only when the card has the
/// value.
pub fn read(card: &ContactCard) -> Value {
    let mut out = Args::new();
    out.insert("id".into(), cut_value(&card.id, ID_MAX));
    put(&mut out, "uid", card.uid.as_deref(), ID_MAX);
    out.insert("kind".into(), cut_value(&card.kind, KIND_MAX));
    put(&mut out, "created", card.created.as_deref(), TIME_MAX);
    put(&mut out, "updated", card.updated.as_deref(), TIME_MAX);
    put(&mut out, "name", display_name(card).as_deref(), NAME_MAX);
    if let Some(name) = &card.name {
        let components: Vec<_> = name
            .components
            .iter()
            .filter(|component| component.kind != "separator")
            .collect();
        capped_list(
            &mut out,
            "name_components",
            &components,
            NAME_COMPONENTS_MAX,
            |component| {
                json!({
                    "kind": cut_value(&component.kind, KIND_MAX),
                    "value": cut_value(&component.value, NAME_MAX),
                })
            },
        );
    }
    capped_list(
        &mut out,
        list_name(EntryList::Nicknames),
        &card.nicknames,
        NICKNAMES_MAX,
        nickname,
    );
    capped_list(
        &mut out,
        list_name(EntryList::Organizations),
        &card.organizations,
        ORGANIZATIONS_MAX,
        organization,
    );
    capped_list(
        &mut out,
        list_name(EntryList::Titles),
        &card.titles,
        TITLES_MAX,
        |title| self::title(title, &card.organizations),
    );
    capped_list(
        &mut out,
        list_name(EntryList::Emails),
        &card.emails,
        EMAILS_MAX,
        email,
    );
    capped_list(
        &mut out,
        list_name(EntryList::Phones),
        &card.phones,
        PHONES_MAX,
        phone,
    );
    capped_list(
        &mut out,
        list_name(EntryList::OnlineServices),
        &card.online_services,
        ONLINE_SERVICES_MAX,
        online_service,
    );
    capped_list(
        &mut out,
        list_name(EntryList::Addresses),
        &card.addresses,
        ADDRESSES_MAX,
        address,
    );
    capped_list(
        &mut out,
        list_name(EntryList::Anniversaries),
        &card.anniversaries,
        ANNIVERSARIES_MAX,
        anniversary,
    );
    capped_list(
        &mut out,
        list_name(EntryList::Notes),
        &card.notes,
        NOTES_MAX,
        note,
    );
    members(&mut out, &card.members);
    strings(
        &mut out,
        "address_book_ids",
        &card.address_book_ids,
        ADDRESS_BOOKS_MAX,
        ID_MAX,
    );
    Value::Object(out)
}

/// A group's member uids at `members`: at most `MEMBERS_MAX` of those that fit
/// `ID_MAX` whole, and `members_omitted` counting the rest. A uid is shown
/// whole or not at all, because a cut one matches no card in a `uid` search.
fn members(out: &mut Args, members: &[String]) {
    let shown: Vec<Value> = members
        .iter()
        .filter(|uid| !cut_width(uid, ID_MAX).1)
        .take(MEMBERS_MAX)
        .map(|uid| json!(uid))
        .collect();
    let omitted = members.len() - shown.len();
    if !shown.is_empty() {
        out.insert("members".into(), Value::Array(shown));
    }
    if omitted > 0 {
        out.insert("members_omitted".into(), json!(omitted));
    }
}

/// An entry of one of the card's lists, opened with its map key at `key`.
/// The key is shown whole or not at all, because a cut key names no entry.
fn entry(key: &str) -> Args {
    let mut out = Args::new();
    if !cut_width(key, ID_MAX).1 {
        out.insert("key".into(), json!(key));
    }
    out
}

fn nickname(nickname: &Nickname) -> Value {
    let mut out = entry(&nickname.key);
    out.insert("name".into(), cut_value(&nickname.name, NAME_MAX));
    Value::Object(out)
}

fn organization(organization: &Organization) -> Value {
    let mut out = entry(&organization.key);
    put(&mut out, "name", organization.name.as_deref(), NAME_MAX);
    strings(&mut out, "units", &organization.units, UNITS_MAX, NAME_MAX);
    Value::Object(out)
}

/// A title, naming the organization its `organizationId` names when the
/// card has that organization and it has a name.
fn title(title: &Title, organizations: &[Organization]) -> Value {
    let mut out = entry(&title.key);
    out.insert("name".into(), cut_value(&title.name, NAME_MAX));
    out.insert("kind".into(), cut_value(&title.kind, KIND_MAX));
    let named = title.organization_id.as_ref().and_then(|id| {
        organizations
            .iter()
            .find(|organization| &organization.key == id)
            .and_then(|organization| organization.name.as_deref())
    });
    put(&mut out, "organization", named, NAME_MAX);
    Value::Object(out)
}

fn email(email: &CardEmail) -> Value {
    let mut out = entry(&email.key);
    out.insert("address".into(), cut_value(&email.address, NAME_MAX));
    set(&mut out, "contexts", &email.contexts);
    put(&mut out, "label", email.label.as_deref(), ID_MAX);
    pref(&mut out, email.pref);
    Value::Object(out)
}

fn phone(phone: &Phone) -> Value {
    let mut out = entry(&phone.key);
    out.insert("number".into(), cut_value(&phone.number, NAME_MAX));
    set(&mut out, "features", &phone.features);
    set(&mut out, "contexts", &phone.contexts);
    put(&mut out, "label", phone.label.as_deref(), ID_MAX);
    pref(&mut out, phone.pref);
    Value::Object(out)
}

fn online_service(service: &OnlineService) -> Value {
    let mut out = entry(&service.key);
    put(&mut out, "service", service.service.as_deref(), NAME_MAX);
    put(&mut out, "user", service.user.as_deref(), NAME_MAX);
    put(&mut out, "uri", service.uri.as_deref(), TEXT_MAX);
    set(&mut out, "contexts", &service.contexts);
    put(&mut out, "label", service.label.as_deref(), ID_MAX);
    pref(&mut out, service.pref);
    Value::Object(out)
}

fn address(address: &PostalAddress) -> Value {
    let mut out = entry(&address.key);
    put(&mut out, "text", address.display().as_deref(), TEXT_MAX);
    put(
        &mut out,
        "country_code",
        address.country_code.as_deref(),
        KIND_MAX,
    );
    set(&mut out, "contexts", &address.contexts);
    pref(&mut out, address.pref);
    Value::Object(out)
}

fn anniversary(anniversary: &Anniversary) -> Value {
    let mut out = entry(&anniversary.key);
    out.insert("kind".into(), cut_value(&anniversary.kind, KIND_MAX));
    out.insert(
        "date".into(),
        cut_value(&anniversary.date.to_text(), TIME_MAX),
    );
    Value::Object(out)
}

fn note(note: &Note) -> Value {
    let (cut, truncated) = cut_width(&note.note, NOTE_MAX);
    let mut out = entry(&note.key);
    out.insert("note".into(), json!(cut));
    put(&mut out, "created", note.created.as_deref(), TIME_MAX);
    out.insert("truncated".into(), json!(truncated));
    Value::Object(out)
}
