//! The contacts methods this crate builds and reads (RFC 9610):
//! `AddressBook/get`, `ContactCard/query`, `ContactCard/get` and
//! `ContactCard/set`. A card is JSContact (RFC 9553); the reader takes the
//! properties the tools show, each with the type RFC 9553 gives it, and
//! ignores every other property. A write builds a new card from bare values
//! and patches an existing one at the entry, so what it does not name stays
//! as the server holds it.
//!
//! JSContact's lists are `Id`-keyed maps with no order of their own. Each is
//! read into a `Vec` ordered by `pref` ascending where the type has one (RFC
//! 9553 section 1.5.3: lower is preferred), entries without `pref` after those
//! with, and ties and the types without `pref` by map key.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::method::{Fields, GetResult, Ids, Sort, read_get};
use crate::request::{CallId, Request};
use crate::set::{Patch, SetResult, set_args};

/// The properties `AddressBook/get` asks for; `AddressBook::list` reads these.
pub const ADDRESS_BOOK_PROPERTIES: &[&str] = &[
    "id",
    "name",
    "description",
    "sortOrder",
    "isDefault",
    "isSubscribed",
];

/// The properties of a card summary.
pub const CARD_SUMMARY_PROPERTIES: &[&str] = &[
    "id",
    "kind",
    "name",
    "emails",
    "phones",
    "organizations",
    "addressBookIds",
];

/// The properties of a full card read: the summary's and the rest the read
/// shows. `media` is not among them, so no photo is ever fetched.
pub const CARD_READ_PROPERTIES: &[&str] = &[
    "id",
    "kind",
    "name",
    "emails",
    "phones",
    "organizations",
    "addressBookIds",
    "uid",
    "nicknames",
    "titles",
    "onlineServices",
    "addresses",
    "anniversaries",
    "notes",
    "members",
    "created",
    "updated",
];

/// `AddressBook/get` over every address book in the account (`ids` null).
#[derive(Debug, Clone)]
pub struct AddressBookGet<'a> {
    pub account_id: &'a str,
    pub properties: &'a [&'a str],
}

impl AddressBookGet<'_> {
    pub const METHOD: &'static str = "AddressBook/get";

    pub fn args(&self) -> Value {
        json!({
            "accountId": self.account_id,
            "ids": null,
            "properties": self.properties,
        })
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// One address book (RFC 9610 section 2). An absent `sortOrder` reads as
/// zero, its RFC default; `isDefault` and `isSubscribed` have no default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressBook {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: u32,
    pub is_default: bool,
    pub is_subscribed: bool,
}

impl AddressBook {
    /// The address books in an `AddressBook/get` result, in the server's
    /// order.
    pub fn list(args: &Value) -> Result<Vec<AddressBook>, Error> {
        Ok(read_get(args, AddressBook::read)?.list)
    }

    fn read(value: &Value) -> Result<AddressBook, Error> {
        let f = Fields::of(value, "AddressBook")?;
        let sort_order = f.u64_or_zero("sortOrder")?;
        Ok(AddressBook {
            id: f.string("id")?,
            name: f.string("name")?,
            description: f.opt_string("description")?,
            sort_order: u32::try_from(sort_order)
                .map_err(|_| f.shape("sortOrder", "is over 2^32"))?,
            is_default: f.bool("isDefault")?,
            is_subscribed: f.bool("isSubscribed")?,
        })
    }
}

/// One `ContactCard/query` condition (RFC 9610 section 3.3.1); every property
/// given must match. A `None` field is left out; a condition with every field
/// `None` matches every card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardFilter {
    pub in_address_book: Option<String>,
    /// Matches the card whose `uid` is exactly this.
    pub uid: Option<String>,
    pub kind: Option<String>,
    pub text: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub organization: Option<String>,
}

impl CardFilter {
    pub fn to_value(&self) -> Value {
        let mut condition = Map::new();
        let strings = [
            ("inAddressBook", &self.in_address_book),
            ("uid", &self.uid),
            ("kind", &self.kind),
            ("text", &self.text),
            ("name", &self.name),
            ("email", &self.email),
            ("phone", &self.phone),
            ("organization", &self.organization),
        ];
        for (key, value) in strings {
            if let Some(value) = value {
                condition.insert(key.into(), Value::String(value.clone()));
            }
        }
        Value::Object(condition)
    }
}

/// `ContactCard/query` (RFC 9610 section 3.3).
#[derive(Debug, Clone)]
pub struct ContactCardQuery<'a> {
    pub account_id: &'a str,
    pub filter: CardFilter,
    pub sort: Vec<Sort>,
    pub position: u64,
    pub limit: u64,
    pub calculate_total: bool,
}

impl ContactCardQuery<'_> {
    pub const METHOD: &'static str = "ContactCard/query";

    pub fn args(&self) -> Value {
        json!({
            "accountId": self.account_id,
            "filter": self.filter.to_value(),
            "sort": Sort::values(&self.sort),
            "position": self.position,
            "limit": self.limit,
            "calculateTotal": self.calculate_total,
        })
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// `ContactCard/get` (RFC 9610 section 3.1).
#[derive(Debug, Clone)]
pub struct ContactCardGet<'a> {
    pub account_id: &'a str,
    pub ids: Ids,
    pub properties: &'a [&'a str],
}

impl ContactCardGet<'_> {
    pub const METHOD: &'static str = "ContactCard/get";

    pub fn args(&self) -> Value {
        let mut args = Map::new();
        args.insert("accountId".into(), json!(self.account_id));
        self.ids.insert_into(&mut args);
        args.insert("properties".into(), json!(self.properties));
        Value::Object(args)
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// A `NameComponent` or `AddressComponent` (RFC 9553 sections 2.2.1 and
/// 2.5.1). A `separator` component's value is inserted verbatim between its
/// neighbours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub kind: String,
    pub value: String,
}

impl Component {
    fn read(value: &Value, what: &'static str) -> Result<Component, Error> {
        let f = Fields::of(value, what)?;
        Ok(Component {
            kind: f.string("kind")?,
            value: f.string("value")?,
        })
    }

    fn read_name(value: &Value) -> Result<Component, Error> {
        Component::read(value, "ContactCard.name.components")
    }

    fn read_address(value: &Value) -> Result<Component, Error> {
        Component::read(value, "ContactCard.addresses.components")
    }
}

/// `full` when present and non-empty; otherwise the components joined, a
/// `separator` component's value inserted verbatim and `separator` (or one
/// space when there is none) between two adjacent other components; `None`
/// when there are no components.
fn compose(
    full: Option<&str>,
    components: &[Component],
    separator: Option<&str>,
) -> Option<String> {
    if let Some(full) = full
        && !full.is_empty()
    {
        return Some(full.to_owned());
    }
    if components.is_empty() {
        return None;
    }
    let separator = separator.unwrap_or(" ");
    let mut text = String::new();
    let mut after_value = false;
    for component in components {
        if component.kind == "separator" {
            text.push_str(&component.value);
            after_value = false;
        } else {
            if after_value {
                text.push_str(separator);
            }
            text.push_str(&component.value);
            after_value = true;
        }
    }
    Some(text)
}

/// A card's `name` (RFC 9553 section 2.2.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardName {
    pub full: Option<String>,
    pub components: Vec<Component>,
    pub is_ordered: bool,
    pub default_separator: Option<String>,
}

impl CardName {
    fn read(value: &Value) -> Result<CardName, Error> {
        let f = Fields::of(value, "ContactCard.name")?;
        Ok(CardName {
            full: f.opt_string("full")?,
            components: f.list("components", Component::read_name)?,
            is_ordered: f.opt_bool("isOrdered")?.unwrap_or(false),
            default_separator: f.opt_string("defaultSeparator")?,
        })
    }

    /// The name as one line (RFC 9553 section 2.2.1.1): `full`, or the
    /// components joined.
    pub fn display(&self) -> Option<String> {
        compose(
            self.full.as_deref(),
            &self.components,
            self.default_separator.as_deref(),
        )
    }
}

/// An `Organization` (RFC 9553 section 2.2.3); a title names it by `key`.
/// `units` are the `OrgUnit` names, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub key: String,
    pub name: Option<String>,
    pub units: Vec<String>,
}

impl Organization {
    fn read(key: String, value: &Value) -> Result<Organization, Error> {
        let f = Fields::of(value, "ContactCard.organizations")?;
        Ok(Organization {
            key,
            name: f.opt_string("name")?,
            units: f.list("units", |unit| {
                Fields::of(unit, "ContactCard.organizations.units")?.string("name")
            })?,
        })
    }
}

/// A `Title` (RFC 9553 section 2.2.5); `kind` defaults to `title`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Title {
    /// The entry's key in the card's map.
    pub key: String,
    pub name: String,
    pub kind: String,
    pub organization_id: Option<String>,
}

impl Title {
    fn read(key: String, value: &Value) -> Result<Title, Error> {
        let f = Fields::of(value, "ContactCard.titles")?;
        Ok(Title {
            key,
            name: f.string("name")?,
            kind: f.opt_string("kind")?.unwrap_or_else(|| "title".into()),
            organization_id: f.opt_string("organizationId")?,
        })
    }
}

/// An `EmailAddress` (RFC 9553 section 2.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardEmail {
    /// The entry's key in the card's map.
    pub key: String,
    pub address: String,
    pub contexts: Vec<String>,
    pub label: Option<String>,
    pub pref: Option<u64>,
}

impl CardEmail {
    fn read(key: String, value: &Value) -> Result<CardEmail, Error> {
        let f = Fields::of(value, "ContactCard.emails")?;
        Ok(CardEmail {
            key,
            address: f.string("address")?,
            contexts: held(&f, "contexts")?,
            label: f.opt_string("label")?,
            pref: f.opt_u64("pref")?,
        })
    }
}

