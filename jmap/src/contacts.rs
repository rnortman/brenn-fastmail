//! The contacts methods this crate builds and reads (RFC 9610):
//! `AddressBook/get`, `ContactCard/query` and `ContactCard/get`. A card is
//! JSContact (RFC 9553); the reader takes the properties the tools show, each
//! with the type RFC 9553 gives it, and ignores every other property.
//!
//! JSContact's lists are `Id`-keyed maps with no order of their own. Each is
//! read into a `Vec` ordered by `pref` ascending where the type has one (RFC
//! 9553 section 1.5.3: lower is preferred), entries without `pref` after those
//! with, and ties and the types without `pref` by map key.

use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::method::{Fields, GetResult, Ids, Sort, read_get};
use crate::request::{CallId, Request};

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

/// An `Organization` (RFC 9553 section 2.2.3), with its map key as `id` so a
/// title can name it. `units` are the `OrgUnit` names, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub id: String,
    pub name: Option<String>,
    pub units: Vec<String>,
}

impl Organization {
    fn read(value: &Value) -> Result<Organization, Error> {
        let f = Fields::of(value, "ContactCard.organizations")?;
        Ok(Organization {
            id: String::new(),
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
    pub name: String,
    pub kind: String,
    pub organization_id: Option<String>,
}

impl Title {
    fn read(value: &Value) -> Result<Title, Error> {
        let f = Fields::of(value, "ContactCard.titles")?;
        Ok(Title {
            name: f.string("name")?,
            kind: f.opt_string("kind")?.unwrap_or_else(|| "title".into()),
            organization_id: f.opt_string("organizationId")?,
        })
    }
}

/// An `EmailAddress` (RFC 9553 section 2.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardEmail {
    pub address: String,
    pub contexts: Vec<String>,
    pub label: Option<String>,
    pub pref: Option<u64>,
}

impl CardEmail {
    fn read(value: &Value) -> Result<CardEmail, Error> {
        let f = Fields::of(value, "ContactCard.emails")?;
        Ok(CardEmail {
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
    pub number: String,
    pub features: Vec<String>,
    pub contexts: Vec<String>,
    pub label: Option<String>,
    pub pref: Option<u64>,
}

impl Phone {
    fn read(value: &Value) -> Result<Phone, Error> {
        let f = Fields::of(value, "ContactCard.phones")?;
        Ok(Phone {
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
    pub service: Option<String>,
    pub uri: Option<String>,
    pub user: Option<String>,
    pub contexts: Vec<String>,
    pub label: Option<String>,
    pub pref: Option<u64>,
}

impl OnlineService {
    fn read(value: &Value) -> Result<OnlineService, Error> {
        let f = Fields::of(value, "ContactCard.onlineServices")?;
        Ok(OnlineService {
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
    pub full: Option<String>,
    pub components: Vec<Component>,
    pub is_ordered: bool,
    pub default_separator: Option<String>,
    pub country_code: Option<String>,
    pub contexts: Vec<String>,
    pub pref: Option<u64>,
}

impl PostalAddress {
    fn read(value: &Value) -> Result<PostalAddress, Error> {
        let f = Fields::of(value, "ContactCard.addresses")?;
        Ok(PostalAddress {
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
    pub kind: String,
    pub date: AnniversaryDate,
}

impl Anniversary {
    fn read(value: &Value) -> Result<Anniversary, Error> {
        let f = Fields::of(value, "ContactCard.anniversaries")?;
        let date = f.get("date").ok_or_else(|| f.shape("date", "is missing"))?;
        Ok(Anniversary {
            kind: f.string("kind")?,
            date: AnniversaryDate::read(date)?,
        })
    }
}

/// A `Note` (RFC 9553 section 2.8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub note: String,
    pub created: Option<String>,
}

impl Note {
    fn read(value: &Value) -> Result<Note, Error> {
        let f = Fields::of(value, "ContactCard.notes")?;
        Ok(Note {
            note: f.string("note")?,
            created: f.opt_string("created")?,
        })
    }
}

/// A `Nickname` (RFC 9553 section 2.2.2): only its name is shown, but its
/// `pref` orders the list.
struct Nickname {
    name: String,
    pref: Option<u64>,
}

impl Nickname {
    fn read(value: &Value) -> Result<Nickname, Error> {
        let f = Fields::of(value, "ContactCard.nicknames")?;
        Ok(Nickname {
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
    pub nicknames: Vec<String>,
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
        let organizations = in_key_order(f, "organizations", Organization::read)?
            .into_iter()
            .map(|(id, organization)| Organization { id, ..organization })
            .collect();
        Ok(ContactCard {
            id: id.to_owned(),
            uid: f.opt_string("uid")?,
            kind: f.opt_string("kind")?.unwrap_or_else(|| "individual".into()),
            name: f.get("name").map(CardName::read).transpose()?,
            nicknames: by_pref(f, "nicknames", Nickname::read, |n| n.pref)?
                .into_iter()
                .map(|nickname| nickname.name)
                .collect(),
            organizations,
            titles: values(in_key_order(f, "titles", Title::read)?),
            emails: by_pref(f, "emails", CardEmail::read, |e| e.pref)?,
            phones: by_pref(f, "phones", Phone::read, |p| p.pref)?,
            online_services: by_pref(f, "onlineServices", OnlineService::read, |s| s.pref)?,
            addresses: by_pref(f, "addresses", PostalAddress::read, |a| a.pref)?,
            anniversaries: values(in_key_order(f, "anniversaries", Anniversary::read)?),
            notes: values(in_key_order(f, "notes", Note::read)?),
            members: held(f, "members")?,
            address_book_ids: held(f, "addressBookIds")?,
            created: f.opt_string("created")?,
            updated: f.opt_string("updated")?,
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

/// An `Id`-keyed map's entries through `read`, by key.
fn in_key_order<T>(
    f: &Fields<'_>,
    key: &str,
    read: fn(&Value) -> Result<T, Error>,
) -> Result<Vec<(String, T)>, Error> {
    Ok(f.map(key, read)?.into_iter().collect())
}

/// An `Id`-keyed map's values through `read`, by `pref` ascending, entries
/// without `pref` last, ties by key.
fn by_pref<T>(
    f: &Fields<'_>,
    key: &str,
    read: fn(&Value) -> Result<T, Error>,
    pref: fn(&T) -> Option<u64>,
) -> Result<Vec<T>, Error> {
    let mut entries = values(in_key_order(f, key, read)?);
    entries.sort_by_key(|entry| (pref(entry).is_none(), pref(entry)));
    Ok(entries)
}

fn values<T>(entries: Vec<(String, T)>) -> Vec<T> {
    entries.into_iter().map(|(_, value)| value).collect()
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
            nicknames: strings(&["JQ", "Johnny"]),
            organizations: vec![
                Organization {
                    id: "o1".into(),
                    name: None,
                    units: strings(&["Volunteers"]),
                },
                Organization {
                    id: "o2".into(),
                    name: Some("ABC, Inc.".into()),
                    units: strings(&["North American Division", "Marketing"]),
                },
            ],
            titles: vec![
                Title {
                    name: "Treasurer".into(),
                    kind: "title".into(),
                    organization_id: None,
                },
                Title {
                    name: "Project Leader".into(),
                    kind: "title".into(),
                    organization_id: Some("o2".into()),
                },
            ],
            emails: vec![
                CardEmail {
                    address: "alice@example.com".into(),
                    contexts: strings(&["private"]),
                    label: Some("home".into()),
                    pref: Some(1),
                },
                CardEmail {
                    address: "jane_doe@example.com".into(),
                    contexts: vec![],
                    label: None,
                    pref: Some(2),
                },
                CardEmail {
                    address: "jqpublic@xyz.example.com".into(),
                    contexts: strings(&["work"]),
                    label: None,
                    pref: None,
                },
            ],
            phones: vec![
                Phone {
                    number: "tel:+1-555-555-5555;ext=5555".into(),
                    features: strings(&["voice"]),
                    contexts: strings(&["private"]),
                    label: None,
                    pref: Some(1),
                },
                Phone {
                    number: "tel:+1-201-555-0123".into(),
                    features: strings(&["text", "voice"]),
                    contexts: strings(&["work"]),
                    label: None,
                    pref: None,
                },
            ],
            online_services: vec![OnlineService {
                service: Some("Mastodon".into()),
                uri: Some("https://example2.com/@alice".into()),
                user: Some("@alice@example2.com".into()),
                contexts: vec![],
                label: None,
                pref: None,
            }],
            addresses: vec![
                PostalAddress {
                    full: Some("4445 Tree Street\nNew England, ND 58647\nUSA".into()),
                    pref: Some(1),
                    ..PostalAddress::default()
                },
                PostalAddress {
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
                    kind: "wedding".into(),
                    date: partial(None, Some(6), Some(21)),
                },
                Anniversary {
                    kind: "x-graduation".into(),
                    date: partial(Some(1975), None, None),
                },
                Anniversary {
                    kind: "birth".into(),
                    date: partial(Some(1953), Some(4), Some(15)),
                },
                Anniversary {
                    kind: "death".into(),
                    date: AnniversaryDate::Utc("2019-10-15T23:10:00Z".into()),
                },
            ],
            notes: vec![
                Note {
                    note: "Open office hours are 1600 to 1715 EST, Mon-Fri".into(),
                    created: Some("2022-11-23T15:01:32Z".into()),
                },
                Note {
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
                id: "o1".into(),
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
}
