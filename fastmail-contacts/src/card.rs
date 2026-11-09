//! A contact card as this component answers it: a search's summary and a
//! read's full answer. Every list is capped, with an `_omitted` count when
//! entries were left out, and every string copied out of the card is cut at a
//! bound on its serialized width, so no card can drive an answer past the
//! result ceiling.

use brenn_provider_kit::Args;
use fastmail_jmap::{
    Anniversary, AnniversaryDate, CardEmail, CardName, ContactCard, Note, OnlineService,
    Organization, Phone, PostalAddress, Title,
};
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
/// An id, a uid, a member uid, an address book id, a label.
pub const ID_MAX: usize = 255;
/// A timestamp.
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

/// The bytes `c` occupies inside a JSON string as `serde_json` writes it.
fn char_width(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// `text` cut to the longest prefix of whole characters whose serialized
/// widths sum to at most `max`, ending in `…` when cut, and whether it was
/// cut.
pub fn cut_width(text: &str, max: usize) -> (String, bool) {
    let mut width = 0;
    for (at, c) in text.char_indices() {
        width += char_width(c);
        if width > max {
            return (format!("{}…", &text[..at]), true);
        }
    }
    (text.to_owned(), false)
}

fn text(value: &str, max: usize) -> Value {
    json!(cut_width(value, max).0)
}

/// `value` cut at `max` at `key`; nothing when absent.
fn put(out: &mut Args, key: &str, value: Option<&str>, max: usize) {
    if let Some(value) = value {
        out.insert(key.into(), text(value, max));
    }
}

/// At most `cap` of `items`, each through `show`, at `key`, and
/// `<key>_omitted` counting the rest; nothing when `items` is empty.
fn list<T>(out: &mut Args, key: &str, items: &[T], cap: usize, show: impl Fn(&T) -> Value) {
    if items.is_empty() {
        return;
    }
    let shown: Vec<Value> = items.iter().take(cap).map(show).collect();
    out.insert(key.into(), Value::Array(shown));
    if items.len() > cap {
        out.insert(format!("{key}_omitted"), json!(items.len() - cap));
    }
}

/// A list of strings, each cut at `max`, as [`list`] carries it.
fn strings(out: &mut Args, key: &str, items: &[String], cap: usize, max: usize) {
    list(out, key, items, cap, |item| text(item, max));
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
    out.insert("id".into(), text(&card.id, ID_MAX));
    out.insert("kind".into(), text(&card.kind, KIND_MAX));
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
    out.insert("id".into(), text(&card.id, ID_MAX));
    put(&mut out, "uid", card.uid.as_deref(), ID_MAX);
    out.insert("kind".into(), text(&card.kind, KIND_MAX));
    put(&mut out, "created", card.created.as_deref(), TIME_MAX);
    put(&mut out, "updated", card.updated.as_deref(), TIME_MAX);
    put(&mut out, "name", display_name(card).as_deref(), NAME_MAX);
    if let Some(name) = &card.name {
        let components: Vec<_> = name
            .components
            .iter()
            .filter(|component| component.kind != "separator")
            .collect();
        list(
            &mut out,
            "name_components",
            &components,
            NAME_COMPONENTS_MAX,
            |component| {
                json!({
                    "kind": text(&component.kind, KIND_MAX),
                    "value": text(&component.value, NAME_MAX),
                })
            },
        );
    }
    strings(
        &mut out,
        "nicknames",
        &card.nicknames,
        NICKNAMES_MAX,
        NAME_MAX,
    );
    list(
        &mut out,
        "organizations",
        &card.organizations,
        ORGANIZATIONS_MAX,
        organization,
    );
    list(&mut out, "titles", &card.titles, TITLES_MAX, |title| {
        self::title(title, &card.organizations)
    });
    list(&mut out, "emails", &card.emails, EMAILS_MAX, email);
    list(&mut out, "phones", &card.phones, PHONES_MAX, phone);
    list(
        &mut out,
        "online_services",
        &card.online_services,
        ONLINE_SERVICES_MAX,
        online_service,
    );
    list(
        &mut out,
        "addresses",
        &card.addresses,
        ADDRESSES_MAX,
        address,
    );
    list(
        &mut out,
        "anniversaries",
        &card.anniversaries,
        ANNIVERSARIES_MAX,
        anniversary,
    );
    list(&mut out, "notes", &card.notes, NOTES_MAX, note);
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

fn organization(organization: &Organization) -> Value {
    let mut out = Args::new();
    put(&mut out, "name", organization.name.as_deref(), NAME_MAX);
    strings(&mut out, "units", &organization.units, UNITS_MAX, NAME_MAX);
    Value::Object(out)
}

/// A title, naming the organization its `organizationId` names when the
/// card has that organization and it has a name.
fn title(title: &Title, organizations: &[Organization]) -> Value {
    let mut out = Args::new();
    out.insert("name".into(), text(&title.name, NAME_MAX));
    out.insert("kind".into(), text(&title.kind, KIND_MAX));
    let named = title.organization_id.as_ref().and_then(|id| {
        organizations
            .iter()
            .find(|organization| &organization.id == id)
            .and_then(|organization| organization.name.as_deref())
    });
    put(&mut out, "organization", named, NAME_MAX);
    Value::Object(out)
}

fn email(email: &CardEmail) -> Value {
    let mut out = Args::new();
    out.insert("address".into(), text(&email.address, NAME_MAX));
    set(&mut out, "contexts", &email.contexts);
    put(&mut out, "label", email.label.as_deref(), ID_MAX);
    pref(&mut out, email.pref);
    Value::Object(out)
}

fn phone(phone: &Phone) -> Value {
    let mut out = Args::new();
    out.insert("number".into(), text(&phone.number, NAME_MAX));
    set(&mut out, "features", &phone.features);
    set(&mut out, "contexts", &phone.contexts);
    put(&mut out, "label", phone.label.as_deref(), ID_MAX);
    pref(&mut out, phone.pref);
    Value::Object(out)
}

fn online_service(service: &OnlineService) -> Value {
    let mut out = Args::new();
    put(&mut out, "service", service.service.as_deref(), NAME_MAX);
    put(&mut out, "user", service.user.as_deref(), NAME_MAX);
    put(&mut out, "uri", service.uri.as_deref(), TEXT_MAX);
    set(&mut out, "contexts", &service.contexts);
    put(&mut out, "label", service.label.as_deref(), ID_MAX);
    pref(&mut out, service.pref);
    Value::Object(out)
}

fn address(address: &PostalAddress) -> Value {
    let mut out = Args::new();
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
    json!({
        "kind": text(&anniversary.kind, KIND_MAX),
        "date": date(&anniversary.date),
    })
}

/// A date as ISO 8601 writes it: `1953-04-15`, `1953-04`, `1953`, a month
/// and day as `--04-15`; a timestamp as its UTC string.
fn date(date: &AnniversaryDate) -> Value {
    match date {
        AnniversaryDate::Utc(utc) => text(utc, TIME_MAX),
        AnniversaryDate::Partial { year, month, day } => json!(match (year, month, day) {
            (Some(year), None, None) => format!("{year:04}"),
            (Some(year), Some(month), None) => format!("{year:04}-{month:02}"),
            (Some(year), Some(month), Some(day)) => format!("{year:04}-{month:02}-{day:02}"),
            (None, Some(month), Some(day)) => format!("--{month:02}-{day:02}"),
            _ => unreachable!("the card reader admits only RFC 9553's four partial dates"),
        }),
    }
}

fn note(note: &Note) -> Value {
    let (cut, truncated) = cut_width(&note.note, NOTE_MAX);
    let mut out = Args::new();
    out.insert("note".into(), json!(cut));
    put(&mut out, "created", note.created.as_deref(), TIME_MAX);
    out.insert("truncated".into(), json!(truncated));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(text: &str) -> usize {
        serde_json::to_string(text).unwrap().len() - 2
    }

    #[test]
    fn cut_width_bounds_the_serialized_width_on_a_character_boundary() {
        assert_eq!(cut_width("hello", 5), ("hello".to_owned(), false));
        assert_eq!(cut_width("hello", 4), ("hell…".to_owned(), true));
        assert_eq!(cut_width("", 0), (String::new(), false));
        // As the kit's `cut` would cut text with nothing to escape.
        for (text, max) in [("héllo wörld", 3), ("😀😀", 5), ("abc", 2)] {
            assert_eq!(cut_width(text, max), brenn_provider_kit::cut(text, max));
        }
        assert_eq!(cut_width("a\"b", 2), ("a…".to_owned(), true));
        assert_eq!(cut_width("a\"b", 3), ("a\"…".to_owned(), true));
        assert_eq!(cut_width("\n\n\n", 5), ("\n\n…".to_owned(), true));
        assert_eq!(cut_width("\u{1}\u{1}", 11), ("\u{1}…".to_owned(), true));
        assert_eq!(cut_width("é", 1), ("…".to_owned(), true));
        for text in [
            "a\"b\\c\nd\te\r\u{8}\u{c}\u{1}\u{1f}\u{7f}é😀".repeat(40),
            "\u{1}".repeat(100),
            "€".repeat(100),
        ] {
            for max in [0, 1, 5, 6, 7, 64, 255, 400] {
                let (cut, was_cut) = cut_width(&text, max);
                let kept = cut.strip_suffix('…').unwrap_or(&cut);
                assert!(width(kept) <= max, "{max}: {kept:?}");
                assert_eq!(was_cut, kept.len() < text.len());
                if was_cut {
                    let next = text[kept.len()..].chars().next().unwrap();
                    assert!(width(kept) + width(&next.to_string()) > max);
                }
            }
        }
    }
}