/// A `Phone` (RFC 9553 section 2.3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phone {
    /// The entry's key in the card's map.
    pub key: String,
    pub number: String,
    pub features: Vec<String>,
    pub contexts: Vec<String>,
    pub label: Option<String>,
    pub pref: Option<u64>,
}

impl Phone {
    fn read(key: String, value: &Value) -> Result<Phone, Error> {
        let f = Fields::of(value, "ContactCard.phones")?;
        Ok(Phone {
            key,
            number: f.string("number")?,
            features: held(&f, "features")?,
            contexts: held(&f, "contexts")?,
            label: f.opt_string("label")?,
            pref: f.opt_u64("pref")?,
        })
    }
}

/// An `OnlineService` (RFC 9553 section 2.3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineService {
    /// The entry's key in the card's map.
    pub key: String,
    pub service: Option<String>,
    pub uri: Option<String>,
    pub user: Option<String>,
    pub contexts: Vec<String>,
    pub label: Option<String>,
    pub pref: Option<u64>,
}

impl OnlineService {
    fn read(key: String, value: &Value) -> Result<OnlineService, Error> {
        let f = Fields::of(value, "ContactCard.onlineServices")?;
        Ok(OnlineService {
            key,
            service: f.opt_string("service")?,
            uri: f.opt_string("uri")?,
            user: f.opt_string("user")?,
            contexts: held(&f, "contexts")?,
            label: f.opt_string("label")?,
            pref: f.opt_u64("pref")?,
        })
    }
}

/// An `Address` (RFC 9553 section 2.5.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostalAddress {
    /// The entry's key in the card's map.
    pub key: String,
    pub full: Option<String>,
    pub components: Vec<Component>,
    pub is_ordered: bool,
    pub default_separator: Option<String>,
    pub country_code: Option<String>,
    pub contexts: Vec<String>,
    pub pref: Option<u64>,
}

impl PostalAddress {
    fn read(key: String, value: &Value) -> Result<PostalAddress, Error> {
        let f = Fields::of(value, "ContactCard.addresses")?;
        Ok(PostalAddress {
            key,
            full: f.opt_string("full")?,
            components: f.list("components", Component::read_address)?,
            is_ordered: f.opt_bool("isOrdered")?.unwrap_or(false),
            default_separator: f.opt_string("defaultSeparator")?,
            country_code: f.opt_string("countryCode")?,
            contexts: held(&f, "contexts")?,
            pref: f.opt_u64("pref")?,
        })
    }

    /// The address as text (RFC 9553 section 2.5.1.1): `full`, or the
    /// components joined.
    pub fn display(&self) -> Option<String> {
        compose(
            self.full.as_deref(),
            &self.components,
            self.default_separator.as_deref(),
        )
    }
}

/// An anniversary's date (RFC 9553 section 2.8.1): a `PartialDate` holding a
/// year; a year and month; a year, month and day; or a month and day. Or a
/// `Timestamp`'s UTC string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnniversaryDate {
    Partial {
        year: Option<u32>,
        month: Option<u32>,
        day: Option<u32>,
    },
    Utc(String),
}

impl AnniversaryDate {
    fn read(value: &Value) -> Result<AnniversaryDate, Error> {
        let f = Fields::of(value, "ContactCard.anniversaries.date")?;
        match f.opt_string("@type")?.as_deref() {
            None | Some("PartialDate") => {}
            Some("Timestamp") => return Ok(AnniversaryDate::Utc(f.string("utc")?)),
            Some(_) => return Err(f.shape("@type", "is neither PartialDate nor Timestamp")),
        }
        let part = |key: &str, min: u32, max: u32| -> Result<Option<u32>, Error> {
            f.opt_u64(key)?
                .map(|n| {
                    u32::try_from(n)
                        .ok()
                        .filter(|n| (min..=max).contains(n))
                        .ok_or_else(|| f.shape(key, &format!("is outside {min} to {max}")))
                })
                .transpose()
        };
        let year = part("year", 0, u32::MAX)?;
        let month = part("month", 1, 12)?;
        let day = part("day", 1, 31)?;
        match (year, month, day) {
            (Some(_), None, None)
            | (Some(_), Some(_), None)
            | (Some(_), Some(_), Some(_))
            | (None, Some(_), Some(_)) => Ok(AnniversaryDate::Partial { year, month, day }),
            _ => Err(Error::Shape(
                "ContactCard.anniversaries.date is not a year, a year and month, \
                 a year, month and day, or a month and day"
                    .into(),
            )),
        }
    }
}

/// An `Anniversary` (RFC 9553 section 2.8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anniversary {
    /// The entry's key in the card's map.
    pub key: String,
    pub kind: String,
    pub date: AnniversaryDate,
}

impl Anniversary {
    fn read(key: String, value: &Value) -> Result<Anniversary, Error> {
        let f = Fields::of(value, "ContactCard.anniversaries")?;
        let date = f.get("date").ok_or_else(|| f.shape("date", "is missing"))?;
        Ok(Anniversary {
            key,
            kind: f.string("kind")?,
            date: AnniversaryDate::read(date)?,
        })
    }
}

/// A `Note` (RFC 9553 section 2.8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The entry's key in the card's map.
    pub key: String,
    pub note: String,
    pub created: Option<String>,
}

impl Note {
    fn read(key: String, value: &Value) -> Result<Note, Error> {
        let f = Fields::of(value, "ContactCard.notes")?;
        Ok(Note {
            key,
            note: f.string("note")?,
            created: f.opt_string("created")?,
        })
    }
}

/// A `Nickname` (RFC 9553 section 2.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nickname {
    /// The entry's key in the card's map.
    pub key: String,
    pub name: String,
    pub pref: Option<u64>,
}

impl Nickname {
    fn read(key: String, value: &Value) -> Result<Nickname, Error> {
        let f = Fields::of(value, "ContactCard.nicknames")?;
        Ok(Nickname {
            key,
            name: f.string("name")?,
            pref: f.opt_u64("pref")?,
        })
    }
}

/// A card with the properties `CARD_SUMMARY_PROPERTIES` and
/// `CARD_READ_PROPERTIES` name (RFC 9610 section 3, RFC 9553). Only `id` is
/// required: a property not asked for, or null, reads as `None`, empty or its
/// RFC default. `uid` is optional because the summary does not ask for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactCard {
    pub id: String,
    pub uid: Option<String>,
    /// `individual` when absent.
    pub kind: String,
    pub name: Option<CardName>,
    pub nicknames: Vec<Nickname>,
    pub organizations: Vec<Organization>,
    pub titles: Vec<Title>,
    pub emails: Vec<CardEmail>,
    pub phones: Vec<Phone>,
    pub online_services: Vec<OnlineService>,
    pub addresses: Vec<PostalAddress>,
    pub anniversaries: Vec<Anniversary>,
    pub notes: Vec<Note>,
    /// The member uids held `true`, sorted.
    pub members: Vec<String>,
    /// The address book ids held `true`, sorted.
    pub address_book_ids: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
}

impl ContactCard {
    /// The cards in a `ContactCard/get` result.
    pub fn get(args: &Value) -> Result<GetResult<ContactCard>, Error> {
        read_get(args, ContactCard::read)
    }

    /// A card; a property of the wrong shape is refused naming the card's id.
    fn read(value: &Value) -> Result<ContactCard, Error> {
        let f = Fields::of(value, "ContactCard")?;
        let id = f.string("id")?;
        ContactCard::properties(&f, &id).map_err(|error| match error {
            Error::Shape(problem) => Error::Shape(format!("{problem} (card `{id}`)")),
            other => other,
        })
    }

    fn properties(f: &Fields<'_>, id: &str) -> Result<ContactCard, Error> {
        Ok(ContactCard {
            id: id.to_owned(),
            uid: f.opt_string("uid")?,
            kind: f.opt_string("kind")?.unwrap_or_else(|| "individual".into()),
            name: f.get("name").map(CardName::read).transpose()?,
            nicknames: by_pref(f, EntryList::Nicknames, Nickname::read, |n| n.pref)?,
            organizations: in_key_order(f, EntryList::Organizations, Organization::read)?,
            titles: in_key_order(f, EntryList::Titles, Title::read)?,
            emails: by_pref(f, EntryList::Emails, CardEmail::read, |e| e.pref)?,
            phones: by_pref(f, EntryList::Phones, Phone::read, |p| p.pref)?,
            online_services: by_pref(f, EntryList::OnlineServices, OnlineService::read, |s| {
                s.pref
            })?,
            addresses: by_pref(f, EntryList::Addresses, PostalAddress::read, |a| a.pref)?,
            anniversaries: in_key_order(f, EntryList::Anniversaries, Anniversary::read)?,
            notes: in_key_order(f, EntryList::Notes, Note::read)?,
            members: held(f, "members")?,
            address_book_ids: held(f, "addressBookIds")?,
            created: f.opt_string("created")?,
            updated: f.opt_string("updated")?,
        })
    }
}

impl AnniversaryDate {
    /// A partial date as ISO 8601 writes it: `1953`, `1953-04`, `1953-04-15`,
    /// or `--04-15` for a month and day; month 1 to 12, day 1 to 31, as the
    /// reader holds them. `None` for any other text.
    pub fn parse_partial(text: &str) -> Option<AnniversaryDate> {
        let number = |part: &str, digits: usize| -> Option<u32> {
            (part.len() == digits && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        };
        let (year, rest) = match text.strip_prefix("--") {
            Some(rest) => (None, rest.split('-').map(str::to_owned).collect::<Vec<_>>()),
            None => {
                let mut parts = text.split('-');
                let year = number(parts.next()?, 4)?;
                (Some(year), parts.map(str::to_owned).collect())
            }
        };
        let (month, day) = match (year, rest.as_slice()) {
            (Some(_), []) => (None, None),
            (Some(_), [month]) => (Some(number(month, 2)?), None),
            (_, [month, day]) => (Some(number(month, 2)?), Some(number(day, 2)?)),
            _ => return None,
        };
        if month.is_some_and(|m| !(1..=12).contains(&m))
            || day.is_some_and(|d| !(1..=31).contains(&d))
        {
            return None;
        }
        Some(AnniversaryDate::Partial { year, month, day })
    }

    /// The date as text: a partial date in the form [`parse_partial`] reads
    /// (`1953`, `1953-04`, `1953-04-15`, `--04-15`), a timestamp as its UTC
    /// string.
    ///
    /// [`parse_partial`]: AnniversaryDate::parse_partial
    pub fn to_text(&self) -> String {
        match self {
            AnniversaryDate::Utc(utc) => utc.clone(),
            AnniversaryDate::Partial { year, month, day } => match (year, month, day) {
                (Some(year), None, None) => format!("{year:04}"),
                (Some(year), Some(month), None) => format!("{year:04}-{month:02}"),
                (Some(year), Some(month), Some(day)) => format!("{year:04}-{month:02}-{day:02}"),
                (None, Some(month), Some(day)) => format!("--{month:02}-{day:02}"),
                _ => unreachable!("the card reader admits only RFC 9553's four partial dates"),
            },
        }
    }

    /// The date as JSContact writes it: a `PartialDate` or a `Timestamp`.
    pub fn to_value(&self) -> Value {
        match self {
            AnniversaryDate::Partial { year, month, day } => {
                let mut date = Map::new();
                date.insert("@type".into(), json!("PartialDate"));
                for (key, part) in [("year", year), ("month", month), ("day", day)] {
                    if let Some(part) = part {
                        date.insert(key.into(), json!(part));
                    }
                }
                Value::Object(date)
            }
            AnniversaryDate::Utc(utc) => json!({"@type": "Timestamp", "utc": utc}),
        }
    }
}

/// One of a card's `Id`-keyed lists an update can change an entry of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryList {
    Emails,
    Phones,
    OnlineServices,
    Addresses,
    Organizations,
    Titles,
    Nicknames,
    Anniversaries,
    Notes,
}

impl EntryList {
    pub const ALL: [EntryList; 9] = [
        EntryList::Emails,
        EntryList::Phones,
        EntryList::OnlineServices,
        EntryList::Addresses,
        EntryList::Organizations,
        EntryList::Titles,
        EntryList::Nicknames,
        EntryList::Anniversaries,
        EntryList::Notes,
    ];

    /// The card's property (RFC 9553).
    pub fn property(self) -> &'static str {
        match self {
            EntryList::Emails => "emails",
            EntryList::Phones => "phones",
            EntryList::OnlineServices => "onlineServices",
            EntryList::Addresses => "addresses",
            EntryList::Organizations => "organizations",
            EntryList::Titles => "titles",
            EntryList::Nicknames => "nicknames",
            EntryList::Anniversaries => "anniversaries",
            EntryList::Notes => "notes",
        }
    }

    /// The entry's sub-property holding its main value.
    fn value_property(self) -> &'static str {
        match self {
            EntryList::Emails => "address",
            EntryList::Phones => "number",
            EntryList::OnlineServices => "uri",
            EntryList::Addresses => "full",
            EntryList::Organizations | EntryList::Titles | EntryList::Nicknames => "name",
            EntryList::Anniversaries => "date",
            EntryList::Notes => "note",
        }
    }
}

/// An entry's main value: text, or an anniversary's date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryValue {
    Text(String),
    Date(AnniversaryDate),
}

/// The sub-properties one entry change names; each `None` is left alone.
/// Which a list takes is the caller's to hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryFields {
    pub value: Option<EntryValue>,
    pub label: Option<String>,
    /// The whole context set.
    pub contexts: Option<Vec<String>>,
    /// The whole feature set.
    pub features: Option<Vec<String>>,
    pub pref: Option<u64>,
    pub kind: Option<String>,
    pub service: Option<String>,
    pub user: Option<String>,
    pub country_code: Option<String>,
}

impl EntryFields {
    /// An entry holding only `value`.
    pub fn of_value(value: EntryValue) -> EntryFields {
        EntryFields {
            value: Some(value),
            ..EntryFields::default()
        }
    }

    /// `(sub-property, value)` for each field given, in a fixed order; a set
    /// is written `{"<member>": true, ...}`.
    pub fn pairs(&self, list: EntryList) -> Vec<(&'static str, Value)> {
        let set = |members: &[String]| -> Value {
            Value::Object(
                members
                    .iter()
                    .map(|member| (member.clone(), Value::Bool(true)))
                    .collect(),
            )
        };
        let mut pairs = Vec::new();
        if let Some(value) = &self.value {
            let value = match value {
                EntryValue::Text(text) => json!(text),
                EntryValue::Date(date) => date.to_value(),
            };
            pairs.push((list.value_property(), value));
        }
        let texts = [
            ("label", &self.label),
            ("kind", &self.kind),
            ("service", &self.service),
            ("user", &self.user),
            ("countryCode", &self.country_code),
        ];
        for (key, text) in texts {
            if let Some(text) = text {
                pairs.push((key, json!(text)));
            }
        }
        if let Some(contexts) = &self.contexts {
            pairs.push(("contexts", set(contexts)));
        }
        if let Some(features) = &self.features {
            pairs.push(("features", set(features)));
        }
        if let Some(pref) = self.pref {
            pairs.push(("pref", json!(pref)));
        }
        pairs
    }

    /// The entry object an add writes: exactly the fields given.
    pub fn entry(&self, list: EntryList) -> Value {
        Value::Object(
            self.pairs(list)
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }
}

/// One change to one entry of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryChange {
    Add(EntryFields),
    Edit { key: String, fields: EntryFields },
    Remove { key: String },
}

impl EntryChange {
    /// `patch` with this change to `list` added, given the keys the card
    /// holds there (`None` when the card has no such property), and the new
    /// entry's key on an add. An add to an absent property writes the whole
    /// property, since a patch path's parent must exist (RFC 8620 section
    /// 5.3); the new key is `k<n>` for the smallest `n` not held. An edit
    /// writes each sub-property given at its own path, and an edit of an
    /// address's text clears the parts that would contradict it. `None` when
    /// an edit or remove names a key the card does not hold.
    pub fn patch(
        &self,
        list: EntryList,
        held: Option<&BTreeSet<String>>,
        patch: Patch,
    ) -> Option<(Patch, Option<String>)> {
        let property = list.property();
        match self {
            EntryChange::Add(fields) => {
                let key = (1..)
                    .map(|n| format!("k{n}"))
                    .find(|key| held.is_none_or(|held| !held.contains(key)))
                    .expect("an unbounded range holds a free key");
                let patch = match held {
                    Some(_) => patch.at(&[property, &key], fields.entry(list)),
                    None => patch.property(property, json!({ key.as_str(): fields.entry(list) })),
                };
                Some((patch, Some(key)))
            }
            EntryChange::Edit { key, fields } => {
                held?.get(key)?;
                let mut patch = patch;
                for (sub, value) in fields.pairs(list) {
                    patch = patch.at(&[property, key, sub], value);
                }
                if list == EntryList::Addresses && fields.value.is_some() {
                    for sub in ["components", "isOrdered", "defaultSeparator"] {
                        patch = patch.at(&[property, key, sub], Value::Null);
                    }
                }
                Some((patch, None))
            }
            EntryChange::Remove { key } => {
                held?.get(key)?;
                Some((patch.at(&[property, key], Value::Null), None))
            }
        }
    }
}

/// A card's `name` change: `full` and the `given` and `surname` parts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameChange {
    pub full: Option<String>,
    pub given: Option<String>,
    pub surname: Option<String>,
}

/// Why a name change cannot be written to a card's `name` without guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRefusal {
    /// A part change without `full`, on a name whose non-empty `full` is
    /// what a reader shows instead of the parts.
    FullHidesParts,
    /// The name holds more than one component of this kind.
    Ambiguous(&'static str),
    /// The name lacks this kind and is ordered, so the part has no place.
    Unplaceable(&'static str),
}

impl NameChange {
    fn parts(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [("given", &self.given), ("surname", &self.surname)]
            .into_iter()
            .filter_map(|(kind, value)| value.as_deref().map(|value| (kind, value)))
    }

    /// The whole `name` property: `full` and the components given, `given`
    /// before `surname`.
    pub fn to_value(&self) -> Value {
        let mut name = Map::new();
        if let Some(full) = &self.full {
            name.insert("full".into(), json!(full));
        }
        let components: Vec<Value> = self
            .parts()
            .map(|(kind, value)| json!({"kind": kind, "value": value}))
            .collect();
        if !components.is_empty() {
            name.insert("components".into(), Value::Array(components));
        }
        Value::Object(name)
    }

    /// `patch` with this change added. With no `name` on the card, the whole
    /// property. Otherwise `name/full` for `full`, and for a part
    /// `name/components` rewritten whole, since a patch path cannot point
    /// into an array (RFC 8620 section 5.3): every component carried through
    /// as the server sent it, the one of the part's kind with its `value`
    /// replaced, and a kind the name lacks appended. Nothing else of the name
    /// is written.
    pub fn patch(&self, state: Option<&NameState>, patch: Patch) -> Result<Patch, NameRefusal> {
        let Some(state) = state else {
            return Ok(patch.property("name", self.to_value()));
        };
        let mut patch = patch;
        if let Some(full) = &self.full {
            patch = patch.at(&["name", "full"], json!(full));
        }
        if self.parts().next().is_none() {
            return Ok(patch);
        }
        if self.full.is_none() && state.has_full {
            return Err(NameRefusal::FullHidesParts);
        }
        let mut components = state.components.clone();
        for (kind, value) in self.parts() {
            let mut of_kind = components
                .iter_mut()
                .filter(|component| component["kind"] == kind);
            match (of_kind.next(), of_kind.next()) {
                (Some(_), Some(_)) => return Err(NameRefusal::Ambiguous(kind)),
                (Some(component), None) => component["value"] = json!(value),
                (None, _) if state.is_ordered => return Err(NameRefusal::Unplaceable(kind)),
                (None, _) => components.push(json!({"kind": kind, "value": value})),
            }
        }
        Ok(patch.at(&["name", "components"], Value::Array(components)))
    }
}

/// A card's `name` as an update reads it to patch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameState {
    /// Whether `full` is a non-empty string.
    pub has_full: bool,
    /// The components as the server sent them, each an object with a string
    /// `kind`.
    pub components: Vec<Value>,
    pub is_ordered: bool,
}

impl NameState {
    fn read(value: &Value) -> Result<NameState, Error> {
        let f = Fields::of(value, "ContactCard.name")?;
        let components = f.list("components", |component| {
            Fields::of(component, "ContactCard.name.components")?.string("kind")?;
            Ok(component.clone())
        })?;
        Ok(NameState {
            has_full: f.opt_string("full")?.is_some_and(|full| !full.is_empty()),
            components,
            is_ordered: f.opt_bool("isOrdered")?.unwrap_or(false),
        })
    }
}

/// What an update reads of a card to patch it: per list present, its keys,
/// and its `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardKeys {
    pub id: String,
    lists: BTreeMap<EntryList, BTreeSet<String>>,
    pub name: Option<NameState>,
}

impl CardKeys {
    /// A `ContactCard/get` result's `state` and its cards. A property absent
    /// or null reads as not present.
    pub fn get(args: &Value) -> Result<(String, GetResult<CardKeys>), Error> {
        let state = Fields::of(args, "get result")?.string("state")?;
        Ok((state, read_get(args, CardKeys::read)?))
    }

    /// The keys the card holds in `list`; `None` when the property is not
    /// present.
    pub fn keys(&self, list: EntryList) -> Option<&BTreeSet<String>> {
        self.lists.get(&list)
    }

    fn read(value: &Value) -> Result<CardKeys, Error> {
        let f = Fields::of(value, "ContactCard")?;
        let mut lists = BTreeMap::new();
        for list in EntryList::ALL {
            if let Some(map) = f.get(list.property()) {
                let keys = map
                    .as_object()
                    .ok_or_else(|| f.shape(list.property(), "is not an object"))?
                    .keys()
                    .cloned()
                    .collect();
                lists.insert(list, keys);
            }
        }
        Ok(CardKeys {
            id: f.string("id")?,
            lists,
            name: f.get("name").map(NameState::read).transpose()?,
        })
    }
}

/// A new individual card (RFC 9553 section 2): its uid, its one address
/// book, its name, and bare values for lists, each entry holding its main
/// value only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCard {
    pub uid: String,
    pub address_book: String,
    pub name: Option<NameChange>,
    /// Each list's values, entry keys `k1`, `k2`, … in order.
    pub lists: Vec<(EntryList, Vec<String>)>,
    pub birthday: Option<AnniversaryDate>,
}

impl NewCard {
    pub fn to_value(&self) -> Value {
        let mut card = Map::new();
        card.insert("@type".into(), json!("Card"));
        card.insert("version".into(), json!("1.0"));
        card.insert("uid".into(), json!(self.uid));
        card.insert(
            "addressBookIds".into(),
            json!({ self.address_book.as_str(): true }),
        );
        if let Some(name) = &self.name {
            card.insert("name".into(), name.to_value());
        }
        for (list, values) in &self.lists {
            let entries: Map<String, Value> = values
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    let fields = EntryFields::of_value(EntryValue::Text(value.clone()));
                    (format!("k{}", i + 1), fields.entry(*list))
                })
                .collect();
            card.insert(list.property().into(), Value::Object(entries));
        }
        if let Some(birthday) = &self.birthday {
            let fields = EntryFields {
                kind: Some("birth".into()),
                ..EntryFields::of_value(EntryValue::Date(birthday.clone()))
            };
            card.insert(
                EntryList::Anniversaries.property().into(),
                json!({"k1": fields.entry(EntryList::Anniversaries)}),
            );
        }
        Value::Object(card)
    }
}

/// `ContactCard/set` (RFC 9610 section 3.4): creates and updates. Nothing
/// here destroys.
#[derive(Debug, Clone)]
pub struct ContactCardSet<'a> {
    pub account_id: &'a str,
    /// The state the update was computed against; the server refuses the
    /// whole method with `stateMismatch` when the account has moved on.
    pub if_in_state: Option<&'a str>,
    /// Creation id → the new card.
    pub create: Vec<(String, NewCard)>,
    /// Card id → its patch.
    pub update: Vec<(String, Patch)>,
}

impl ContactCardSet<'_> {
    pub const METHOD: &'static str = "ContactCard/set";

    pub fn args(&self) -> Value {
        set_args(
            self.account_id,
            self.create
                .iter()
                .map(|(cid, card)| (cid.clone(), card.to_value())),
            &self.update,
            self.if_in_state,
        )
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }

    /// Reads the result of a `ContactCard/set`.
    pub fn result(args: &Value) -> Result<SetResult<CardCreated>, Error> {
        SetResult::read(args, CardCreated::read)
    }
}

/// What the server set on a created card that this crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardCreated {
    pub id: String,
}

impl CardCreated {
    fn read(value: &Value) -> Result<CardCreated, Error> {
        Ok(CardCreated {
            id: Fields::of(value, "created ContactCard")?.string("id")?,
        })
    }
}

/// The keys of a `String[Boolean]` or `Id[Boolean]` set held `true`, sorted.
fn held(f: &Fields<'_>, key: &str) -> Result<Vec<String>, Error> {
    Ok(f.flags(key)?
        .into_iter()
        .filter_map(|(name, flag)| flag.then_some(name))
        .collect())
}

/// `list`'s entries through `read`, each handed its key, by key.
fn in_key_order<T>(
    f: &Fields<'_>,
    list: EntryList,
    read: fn(String, &Value) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let key = list.property();
    let Some(map) = f.get(key) else {
        return Ok(Vec::new());
    };
    let map: BTreeMap<&String, &Value> = map
        .as_object()
        .ok_or_else(|| f.shape(key, "is not an object"))?
        .iter()
        .collect();
    map.into_iter()
        .map(|(entry_key, value)| read(entry_key.clone(), value))
        .collect()
}

/// `list`'s entries through `read`, by `pref` ascending, entries without
/// `pref` last, ties by key.
fn by_pref<T>(
    f: &Fields<'_>,
    list: EntryList,
    read: fn(String, &Value) -> Result<T, Error>,
    pref: fn(&T) -> Option<u64>,
) -> Result<Vec<T>, Error> {
    let mut entries = in_key_order(f, list, read)?;
    entries.sort_by_key(|entry| (pref(entry).is_none(), pref(entry)));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::QueryResult;
    use crate::request::result_ref;

    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn body(request: &Request) -> Value {
        serde_json::from_str(&request.to_body()).unwrap()
    }

    fn cards() -> GetResult<ContactCard> {
        ContactCard::get(&fixture(include_str!("../fixtures/contact_card_get.json"))).unwrap()
    }

    /// One card read from `card`, with `id` added.
    fn card(mut card: Value) -> Result<ContactCard, Error> {
        card["id"] = json!("c1");
        let mut result = ContactCard::get(&json!({"list": [card], "notFound": []}))?;
        Ok(result.list.remove(0))
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|&item| item.to_owned()).collect()
    }

    fn component(kind: &str, value: &str) -> Component {
        Component {
            kind: kind.into(),
            value: value.into(),
        }
    }

    fn partial(year: Option<u32>, month: Option<u32>, day: Option<u32>) -> AnniversaryDate {
        AnniversaryDate::Partial { year, month, day }
    }

    /// The problem in a shape error refusing the card `card` builds, without
    /// the card's id the message ends with.
    fn shape_message<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
        match result {
            Err(Error::Shape(message)) => match message.strip_suffix(" (card `c1`)") {
                Some(problem) => problem.to_owned(),
                None => panic!("{message:?} does not name card `c1`"),
            },
            other => panic!("expected a shape error, got {other:?}"),
        }
    }

    #[test]
    fn address_book_get_asks_for_every_book() {
        let mut request = Request::contacts();
        let call = AddressBookGet {
            account_id: "a1",
            properties: ADDRESS_BOOK_PROPERTIES,
        }
        .push(&mut request);
        assert_eq!(call.name(), "AddressBook/get");
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["AddressBook/get", {
                "accountId": "a1",
                "ids": null,
                "properties": ["id", "name", "description", "sortOrder", "isDefault",
                    "isSubscribed"]
            }, "c0"]])
        );
    }

    #[test]
    fn address_books_are_read_with_an_absent_sort_order_as_zero() {
        let books =
            AddressBook::list(&fixture(include_str!("../fixtures/address_book_get.json"))).unwrap();
        assert_eq!(
            books,
            vec![
                AddressBook {
                    id: "b1".into(),
                    name: "Personal".into(),
                    description: Some("Friends and family".into()),
                    sort_order: 2,
                    is_default: true,
                    is_subscribed: true,
                },
                AddressBook {
                    id: "b2".into(),
                    name: "Shared".into(),
                    description: None,
                    sort_order: 0,
                    is_default: false,
                    is_subscribed: false,
                },
            ]
        );
    }

    #[test]
    fn an_address_book_of_the_wrong_shape_is_malformed() {
        let good = json!({"id": "b1", "name": "Personal", "isDefault": true, "isSubscribed": true});
        let mut cases = Vec::new();
        for key in ["id", "name", "isDefault", "isSubscribed"] {
            let mut book = good.clone();
            book.as_object_mut().unwrap().remove(key);
            cases.push(book);
        }
        for (key, value) in [
            ("sortOrder", json!("1")),
            ("sortOrder", json!(4_294_967_296_u64)),
            ("isDefault", json!("true")),
            ("description", json!(1)),
        ] {
            let mut book = good.clone();
            book[key] = value;
            cases.push(book);
        }
        for book in cases {
            let args = json!({"list": [book], "notFound": []});
            assert!(
                matches!(AddressBook::list(&args), Err(Error::Shape(_))),
                "{book}"
            );
        }
    }

    #[test]
    fn a_card_query_sends_every_condition_under_its_rfc_name_and_the_sort() {
        let query = ContactCardQuery {
            account_id: "a1",
            filter: CardFilter {
                in_address_book: Some("b1".into()),
                uid: Some("urn:uuid:1".into()),
                kind: Some("group".into()),
                text: Some("alice".into()),
                name: Some("Alice".into()),
                email: Some("alice@example.com".into()),
                phone: Some("555".into()),
                organization: Some("Example".into()),
            },
            sort: vec![
                Sort {
                    property: "name/surname",
                    is_ascending: true,
                },
                Sort {
                    property: "name/given",
                    is_ascending: true,
                },
            ],
            position: 20,
            limit: 10,
            calculate_total: true,
        };
        let mut request = Request::contacts();
        let call = query.push(&mut request);
        assert_eq!(call.name(), "ContactCard/query");
        assert_eq!(
            body(&request)["methodCalls"][0][1],
            json!({
                "accountId": "a1",
                "filter": {
                    "inAddressBook": "b1", "uid": "urn:uuid:1", "kind": "group",
                    "text": "alice", "name": "Alice", "email": "alice@example.com",
                    "phone": "555", "organization": "Example"
                },
                "sort": [
                    {"property": "name/surname", "isAscending": true},
                    {"property": "name/given", "isAscending": true}
                ],
                "position": 20,
                "limit": 10,
                "calculateTotal": true
            })
        );
        assert_eq!(CardFilter::default().to_value(), json!({}));
    }

    #[test]
    fn a_card_query_result_is_read() {
        assert_eq!(
            QueryResult::read(&fixture(include_str!(
                "../fixtures/contact_card_query.json"
            )))
            .unwrap(),
            QueryResult {
                ids: strings(&["c1", "c2", "c3"]),
                position: 0,
                total: Some(3),
            }
        );
    }

    #[test]
    fn a_card_get_names_its_ids_listed_or_by_reference() {
        let mut request = Request::contacts();
        let query = ContactCardQuery {
            account_id: "a1",
            filter: CardFilter::default(),
            sort: vec![],
            position: 0,
            limit: 10,
            calculate_total: false,
        }
        .push(&mut request);
        ContactCardGet {
            account_id: "a1",
            ids: Ids::Ref(result_ref(&query, "/ids")),
            properties: CARD_SUMMARY_PROPERTIES,
        }
        .push(&mut request);
        ContactCardGet {
            account_id: "a1",
            ids: Ids::Listed(vec!["c1".into()]),
            properties: CARD_READ_PROPERTIES,
        }
        .push(&mut request);
        let calls = body(&request)["methodCalls"].clone();
        assert_eq!(
            calls[1],
            json!(["ContactCard/get", {
                "accountId": "a1",
                "#ids": {"resultOf": "c0", "name": "ContactCard/query", "path": "/ids"},
                "properties": ["id", "kind", "name", "emails", "phones", "organizations",
                    "addressBookIds"]
            }, "c1"])
        );
        assert_eq!(calls[2][0], json!("ContactCard/get"));
        assert_eq!(calls[2][1]["ids"], json!(["c1"]));
        assert_eq!(calls[2][1]["properties"], json!(CARD_READ_PROPERTIES));
        assert!(calls[2][1].get("#ids").is_none());
    }

    #[test]
    fn the_full_card_reads_every_property_it_shows() {
        let result = cards();
        assert_eq!(result.not_found, strings(&["c9"]));
        let full = &result.list[0];
        let expected = ContactCard {
            id: "c1".into(),
            uid: Some("urn:uuid:03a0e51f-d1aa-4385-8a53-e29025acd8af".into()),
            kind: "individual".into(),
            name: Some(CardName {
                full: None,
                components: vec![
                    component("title", "Mr."),
                    component("given", "John"),
                    component("given2", "Quinlan"),
                    component("surname", "Public"),
                    component("separator", ", "),
                    component("generation", "Esq."),
                ],
                is_ordered: true,
                default_separator: None,
            }),
            nicknames: vec![
                Nickname {
                    key: "k392".into(),
                    name: "JQ".into(),
                    pref: Some(1),
                },
                Nickname {
                    key: "k391".into(),
                    name: "Johnny".into(),
                    pref: None,
                },
            ],
            organizations: vec![
                Organization {
                    key: "o1".into(),
                    name: None,
                    units: strings(&["Volunteers"]),
                },
                Organization {
                    key: "o2".into(),
                    name: Some("ABC, Inc.".into()),
                    units: strings(&["North American Division", "Marketing"]),
                },
            ],
            titles: vec![
                Title {
                    key: "k2".into(),
                    name: "Treasurer".into(),
                    kind: "title".into(),
                    organization_id: None,
                },
                Title {
                    key: "le9".into(),
                    name: "Project Leader".into(),
                    kind: "title".into(),
                    organization_id: Some("o2".into()),
                },
            ],
            emails: vec![
                CardEmail {
                    key: "e3".into(),
                    address: "alice@example.com".into(),
                    contexts: strings(&["private"]),
                    label: Some("home".into()),
                    pref: Some(1),
                },
                CardEmail {
                    key: "e2".into(),
                    address: "jane_doe@example.com".into(),
                    contexts: vec![],
                    label: None,
                    pref: Some(2),
                },
                CardEmail {
                    key: "e1".into(),
                    address: "jqpublic@xyz.example.com".into(),
                    contexts: strings(&["work"]),
                    label: None,
                    pref: None,
                },
            ],
            phones: vec![
                Phone {
                    key: "tel0".into(),
                    number: "tel:+1-555-555-5555;ext=5555".into(),
                    features: strings(&["voice"]),
                    contexts: strings(&["private"]),
                    label: None,
                    pref: Some(1),
                },
                Phone {
                    key: "tel3".into(),
                    number: "tel:+1-201-555-0123".into(),
                    features: strings(&["text", "voice"]),
                    contexts: strings(&["work"]),
                    label: None,
                    pref: None,
                },
            ],
            online_services: vec![OnlineService {
                key: "x-social".into(),
                service: Some("Mastodon".into()),
                uri: Some("https://example2.com/@alice".into()),
                user: Some("@alice@example2.com".into()),
                contexts: vec![],
                label: None,
                pref: None,
            }],
            addresses: vec![
                PostalAddress {
                    key: "k24".into(),
                    full: Some("4445 Tree Street\nNew England, ND 58647\nUSA".into()),
                    pref: Some(1),
                    ..PostalAddress::default()
                },
                PostalAddress {
                    key: "k23".into(),
                    full: None,
                    components: vec![
                        component("number", "54321"),
                        component("separator", " "),
                        component("name", "Oak St"),
                        component("locality", "Reston"),
                        component("region", "VA"),
                        component("separator", " "),
                        component("postcode", "20190"),
                        component("country", "USA"),
                    ],
                    is_ordered: true,
                    default_separator: Some(", ".into()),
                    country_code: Some("US".into()),
                    contexts: strings(&["work"]),
                    pref: None,
                },
            ],
            anniversaries: vec![
                Anniversary {
                    key: "k10".into(),
                    kind: "wedding".into(),
                    date: partial(None, Some(6), Some(21)),
                },
                Anniversary {
                    key: "k11".into(),
                    kind: "x-graduation".into(),
                    date: partial(Some(1975), None, None),
                },
                Anniversary {
                    key: "k8".into(),
                    kind: "birth".into(),
                    date: partial(Some(1953), Some(4), Some(15)),
                },
                Anniversary {
                    key: "k9".into(),
                    kind: "death".into(),
                    date: AnniversaryDate::Utc("2019-10-15T23:10:00Z".into()),
                },
            ],
            notes: vec![
                Note {
                    key: "n1".into(),
                    note: "Open office hours are 1600 to 1715 EST, Mon-Fri".into(),
                    created: Some("2022-11-23T15:01:32Z".into()),
                },
                Note {
                    key: "n2".into(),
                    note: "Prefers \"JQ\".\nCall after 10.".into(),
                    created: None,
                },
            ],
            members: vec![],
            address_book_ids: strings(&["b1", "b2"]),
            created: Some("2022-09-30T14:35:10Z".into()),
            updated: Some("2022-11-23T15:01:32Z".into()),
        };
        assert_eq!(full, &expected);
        assert_eq!(
            full.name.as_ref().unwrap().display().as_deref(),
            Some("Mr. John Quinlan Public, Esq.")
        );
        assert_eq!(
            full.addresses[1].display().as_deref(),
            Some("54321 Oak St, Reston, VA 20190, USA")
        );
        assert_eq!(
            full.addresses[0].display().as_deref(),
            Some("4445 Tree Street\nNew England, ND 58647\nUSA")
        );
    }

    #[test]
    fn the_minimal_card_reads_with_defaults() {
        assert_eq!(
            cards().list[1],
            ContactCard {
                id: "c2".into(),
                uid: None,
                kind: "individual".into(),
                name: None,
                nicknames: vec![],
                organizations: vec![],
                titles: vec![],
                emails: vec![],
                phones: vec![],
                online_services: vec![],
                addresses: vec![],
                anniversaries: vec![],
                notes: vec![],
                members: vec![],
                address_book_ids: strings(&["b1"]),
                created: None,
                updated: None,
            }
        );
    }

    #[test]
    fn a_group_reads_its_members_held_true_sorted() {
        let cards = cards();
        let group = &cards.list[2];
        assert_eq!(group.kind, "group");
        assert_eq!(
            group.name.as_ref().unwrap().display().as_deref(),
            Some("The Doe family")
        );
        assert_eq!(
            group.members,
            strings(&[
                "urn:uuid:03a0e51f-d1aa-4385-8a53-e29025acd8af",
                "urn:uuid:b8767877-b4a1-4c70-9acc-505d3819e519",
            ])
        );
    }

    #[test]
    fn preference_orders_a_list_independent_of_key_order() {
        let read = card(json!({
            "emails": {
                "a": {"address": "none-a@example.com"},
                "b": {"address": "three@example.com", "pref": 3},
                "c": {"address": "none-c@example.com"},
                "d": {"address": "one@example.com", "pref": 1},
                "e": {"address": "three-e@example.com", "pref": 3}
            },
            "phones": {
                "z": {"number": "1", "pref": 1},
                "a": {"number": "2", "pref": 50}
            }
        }))
        .unwrap();
        let addresses: Vec<&str> = read.emails.iter().map(|e| e.address.as_str()).collect();
        let keys: Vec<&str> = read.emails.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["d", "b", "e", "a", "c"]);
        assert_eq!(
            addresses,
            [
                "one@example.com",
                "three@example.com",
                "three-e@example.com",
                "none-a@example.com",
                "none-c@example.com",
            ]
        );
        let numbers: Vec<&str> = read.phones.iter().map(|p| p.number.as_str()).collect();
        assert_eq!(numbers, ["1", "2"]);
    }

    #[test]
    fn a_name_or_address_displays_full_or_its_components() {
        let name = |value: Value| card(json!({"name": value})).unwrap().name.unwrap();
        assert_eq!(
            name(json!({"full": "Alice Example", "components": [
                {"kind": "given", "value": "Ignored"}
            ]}))
            .display()
            .as_deref(),
            Some("Alice Example")
        );
        assert_eq!(
            name(json!({"full": "", "components": [
                {"kind": "given", "value": "Alice"}, {"kind": "surname", "value": "Example"}
            ], "isOrdered": true}))
            .display()
            .as_deref(),
            Some("Alice Example")
        );
        assert_eq!(
            name(json!({"components": [
                {"kind": "surname", "value": "Example"},
                {"kind": "given", "value": "Alice"},
                {"kind": "given2", "value": "B"}
            ], "isOrdered": true, "defaultSeparator": ", "}))
            .display()
            .as_deref(),
            Some("Example, Alice, B")
        );
        assert_eq!(
            name(json!({"components": [
                {"kind": "surname", "value": "Example"},
                {"kind": "separator", "value": "; "},
                {"kind": "given", "value": "Alice"},
                {"kind": "given2", "value": "B"}
            ], "isOrdered": true}))
            .display()
            .as_deref(),
            Some("Example; Alice B")
        );
        let unordered = name(json!({"components": [
            {"kind": "given", "value": "Alice"}, {"kind": "surname", "value": "Example"}
        ]}));
        assert!(!unordered.is_ordered);
        assert_eq!(unordered.display().as_deref(), Some("Alice Example"));
        assert_eq!(name(json!({})).display(), None);
        assert_eq!(name(json!({"full": ""})).display(), None);

        let address = |value: Value| {
            card(json!({"addresses": {"k1": value}}))
                .unwrap()
                .addresses
                .remove(0)
        };
        assert_eq!(
            address(json!({"full": "1 Main St"})).display().as_deref(),
            Some("1 Main St")
        );
        assert_eq!(
            address(json!({"components": [
                {"kind": "number", "value": "1"},
                {"kind": "name", "value": "Main St"},
                {"kind": "separator", "value": "\n"},
                {"kind": "locality", "value": "Springfield"}
            ]}))
            .display()
            .as_deref(),
            Some("1 Main St\nSpringfield")
        );
        assert_eq!(address(json!({"countryCode": "US"})).display(), None);
    }

    #[test]
    fn organization_units_are_objects_with_names() {
        let read = card(json!({"organizations": {
            "o1": {"units": [{"name": "Sales"}, {"name": "East", "sortAs": "E"}]}
        }}))
        .unwrap();
        assert_eq!(
            read.organizations,
            vec![Organization {
                key: "o1".into(),
                name: None,
                units: strings(&["Sales", "East"]),
            }]
        );
        assert_eq!(
            shape_message(card(json!({"organizations": {"o1": {"units": ["Sales"]}}}))),
            "ContactCard.organizations.units is not an object"
        );
        assert_eq!(
            shape_message(card(
                json!({"organizations": {"o1": {"units": [{"sortAs": "S"}]}}})
            )),
            "ContactCard.organizations.units.name is missing"
        );
    }

    #[test]
    fn an_anniversary_reads_each_date_rfc_9553_allows_and_refuses_the_rest() {
        let date = |date: Value| {
            card(json!({"anniversaries": {"k1": {"kind": "birth", "date": date}}}))
                .map(|mut card| card.anniversaries.remove(0).date)
        };
        let readable = [
            (json!({"year": 1953}), partial(Some(1953), None, None)),
            (
                json!({"year": 1953, "month": 4}),
                partial(Some(1953), Some(4), None),
            ),
            (
                json!({"@type": "PartialDate", "year": 1953, "month": 4, "day": 15}),
                partial(Some(1953), Some(4), Some(15)),
            ),
            (
                json!({"month": 12, "day": 31, "calendarScale": "gregorian"}),
                partial(None, Some(12), Some(31)),
            ),
            (
                json!({"@type": "Timestamp", "utc": "2019-10-15T23:10:00Z"}),
                AnniversaryDate::Utc("2019-10-15T23:10:00Z".into()),
            ),
        ];
        for (value, expected) in readable {
            assert_eq!(date(value.clone()).unwrap(), expected, "{value}");
        }
        let combination = "ContactCard.anniversaries.date is not a year, a year and month, \
                           a year, month and day, or a month and day";
        let refused = [
            (json!({}), combination),
            (json!({"month": 4}), combination),
            (json!({"day": 15}), combination),
            (json!({"year": 1953, "day": 15}), combination),
            (
                json!({"month": 13, "day": 1}),
                "ContactCard.anniversaries.date.month is outside 1 to 12",
            ),
            (
                json!({"month": 1, "day": 0}),
                "ContactCard.anniversaries.date.day is outside 1 to 31",
            ),
            (
                json!({"year": "1953"}),
                "ContactCard.anniversaries.date.year is not an unsigned integer",
            ),
            (
                json!({"@type": "Timestamp"}),
                "ContactCard.anniversaries.date.utc is missing",
            ),
            (
                json!({"@type": "Hijri", "year": 1400}),
                "ContactCard.anniversaries.date.@type is neither PartialDate nor Timestamp",
            ),
            (
                json!("1953-04-15"),
                "ContactCard.anniversaries.date is not an object",
            ),
        ];
        for (value, message) in refused {
            assert_eq!(shape_message(date(value.clone())), message, "{value}");
        }
        assert_eq!(
            shape_message(card(json!({"anniversaries": {"k1": {"kind": "birth"}}}))),
            "ContactCard.anniversaries.date is missing"
        );
    }

    #[test]
    fn a_wrong_typed_property_is_refused_naming_object_and_property() {
        let cases = [
            (json!({"emails": []}), "ContactCard.emails is not an object"),
            (
                json!({"emails": {"e1": {"address": 7}}}),
                "ContactCard.emails.address is not a string",
            ),
            (
                json!({"emails": {"e1": {"address": "a@example.com", "pref": "1"}}}),
                "ContactCard.emails.pref is not an unsigned integer",
            ),
            (
                json!({"phones": {"p1": {"number": "1", "features": {"voice": "yes"}}}}),
                "ContactCard.phones.features holds a non-boolean",
            ),
            (
                json!({"phones": {"p1": {}}}),
                "ContactCard.phones.number is missing",
            ),
            (json!({"kind": 1}), "ContactCard.kind is not a string"),
            (
                json!({"name": "Alice"}),
                "ContactCard.name is not an object",
            ),
            (
                json!({"name": {"components": [{"kind": "given"}]}}),
                "ContactCard.name.components.value is missing",
            ),
            (
                json!({"addresses": {"k1": {"components": [{"value": "1"}]}}}),
                "ContactCard.addresses.components.kind is missing",
            ),
            (
                json!({"nicknames": {"k1": {}}}),
                "ContactCard.nicknames.name is missing",
            ),
            (
                json!({"titles": {"t1": {"name": "Boss", "organizationId": 2}}}),
                "ContactCard.titles.organizationId is not a string",
            ),
            (
                json!({"onlineServices": {"s1": {"uri": false}}}),
                "ContactCard.onlineServices.uri is not a string",
            ),
            (
                json!({"notes": {"n1": {"created": "2022-11-23T15:01:32Z"}}}),
                "ContactCard.notes.note is missing",
            ),
            (
                json!({"members": ["urn:uuid:1"]}),
                "ContactCard.members is not an object",
            ),
            (
                json!({"addressBookIds": {"b1": 1}}),
                "ContactCard.addressBookIds holds a non-boolean",
            ),
        ];
        for (value, message) in cases {
            assert_eq!(shape_message(card(value.clone())), message, "{value}");
        }
        let without_id = json!({"list": [{"kind": "individual"}], "notFound": []});
        assert!(matches!(
            ContactCard::get(&without_id),
            Err(Error::Shape(message)) if message == "ContactCard.id is missing"
        ));
        let one_bad = json!({"list": [{"id": "c1"}, {"id": "c2", "kind": 1}, {"id": "c3"}],
                             "notFound": []});
        assert!(matches!(
            ContactCard::get(&one_bad),
            Err(Error::Shape(message))
                if message == "ContactCard.kind is not a string (card `c2`)"
        ));
    }

    #[test]
    fn properties_the_tools_do_not_show_are_ignored() {
        let read = card(json!({
            "@type": "Card",
            "version": "1.0",
            "language": 7,
            "media": "not even an object",
            "relatedTo": [],
            "keywords": {"a": "b"},
            "example.com:extension": {"anything": [1, 2]}
        }))
        .unwrap();
        assert_eq!(read.id, "c1");
        assert_eq!(read.kind, "individual");
    }

    #[test]
    fn a_partial_date_parses_each_iso_form_and_refuses_the_rest() {
        for (text, expected) in [
            ("1953", partial(Some(1953), None, None)),
            ("1953-04", partial(Some(1953), Some(4), None)),
            ("1953-04-15", partial(Some(1953), Some(4), Some(15))),
            ("--12-31", partial(None, Some(12), Some(31))),
            ("0000-01-01", partial(Some(0), Some(1), Some(1))),
        ] {
            assert_eq!(
                AnniversaryDate::parse_partial(text),
                Some(expected),
                "{text}"
            );
        }
        for text in [
            "",
            "195",
            "19530",
            "1953-4",
            "1953-04-5",
            "1953-13",
            "1953-00",
            "1953-04-32",
            "1953-04-00",
            "--04",
            "--13-01",
            "--04-15-1",
            "1953-04-15-1",
            "+953",
            "1953--04",
            "--",
            "abcd",
            "1953-0a",
        ] {
            assert_eq!(AnniversaryDate::parse_partial(text), None, "{text}");
        }
    }

    #[test]
    fn a_partial_date_reads_back_the_text_it_is_written_as() {
        for text in ["1953", "1953-04", "1953-04-15", "--04-15", "0000-01-01"] {
            let date = AnniversaryDate::parse_partial(text).expect("a partial date");
            assert_eq!(date.to_text(), text);
        }
        let utc = AnniversaryDate::Utc("1953-04-15T00:00:00Z".to_owned());
        assert_eq!(utc.to_text(), "1953-04-15T00:00:00Z");
    }

    #[test]
    fn a_date_is_written_as_jscontact_types_it() {
        assert_eq!(
            partial(Some(1953), Some(4), Some(15)).to_value(),
            json!({"@type": "PartialDate", "year": 1953, "month": 4, "day": 15})
        );
        assert_eq!(
            partial(None, Some(6), Some(21)).to_value(),
            json!({"@type": "PartialDate", "month": 6, "day": 21})
        );
        assert_eq!(
            AnniversaryDate::Utc("2019-10-15T23:10:00Z".into()).to_value(),
            json!({"@type": "Timestamp", "utc": "2019-10-15T23:10:00Z"})
        );
    }

    fn text(value: &str) -> Option<EntryValue> {
        Some(EntryValue::Text(value.into()))
    }

    /// Every field given, the value as `list` takes it.
    fn every_field(list: EntryList) -> EntryFields {
        EntryFields {
            value: match list {
                EntryList::Anniversaries => Some(EntryValue::Date(partial(Some(1990), None, None))),
                _ => text("v"),
            },
            label: Some("home".into()),
            contexts: Some(strings(&["private", "work"])),
            features: Some(strings(&["voice"])),
            pref: Some(3),
            kind: Some("role".into()),
            service: Some("Mastodon".into()),
            user: Some("@alice".into()),
            country_code: Some("US".into()),
        }
    }

    #[test]
    fn each_list_names_its_main_value_by_its_jscontact_sub_property() {
        let expected = [
            (EntryList::Emails, "emails", "address"),
            (EntryList::Phones, "phones", "number"),
            (EntryList::OnlineServices, "onlineServices", "uri"),
            (EntryList::Addresses, "addresses", "full"),
            (EntryList::Organizations, "organizations", "name"),
            (EntryList::Titles, "titles", "name"),
            (EntryList::Nicknames, "nicknames", "name"),
            (EntryList::Anniversaries, "anniversaries", "date"),
            (EntryList::Notes, "notes", "note"),
        ];
        assert_eq!(expected.map(|(list, ..)| list), EntryList::ALL);
        for (list, property, value) in expected {
            assert_eq!(list.property(), property);
            let fields = every_field(list);
            let main = match list {
                EntryList::Anniversaries => json!({"@type": "PartialDate", "year": 1990}),
                _ => json!("v"),
            };
            assert_eq!(
                fields.entry(list),
                json!({
                    value: main, "label": "home", "kind": "role", "service": "Mastodon",
                    "user": "@alice", "countryCode": "US",
                    "contexts": {"private": true, "work": true}, "features": {"voice": true},
                    "pref": 3
                }),
                "{property}"
            );
            let held = BTreeSet::from(["e1".to_owned()]);
            let edit = EntryChange::Edit {
                key: "e1".into(),
                fields: EntryFields {
                    value: fields.value.clone(),
                    ..EntryFields::default()
                },
            };
            let (patch, key) = edit.patch(list, Some(&held), Patch::new()).unwrap();
            assert_eq!(key, None);
            let mut paths = json!({ format!("{property}/e1/{value}"): main });
            if list == EntryList::Addresses {
                for sub in ["components", "isOrdered", "defaultSeparator"] {
                    paths[format!("addresses/e1/{sub}")] = Value::Null;
                }
            }
            assert_eq!(patch.to_value(), paths, "{property}");
        }
        assert_eq!(EntryFields::default().entry(EntryList::Emails), json!({}));
    }

    #[test]
    fn an_add_takes_the_smallest_free_key_and_writes_an_absent_list_whole() {
        let add = EntryChange::Add(EntryFields {
            value: text("alice@example.com"),
            label: Some("home".into()),
            ..EntryFields::default()
        });
        let entry = json!({"address": "alice@example.com", "label": "home"});
        let held = BTreeSet::from(["k1".to_owned(), "k3".to_owned(), "x".to_owned()]);
        let (patch, key) = add
            .patch(EntryList::Emails, Some(&held), Patch::new())
            .unwrap();
        assert_eq!(key.as_deref(), Some("k2"));
        assert_eq!(patch.to_value(), json!({"emails/k2": entry}));
        let (patch, key) = add
            .patch(EntryList::Emails, Some(&BTreeSet::new()), Patch::new())
            .unwrap();
        assert_eq!(key.as_deref(), Some("k1"));
        assert_eq!(patch.to_value(), json!({"emails/k1": entry}));
        let (patch, key) = add.patch(EntryList::Emails, None, Patch::new()).unwrap();
        assert_eq!(key.as_deref(), Some("k1"));
        assert_eq!(patch.to_value(), json!({"emails": {"k1": entry}}));
    }

    #[test]
    fn an_edit_writes_only_the_sub_properties_named_and_a_remove_only_the_entry() {
        let held = BTreeSet::from(["e/1".to_owned(), "e2".to_owned()]);
        let edit = EntryChange::Edit {
            key: "e/1".into(),
            fields: EntryFields {
                contexts: Some(strings(&["work"])),
                pref: Some(1),
                ..EntryFields::default()
            },
        };
        let base = Patch::new().at(&["name", "full"], json!("Alice"));
        let (patch, _) = edit.patch(EntryList::Phones, Some(&held), base).unwrap();
        assert_eq!(
            patch.to_value(),
            json!({
                "name/full": "Alice",
                "phones/e~11/contexts": {"work": true},
                "phones/e~11/pref": 1
            })
        );
        let edit = EntryChange::Edit {
            key: "e2".into(),
            fields: EntryFields {
                country_code: Some("US".into()),
                ..EntryFields::default()
            },
        };
        let (patch, _) = edit
            .patch(EntryList::Addresses, Some(&held), Patch::new())
            .unwrap();
        assert_eq!(patch.to_value(), json!({"addresses/e2/countryCode": "US"}));
        let remove = EntryChange::Remove { key: "e2".into() };
        let (patch, key) = remove
            .patch(EntryList::Notes, Some(&held), Patch::new())
            .unwrap();
        assert_eq!(key, None);
        assert_eq!(patch.to_value(), json!({"notes/e2": null}));
        for change in [
            EntryChange::Remove { key: "e3".into() },
            EntryChange::Edit {
                key: "e3".into(),
                fields: EntryFields::of_value(EntryValue::Text("x".into())),
            },
        ] {
            assert_eq!(
                change.patch(EntryList::Notes, Some(&held), Patch::new()),
                None
            );
            assert_eq!(change.patch(EntryList::Notes, None, Patch::new()), None);
        }
    }

    fn name_state(name: Value) -> NameState {
        let (_, mut cards) = CardKeys::get(&json!({
            "state": "s1", "list": [{"id": "c1", "name": name}], "notFound": []
        }))
        .unwrap();
        cards.list.remove(0).name.unwrap()
    }

    fn change(full: Option<&str>, given: Option<&str>, surname: Option<&str>) -> NameChange {
        NameChange {
            full: full.map(str::to_owned),
            given: given.map(str::to_owned),
            surname: surname.map(str::to_owned),
        }
    }

    #[test]
    fn a_name_change_on_a_card_without_a_name_writes_the_whole_property() {
        let name = change(Some("Alice Example"), Some("Alice"), Some("Example"));
        let whole = json!({"full": "Alice Example", "components": [
            {"kind": "given", "value": "Alice"}, {"kind": "surname", "value": "Example"}
        ]});
        assert_eq!(name.to_value(), whole);
        assert_eq!(
            name.patch(None, Patch::new()).unwrap().to_value(),
            json!({"name": whole})
        );
        assert_eq!(
            change(None, None, Some("Example")).to_value(),
            json!({"components": [{"kind": "surname", "value": "Example"}]})
        );
        assert_eq!(
            change(Some("A"), None, None).to_value(),
            json!({"full": "A"})
        );
    }

    #[test]
    fn a_name_change_patches_full_and_rewrites_only_the_named_components() {
        let held = name_state(json!({
            "full": "Dr. Alice B. Example PhD",
            "sortAs": {"surname": "Example"},
            "isOrdered": false,
            "example.com:vendor": 1,
            "components": [
                {"kind": "title", "value": "Dr."},
                {"kind": "given", "value": "Alice"},
                {"kind": "surname", "value": "Example", "phonetic": "eg-zam-pul"},
                {"kind": "credential", "value": "PhD", "example.com:x": true}
            ]
        }));
        assert_eq!(
            change(Some("Alice Sample"), None, None)
                .patch(Some(&held), Patch::new())
                .unwrap()
                .to_value(),
            json!({"name/full": "Alice Sample"})
        );
        assert_eq!(
            change(Some("Dr. Alice Sample PhD"), None, Some("Sample"))
                .patch(Some(&held), Patch::new())
                .unwrap()
                .to_value(),
            json!({
                "name/full": "Dr. Alice Sample PhD",
                "name/components": [
                    {"kind": "title", "value": "Dr."},
                    {"kind": "given", "value": "Alice"},
                    {"kind": "surname", "value": "Sample", "phonetic": "eg-zam-pul"},
                    {"kind": "credential", "value": "PhD", "example.com:x": true}
                ]
            })
        );
        let unordered = name_state(json!({"components": [{"kind": "surname", "value": "E"}]}));
        assert_eq!(
            change(None, Some("Alice"), Some("Example"))
                .patch(Some(&unordered), Patch::new())
                .unwrap()
                .to_value(),
            json!({"name/components": [
                {"kind": "surname", "value": "Example"}, {"kind": "given", "value": "Alice"}
            ]})
        );
        let bare = name_state(json!({"full": ""}));
        assert_eq!(
            change(None, Some("Alice"), Some("Example"))
                .patch(Some(&bare), Patch::new())
                .unwrap()
                .to_value(),
            json!({"name/components": [
                {"kind": "given", "value": "Alice"}, {"kind": "surname", "value": "Example"}
            ]})
        );
    }

    #[test]
    fn a_name_change_that_would_be_a_guess_is_refused() {
        let full = name_state(json!({"full": "Alice Example", "components": [
            {"kind": "given", "value": "Alice"}
        ]}));
        assert_eq!(
            change(None, Some("Al"), None).patch(Some(&full), Patch::new()),
            Err(NameRefusal::FullHidesParts)
        );
        let twice = name_state(json!({"components": [
            {"kind": "surname", "value": "A"}, {"kind": "surname", "value": "B"}
        ]}));
        assert_eq!(
            change(Some("X"), None, Some("C")).patch(Some(&twice), Patch::new()),
            Err(NameRefusal::Ambiguous("surname"))
        );
        let ordered = name_state(json!({"isOrdered": true, "components": [
            {"kind": "given", "value": "Alice"}
        ]}));
        assert_eq!(
            change(Some("Alice Example"), Some("Alice"), Some("Example"))
                .patch(Some(&ordered), Patch::new()),
            Err(NameRefusal::Unplaceable("surname"))
        );
        let ordered_empty = name_state(json!({"isOrdered": true}));
        assert_eq!(
            change(None, Some("Alice"), None).patch(Some(&ordered_empty), Patch::new()),
            Err(NameRefusal::Unplaceable("given"))
        );
    }

    #[test]
    fn card_keys_read_the_state_each_present_lists_keys_and_the_name() {
        let (state, result) = CardKeys::get(&json!({
            "accountId": "a1", "state": "s9", "notFound": ["c9"],
            "list": [
                {"id": "c1", "emails": {"e1": {"address": "a"}, "e2": {}}, "phones": null,
                 "onlineServices": {}, "name": {"full": "Alice", "isOrdered": true,
                 "components": [{"kind": "given", "value": "Alice", "phonetic": "a"}]}},
                {"id": "c2"}
            ]
        }))
        .unwrap();
        assert_eq!(state, "s9");
        assert_eq!(result.not_found, strings(&["c9"]));
        let card = &result.list[0];
        assert_eq!(card.id, "c1");
        assert_eq!(
            card.keys(EntryList::Emails),
            Some(&BTreeSet::from(["e1".to_owned(), "e2".to_owned()]))
        );
        assert_eq!(card.keys(EntryList::OnlineServices), Some(&BTreeSet::new()));
        assert_eq!(card.keys(EntryList::Phones), None);
        assert_eq!(card.keys(EntryList::Notes), None);
        assert_eq!(
            card.name,
            Some(NameState {
                has_full: true,
                components: vec![json!({"kind": "given", "value": "Alice", "phonetic": "a"})],
                is_ordered: true,
            })
        );
        let bare = &result.list[1];
        assert!(EntryList::ALL.iter().all(|&list| bare.keys(list).is_none()));
        assert_eq!(bare.name, None);
    }

    #[test]
    fn card_keys_of_the_wrong_shape_are_malformed() {
        let get = |card: Value| json!({"state": "s1", "list": [card], "notFound": []});
        for args in [
            json!({"list": [{"id": "c1"}], "notFound": []}),
            json!({"state": 7, "list": [], "notFound": []}),
            get(json!({"emails": {}})),
            get(json!({"id": "c1", "emails": []})),
            get(json!({"id": "c1", "anniversaries": "x"})),
            get(json!({"id": "c1", "name": "Alice"})),
            get(json!({"id": "c1", "name": {"full": 7}})),
            get(json!({"id": "c1", "name": {"isOrdered": "yes"}})),
            get(json!({"id": "c1", "name": {"components": {}}})),
            get(json!({"id": "c1", "name": {"components": ["given"]}})),
            get(json!({"id": "c1", "name": {"components": [{"value": "Alice"}]}})),
            get(json!({"id": "c1", "name": {"components": [{"kind": 1}]}})),
        ] {
            assert!(
                matches!(CardKeys::get(&args), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }

    #[test]
    fn a_new_card_carries_the_mandatory_properties_and_bare_entries() {
        let full = NewCard {
            uid: "urn:uuid:0".into(),
            address_book: "b1".into(),
            name: Some(change(
                Some("Alice Example"),
                Some("Alice"),
                Some("Example"),
            )),
            lists: vec![
                (EntryList::Emails, strings(&["alice@example.com"])),
                (EntryList::Phones, strings(&["+1 555 0100", "+1 555 0101"])),
                (
                    EntryList::OnlineServices,
                    strings(&["https://example.com/@alice"]),
                ),
                (EntryList::Addresses, strings(&["1 Main St\nSpringfield"])),
                (EntryList::Organizations, strings(&["Example Inc."])),
                (EntryList::Titles, strings(&["Engineer"])),
                (EntryList::Nicknames, strings(&["Al"])),
                (EntryList::Notes, strings(&["Met at the conference."])),
            ],
            birthday: Some(partial(Some(1990), Some(4), Some(15))),
        };
        assert_eq!(
            full.to_value(),
            json!({
                "@type": "Card", "version": "1.0", "uid": "urn:uuid:0",
                "addressBookIds": {"b1": true},
                "name": {"full": "Alice Example", "components": [
                    {"kind": "given", "value": "Alice"}, {"kind": "surname", "value": "Example"}
                ]},
                "emails": {"k1": {"address": "alice@example.com"}},
                "phones": {"k1": {"number": "+1 555 0100"}, "k2": {"number": "+1 555 0101"}},
                "onlineServices": {"k1": {"uri": "https://example.com/@alice"}},
                "addresses": {"k1": {"full": "1 Main St\nSpringfield"}},
                "organizations": {"k1": {"name": "Example Inc."}},
                "titles": {"k1": {"name": "Engineer"}},
                "nicknames": {"k1": {"name": "Al"}},
                "notes": {"k1": {"note": "Met at the conference."}},
                "anniversaries": {"k1": {"kind": "birth", "date": {
                    "@type": "PartialDate", "year": 1990, "month": 4, "day": 15
                }}}
            })
        );
        let minimal = NewCard {
            uid: "urn:uuid:1".into(),
            address_book: "b2".into(),
            name: None,
            lists: vec![(EntryList::Notes, strings(&["x"]))],
            birthday: None,
        };
        assert_eq!(
            minimal.to_value(),
            json!({
                "@type": "Card", "version": "1.0", "uid": "urn:uuid:1",
                "addressBookIds": {"b2": true}, "notes": {"k1": {"note": "x"}}
            })
        );
    }

    #[test]
    fn a_card_set_sends_its_creates_and_updates_and_reads_its_result() {
        let mut request = Request::contacts();
        let call = ContactCardSet {
            account_id: "a1",
            if_in_state: None,
            create: vec![(
                "contact".into(),
                NewCard {
                    uid: "urn:uuid:0".into(),
                    address_book: "b1".into(),
                    name: Some(change(Some("Alice"), None, None)),
                    lists: vec![],
                    birthday: None,
                },
            )],
            update: vec![],
        }
        .push(&mut request);
        assert_eq!(call.name(), "ContactCard/set");
        ContactCardSet {
            account_id: "a1",
            if_in_state: Some("s9"),
            create: vec![],
            update: vec![(
                "c1".into(),
                Patch::new()
                    .at(&["emails", "e1", "address"], json!("alice@example.com"))
                    .at(&["addressBookIds", "b2"], json!(true)),
            )],
        }
        .push(&mut request);
        assert_eq!(
            body(&request)["methodCalls"],
            json!([
                ["ContactCard/set", {"accountId": "a1", "create": {"contact": {
                    "@type": "Card", "version": "1.0", "uid": "urn:uuid:0",
                    "addressBookIds": {"b1": true}, "name": {"full": "Alice"}
                }}}, "c0"],
                ["ContactCard/set", {"accountId": "a1", "ifInState": "s9", "update": {"c1": {
                    "emails/e1/address": "alice@example.com", "addressBookIds/b2": true
                }}}, "c1"]
            ])
        );
        let result = ContactCardSet::result(&json!({
            "accountId": "a1", "oldState": "s1", "newState": "s2",
            "created": {"contact": {"id": "c7", "uid": "urn:uuid:0"}},
            "updated": {"c1": null}
        }))
        .unwrap();
        assert_eq!(
            result.created,
            BTreeMap::from([("contact".to_owned(), CardCreated { id: "c7".into() })])
        );
        assert_eq!(result.updated, strings(&["c1"]));
        assert!(matches!(
            ContactCardSet::result(&json!({"created": {"contact": {}}})),
            Err(Error::Shape(_))
        ));
    }
}
