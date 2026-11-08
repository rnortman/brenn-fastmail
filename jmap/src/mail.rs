//! The mail methods this crate builds and reads (RFC 8621): `Mailbox/get`,
//! `Email/query`, `Email/get` and `Thread/get`. Each builder pushes one call
//! with its own method name; each reader takes the arguments
//! `Response::result` returned for that call and refuses any value whose type
//! is not the one RFC 8621 gives the property.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::request::{CallId, Request};

/// The properties `Mailbox/get` asks for; `Mailbox::list` reads these.
pub const MAILBOX_PROPERTIES: &[&str] = &[
    "id",
    "name",
    "parentId",
    "role",
    "sortOrder",
    "totalEmails",
    "unreadEmails",
    "totalThreads",
    "unreadThreads",
];

/// The properties of an email summary.
pub const SUMMARY_PROPERTIES: &[&str] = &[
    "id",
    "threadId",
    "mailboxIds",
    "keywords",
    "size",
    "receivedAt",
    "from",
    "to",
    "subject",
    "preview",
    "hasAttachment",
];

/// The properties of a full read: the summary's, the remaining headers, and
/// the text body with its values.
pub const READ_PROPERTIES: &[&str] = &[
    "id",
    "threadId",
    "mailboxIds",
    "keywords",
    "size",
    "receivedAt",
    "from",
    "to",
    "subject",
    "preview",
    "hasAttachment",
    "cc",
    "bcc",
    "replyTo",
    "sentAt",
    "messageId",
    "inReplyTo",
    "references",
    "textBody",
    "attachments",
    "bodyValues",
];

/// `Mailbox/get` over every mailbox in the account (`ids` null).
#[derive(Debug, Clone)]
pub struct MailboxGet<'a> {
    pub account_id: &'a str,
    pub properties: &'a [&'a str],
}

impl MailboxGet<'_> {
    pub const METHOD: &'static str = "Mailbox/get";

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

/// One mailbox (RFC 8621 section 2). A count the server leaves out reads as
/// zero; so does an absent `sortOrder`, its RFC default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub sort_order: u32,
    pub total_emails: u64,
    pub unread_emails: u64,
    pub total_threads: u64,
    pub unread_threads: u64,
}

impl Mailbox {
    /// The mailboxes in a `Mailbox/get` result, in the server's order.
    pub fn list(args: &Value) -> Result<Vec<Mailbox>, Error> {
        Ok(read_get(args, Mailbox::read)?.list)
    }

    fn read(value: &Value) -> Result<Mailbox, Error> {
        let f = Fields::of(value, "Mailbox")?;
        let sort_order = f.u64_or_zero("sortOrder")?;
        Ok(Mailbox {
            id: f.string("id")?,
            name: f.string("name")?,
            parent_id: f.opt_string("parentId")?,
            role: f.opt_string("role")?,
            sort_order: u32::try_from(sort_order)
                .map_err(|_| f.shape("sortOrder", "is over 2^32"))?,
            total_emails: f.u64_or_zero("totalEmails")?,
            unread_emails: f.u64_or_zero("unreadEmails")?,
            total_threads: f.u64_or_zero("totalThreads")?,
            unread_threads: f.u64_or_zero("unreadThreads")?,
        })
    }
}

/// An `Email/query` filter (RFC 8621 section 4.4.1): a condition, or an
/// operator over filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    Condition(FilterCondition),
    And(Vec<Filter>),
    Not(Box<Filter>),
}

impl Filter {
    pub fn to_value(&self) -> Value {
        match self {
            Filter::Condition(condition) => condition.to_value(),
            Filter::And(filters) => json!({
                "operator": "AND",
                "conditions": filters.iter().map(Filter::to_value).collect::<Vec<_>>(),
            }),
            Filter::Not(filter) => json!({
                "operator": "NOT",
                "conditions": [filter.to_value()],
            }),
        }
    }
}

/// The `Email/query` conditions this crate sends. A `None` field is left out
/// of the condition; a condition with every field `None` matches every email.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterCondition {
    pub in_mailbox: Option<String>,
    pub text: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    /// An RFC 8620 `UTCDate`; matches `receivedAt` at or after it.
    pub after: Option<String>,
    /// An RFC 8620 `UTCDate`; matches `receivedAt` before it.
    pub before: Option<String>,
    pub has_keyword: Option<String>,
    pub not_keyword: Option<String>,
    pub has_attachment: Option<bool>,
}

impl FilterCondition {
    pub fn to_value(&self) -> Value {
        let mut condition = Map::new();
        let strings = [
            ("inMailbox", &self.in_mailbox),
            ("text", &self.text),
            ("from", &self.from),
            ("to", &self.to),
            ("subject", &self.subject),
            ("after", &self.after),
            ("before", &self.before),
            ("hasKeyword", &self.has_keyword),
            ("notKeyword", &self.not_keyword),
        ];
        for (key, value) in strings {
            if let Some(value) = value {
                condition.insert(key.into(), Value::String(value.clone()));
            }
        }
        if let Some(has_attachment) = self.has_attachment {
            condition.insert("hasAttachment".into(), Value::Bool(has_attachment));
        }
        Value::Object(condition)
    }
}

/// One `Email/query` sort comparator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub property: &'static str,
    pub is_ascending: bool,
}

/// `Email/query` (RFC 8621 section 4.4).
#[derive(Debug, Clone)]
pub struct EmailQuery<'a> {
    pub account_id: &'a str,
    pub filter: Filter,
    pub sort: Vec<Sort>,
    pub position: u64,
    pub limit: u64,
    pub calculate_total: bool,
}

impl EmailQuery<'_> {
    pub const METHOD: &'static str = "Email/query";

    pub fn args(&self) -> Value {
        let sort: Vec<Value> = self
            .sort
            .iter()
            .map(|sort| json!({"property": sort.property, "isAscending": sort.is_ascending}))
            .collect();
        json!({
            "accountId": self.account_id,
            "filter": self.filter.to_value(),
            "sort": sort,
            "position": self.position,
            "limit": self.limit,
            "calculateTotal": self.calculate_total,
        })
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// An `Email/query` result: the ids in sort order from `position`, and the
/// total when it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    pub ids: Vec<String>,
    pub position: u64,
    pub total: Option<u64>,
}

impl QueryResult {
    pub fn read(args: &Value) -> Result<QueryResult, Error> {
        let f = Fields::of(args, "Email/query result")?;
        Ok(QueryResult {
            ids: f.required_strings("ids")?,
            position: f.u64("position")?,
            total: f.opt_u64("total")?,
        })
    }
}

/// The ids a get asks for: listed, or taken from an earlier call's result.
#[derive(Debug, Clone, PartialEq)]
pub enum Ids {
    /// Sent as `ids`.
    Listed(Vec<String>),
    /// Sent as `#ids`; a `result_ref` value.
    Ref(Value),
}

/// `Email/get` (RFC 8621 section 4.2).
#[derive(Debug, Clone)]
pub struct EmailGet<'a> {
    pub account_id: &'a str,
    pub ids: Ids,
    pub properties: &'a [&'a str],
    pub fetch_text_body_values: bool,
    pub max_body_value_bytes: Option<u64>,
}

impl EmailGet<'_> {
    pub const METHOD: &'static str = "Email/get";

    pub fn args(&self) -> Value {
        let mut args = Map::new();
        args.insert("accountId".into(), json!(self.account_id));
        match &self.ids {
            Ids::Listed(ids) => args.insert("ids".into(), json!(ids)),
            Ids::Ref(reference) => args.insert("#ids".into(), reference.clone()),
        };
        args.insert("properties".into(), json!(self.properties));
        args.insert(
            "fetchTextBodyValues".into(),
            json!(self.fetch_text_body_values),
        );
        if let Some(max) = self.max_body_value_bytes {
            args.insert("maxBodyValueBytes".into(), json!(max));
        }
        Value::Object(args)
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// A get result: the objects found, and the ids asked for that do not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetResult<T> {
    pub list: Vec<T>,
    pub not_found: Vec<String>,
}

/// An `EmailAddress` (RFC 8621 section 4.1.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    /// `Name <email>`, or the bare address when there is no name.
    pub fn display(&self) -> String {
        match self.name.as_deref() {
            Some(name) if !name.is_empty() => format!("{name} <{}>", self.email),
            _ => self.email.clone(),
        }
    }

    fn read(value: &Value) -> Result<Address, Error> {
        let f = Fields::of(value, "EmailAddress")?;
        Ok(Address {
            name: f.opt_string("name")?,
            email: f.string("email")?,
        })
    }
}

/// An `EmailBodyPart` (RFC 8621 section 4.1.4), with the default body
/// properties this crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyPart {
    pub part_id: Option<String>,
    pub blob_id: Option<String>,
    pub size: u64,
    pub name: Option<String>,
    pub content_type: String,
    pub charset: Option<String>,
    pub disposition: Option<String>,
    pub cid: Option<String>,
}

impl BodyPart {
    fn read(value: &Value) -> Result<BodyPart, Error> {
        let f = Fields::of(value, "EmailBodyPart")?;
        Ok(BodyPart {
            part_id: f.opt_string("partId")?,
            blob_id: f.opt_string("blobId")?,
            size: f.u64("size")?,
            name: f.opt_string("name")?,
            content_type: f.string("type")?,
            charset: f.opt_string("charset")?,
            disposition: f.opt_string("disposition")?,
            cid: f.opt_string("cid")?,
        })
    }
}

/// An `EmailBodyValue` (RFC 8621 section 4.1.4). The two flags default to
/// false when absent, as RFC 8621 gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyValue {
    pub value: String,
    pub is_encoding_problem: bool,
    pub is_truncated: bool,
}

impl BodyValue {
    fn read(value: &Value) -> Result<BodyValue, Error> {
        let f = Fields::of(value, "EmailBodyValue")?;
        Ok(BodyValue {
            value: f.string("value")?,
            is_encoding_problem: f.opt_bool("isEncodingProblem")?.unwrap_or(false),
            is_truncated: f.opt_bool("isTruncated")?.unwrap_or(false),
        })
    }
}

/// An email with the properties `SUMMARY_PROPERTIES` and `READ_PROPERTIES`
/// name. Only `id` is required: a property not asked for, or null, reads as
/// `None` or empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Email {
    pub id: String,
    pub thread_id: Option<String>,
    pub mailbox_ids: BTreeMap<String, bool>,
    pub keywords: BTreeMap<String, bool>,
    pub size: Option<u64>,
    pub received_at: Option<String>,
    pub sent_at: Option<String>,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: Option<String>,
    pub preview: Option<String>,
    pub has_attachment: Option<bool>,
    pub message_id: Vec<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub text_body: Vec<BodyPart>,
    pub attachments: Vec<BodyPart>,
    pub body_values: BTreeMap<String, BodyValue>,
}

impl Email {
    /// The emails in an `Email/get` result.
    pub fn get(args: &Value) -> Result<GetResult<Email>, Error> {
        read_get(args, Email::read)
    }

    fn read(value: &Value) -> Result<Email, Error> {
        let f = Fields::of(value, "Email")?;
        Ok(Email {
            id: f.string("id")?,
            thread_id: f.opt_string("threadId")?,
            mailbox_ids: f.flags("mailboxIds")?,
            keywords: f.flags("keywords")?,
            size: f.opt_u64("size")?,
            received_at: f.opt_string("receivedAt")?,
            sent_at: f.opt_string("sentAt")?,
            from: f.list("from", Address::read)?,
            to: f.list("to", Address::read)?,
            cc: f.list("cc", Address::read)?,
            bcc: f.list("bcc", Address::read)?,
            reply_to: f.list("replyTo", Address::read)?,
            subject: f.opt_string("subject")?,
            preview: f.opt_string("preview")?,
            has_attachment: f.opt_bool("hasAttachment")?,
            message_id: f.strings("messageId")?,
            in_reply_to: f.strings("inReplyTo")?,
            references: f.strings("references")?,
            text_body: f.list("textBody", BodyPart::read)?,
            attachments: f.list("attachments", BodyPart::read)?,
            body_values: f.map("bodyValues", BodyValue::read)?,
        })
    }
}

/// `Thread/get` (RFC 8621 section 3) over listed thread ids, every property.
#[derive(Debug, Clone)]
pub struct ThreadGet<'a> {
    pub account_id: &'a str,
    pub ids: Vec<String>,
}

impl ThreadGet<'_> {
    pub const METHOD: &'static str = "Thread/get";

    pub fn args(&self) -> Value {
        json!({"accountId": self.account_id, "ids": self.ids})
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// A thread: its id and its emails' ids, oldest `receivedAt` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub email_ids: Vec<String>,
}

impl Thread {
    /// The threads in a `Thread/get` result.
    pub fn get(args: &Value) -> Result<GetResult<Thread>, Error> {
        read_get(args, Thread::read)
    }

    fn read(value: &Value) -> Result<Thread, Error> {
        let f = Fields::of(value, "Thread")?;
        Ok(Thread {
            id: f.string("id")?,
            email_ids: f.required_strings("emailIds")?,
        })
    }
}

/// Reads a `/get` result (RFC 8620 section 5.1): `list` through `read`, and
/// `notFound`.
fn read_get<T>(args: &Value, read: fn(&Value) -> Result<T, Error>) -> Result<GetResult<T>, Error> {
    let f = Fields::of(args, "get result")?;
    let list = f
        .get("list")
        .ok_or_else(|| f.shape("list", "is missing"))?
        .as_array()
        .ok_or_else(|| f.shape("list", "is not an array"))?
        .iter()
        .map(read)
        .collect::<Result<Vec<T>, Error>>()?;
    Ok(GetResult {
        list,
        not_found: f.required_strings("notFound")?,
    })
}

/// An object's properties, read with the type RFC 8621 gives each. A
/// property of the wrong type is `Error::Shape` naming the object and the
/// property; "absent" below means absent or null.
struct Fields<'a> {
    object: &'a Map<String, Value>,
    what: &'static str,
}

impl<'a> Fields<'a> {
    fn of(value: &'a Value, what: &'static str) -> Result<Fields<'a>, Error> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::Shape(format!("{what} is not an object")))?;
        Ok(Fields { object, what })
    }

    fn shape(&self, key: &str, problem: &str) -> Error {
        Error::Shape(format!("{}.{key} {problem}", self.what))
    }

    /// The property, `None` when absent.
    fn get(&self, key: &str) -> Option<&'a Value> {
        self.object.get(key).filter(|value| !value.is_null())
    }

    fn string(&self, key: &str) -> Result<String, Error> {
        self.opt_string(key)?
            .ok_or_else(|| self.shape(key, "is missing"))
    }

    fn opt_string(&self, key: &str) -> Result<Option<String>, Error> {
        self.get(key)
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| self.shape(key, "is not a string"))
            })
            .transpose()
    }

    fn u64(&self, key: &str) -> Result<u64, Error> {
        self.opt_u64(key)?
            .ok_or_else(|| self.shape(key, "is missing"))
    }

    fn opt_u64(&self, key: &str) -> Result<Option<u64>, Error> {
        self.get(key)
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| self.shape(key, "is not an unsigned integer"))
            })
            .transpose()
    }

    fn u64_or_zero(&self, key: &str) -> Result<u64, Error> {
        Ok(self.opt_u64(key)?.unwrap_or(0))
    }

    fn opt_bool(&self, key: &str) -> Result<Option<bool>, Error> {
        self.get(key)
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| self.shape(key, "is not a boolean"))
            })
            .transpose()
    }

    /// A list through `read`; empty when absent.
    fn list<T>(&self, key: &str, read: fn(&Value) -> Result<T, Error>) -> Result<Vec<T>, Error> {
        let Some(value) = self.get(key) else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .ok_or_else(|| self.shape(key, "is not an array"))?
            .iter()
            .map(read)
            .collect()
    }

    /// A string list; empty when absent.
    fn strings(&self, key: &str) -> Result<Vec<String>, Error> {
        if self.get(key).is_none() {
            return Ok(Vec::new());
        }
        self.required_strings(key)
    }

    /// A string list that must be present.
    fn required_strings(&self, key: &str) -> Result<Vec<String>, Error> {
        self.get(key)
            .ok_or_else(|| self.shape(key, "is missing"))?
            .as_array()
            .ok_or_else(|| self.shape(key, "is not an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| self.shape(key, "holds a non-string"))
            })
            .collect()
    }

    /// An object of values through `read`, by key; empty when absent.
    fn map<T>(
        &self,
        key: &str,
        read: fn(&Value) -> Result<T, Error>,
    ) -> Result<BTreeMap<String, T>, Error> {
        let Some(value) = self.get(key) else {
            return Ok(BTreeMap::new());
        };
        value
            .as_object()
            .ok_or_else(|| self.shape(key, "is not an object"))?
            .iter()
            .map(|(name, value)| Ok((name.clone(), read(value)?)))
            .collect()
    }

    /// An `Id[Boolean]` or `String[Boolean]` set; empty when absent.
    fn flags(&self, key: &str) -> Result<BTreeMap<String, bool>, Error> {
        let Some(value) = self.get(key) else {
            return Ok(BTreeMap::new());
        };
        value
            .as_object()
            .ok_or_else(|| self.shape(key, "is not an object"))?
            .iter()
            .map(|(name, flag)| {
                flag.as_bool()
                    .map(|flag| (name.clone(), flag))
                    .ok_or_else(|| self.shape(key, "holds a non-boolean"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::result_ref;
    use crate::response::Response;

    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn body(request: &Request) -> Value {
        serde_json::from_str(&request.to_body()).unwrap()
    }

    #[test]
    fn mailbox_get_asks_for_every_mailbox() {
        let mut request = Request::mail();
        let call = MailboxGet {
            account_id: "a1",
            properties: MAILBOX_PROPERTIES,
        }
        .push(&mut request);
        assert_eq!(call.name(), "Mailbox/get");
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["Mailbox/get", {
                "accountId": "a1",
                "ids": null,
                "properties": ["id", "name", "parentId", "role", "sortOrder",
                    "totalEmails", "unreadEmails", "totalThreads", "unreadThreads"]
            }, "c0"]])
        );
    }

    #[test]
    fn mailboxes_are_read_with_absent_counts_as_zero() {
        let mailboxes =
            Mailbox::list(&fixture(include_str!("../fixtures/mailbox_get.json"))).unwrap();
        assert_eq!(
            mailboxes,
            vec![
                Mailbox {
                    id: "m1".into(),
                    name: "Inbox".into(),
                    parent_id: None,
                    role: Some("inbox".into()),
                    sort_order: 1,
                    total_emails: 120,
                    unread_emails: 3,
                    total_threads: 98,
                    unread_threads: 2,
                },
                Mailbox {
                    id: "m2".into(),
                    name: "Receipts".into(),
                    parent_id: Some("m1".into()),
                    role: None,
                    sort_order: 0,
                    total_emails: 0,
                    unread_emails: 0,
                    total_threads: 0,
                    unread_threads: 0,
                },
            ]
        );
    }

    #[test]
    fn a_mailbox_of_the_wrong_shape_is_malformed() {
        for args in [
            json!({"notFound": []}),
            json!({"list": {}, "notFound": []}),
            json!({"list": [{"id": "m1", "name": "Inbox"}]}),
            json!({"list": [{"name": "Inbox"}], "notFound": []}),
            json!({"list": [{"id": "m1", "name": 7}], "notFound": []}),
            json!({"list": [{"id": "m1", "name": "Inbox", "totalEmails": -1}], "notFound": []}),
            json!({"list": [{"id": "m1", "name": "Inbox", "sortOrder": 4294967296u64}], "notFound": []}),
            json!({"list": ["m1"], "notFound": []}),
        ] {
            assert!(
                matches!(Mailbox::list(&args), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }

    #[test]
    fn a_search_is_a_query_and_a_get_by_reference() {
        let mut request = Request::mail();
        let query = EmailQuery {
            account_id: "a1",
            filter: Filter::And(vec![
                Filter::Condition(FilterCondition {
                    from: Some("alice@example.com".into()),
                    after: Some("2026-11-01T00:00:00Z".into()),
                    ..FilterCondition::default()
                }),
                Filter::Condition(FilterCondition {
                    in_mailbox: Some("m1".into()),
                    has_attachment: Some(false),
                    ..FilterCondition::default()
                }),
                Filter::Not(Box::new(Filter::Condition(FilterCondition {
                    has_keyword: Some("$seen".into()),
                    ..FilterCondition::default()
                }))),
            ]),
            sort: vec![Sort {
                property: "receivedAt",
                is_ascending: false,
            }],
            position: 10,
            limit: 5,
            calculate_total: true,
        }
        .push(&mut request);
        EmailGet {
            account_id: "a1",
            ids: Ids::Ref(result_ref(&query, "/ids")),
            properties: &["id", "subject"],
            fetch_text_body_values: false,
            max_body_value_bytes: None,
        }
        .push(&mut request);
        assert_eq!(
            body(&request)["methodCalls"],
            json!([
                ["Email/query", {
                    "accountId": "a1",
                    "filter": {
                        "operator": "AND",
                        "conditions": [
                            {"from": "alice@example.com", "after": "2026-11-01T00:00:00Z"},
                            {"inMailbox": "m1", "hasAttachment": false},
                            {"operator": "NOT", "conditions": [{"hasKeyword": "$seen"}]}
                        ]
                    },
                    "sort": [{"property": "receivedAt", "isAscending": false}],
                    "position": 10,
                    "limit": 5,
                    "calculateTotal": true
                }, "c0"],
                ["Email/get", {
                    "accountId": "a1",
                    "#ids": {"resultOf": "c0", "name": "Email/query", "path": "/ids"},
                    "properties": ["id", "subject"],
                    "fetchTextBodyValues": false
                }, "c1"]
            ])
        );
    }

    #[test]
    fn every_condition_field_has_its_wire_name_and_an_empty_one_matches_all() {
        let condition = FilterCondition {
            in_mailbox: Some("m1".into()),
            text: Some("invoice".into()),
            from: Some("alice@example.com".into()),
            to: Some("bob@example.com".into()),
            subject: Some("Re: lunch".into()),
            after: Some("2026-11-01T00:00:00Z".into()),
            before: Some("2026-11-08T00:00:00Z".into()),
            has_keyword: Some("$flagged".into()),
            not_keyword: Some("$seen".into()),
            has_attachment: Some(true),
        };
        assert_eq!(
            Filter::Condition(condition).to_value(),
            json!({
                "inMailbox": "m1",
                "text": "invoice",
                "from": "alice@example.com",
                "to": "bob@example.com",
                "subject": "Re: lunch",
                "after": "2026-11-01T00:00:00Z",
                "before": "2026-11-08T00:00:00Z",
                "hasKeyword": "$flagged",
                "notKeyword": "$seen",
                "hasAttachment": true
            })
        );
        assert_eq!(
            Filter::Condition(FilterCondition::default()).to_value(),
            json!({})
        );
    }

    #[test]
    fn a_read_lists_its_id_and_bounds_the_body_values() {
        let args = EmailGet {
            account_id: "a1",
            ids: Ids::Listed(vec!["e1".into()]),
            properties: READ_PROPERTIES,
            fetch_text_body_values: true,
            max_body_value_bytes: Some(12000),
        }
        .args();
        assert_eq!(args["ids"], json!(["e1"]));
        assert_eq!(args.get("#ids"), None);
        assert_eq!(args["fetchTextBodyValues"], json!(true));
        assert_eq!(args["maxBodyValueBytes"], json!(12000));
        assert_eq!(args["properties"], json!(READ_PROPERTIES));
    }

    #[test]
    fn a_query_result_is_read_with_and_without_its_total() {
        assert_eq!(
            QueryResult::read(&json!({
                "accountId": "a1", "queryState": "q1", "canCalculateChanges": false,
                "position": 0, "ids": ["e1", "e2"], "total": 41
            }))
            .unwrap(),
            QueryResult {
                ids: vec!["e1".into(), "e2".into()],
                position: 0,
                total: Some(41),
            }
        );
        assert_eq!(
            QueryResult::read(&json!({"position": 5, "ids": []})).unwrap(),
            QueryResult {
                ids: vec![],
                position: 5,
                total: None,
            }
        );
        for args in [
            json!({"position": 0}),
            json!({"ids": []}),
            json!({"position": 0, "ids": [1]}),
            json!({"position": "0", "ids": []}),
        ] {
            assert!(
                matches!(QueryResult::read(&args), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }

    #[test]
    fn a_summary_with_absent_optionals_reads_as_none_and_empty() {
        let result =
            Email::get(&fixture(include_str!("../fixtures/email_get_summary.json"))).unwrap();
        assert_eq!(result.not_found, vec!["e9".to_owned()]);
        let [full, sparse] = <[Email; 2]>::try_from(result.list).unwrap();
        assert_eq!(full.id, "e1");
        assert_eq!(full.thread_id.as_deref(), Some("t1"));
        assert_eq!(full.mailbox_ids, BTreeMap::from([("m1".to_owned(), true)]));
        assert_eq!(
            full.keywords,
            BTreeMap::from([("$flagged".to_owned(), true), ("$seen".to_owned(), true)])
        );
        assert_eq!(full.size, Some(2048));
        assert_eq!(full.received_at.as_deref(), Some("2026-11-07T09:30:00Z"));
        assert_eq!(
            full.from.iter().map(Address::display).collect::<Vec<_>>(),
            vec!["Alice <alice@example.com>"]
        );
        assert_eq!(
            full.to.iter().map(Address::display).collect::<Vec<_>>(),
            vec!["bob@example.com", "carol@example.com"]
        );
        assert_eq!(full.subject.as_deref(), Some("Lunch on Friday"));
        assert_eq!(full.preview.as_deref(), Some("Are we still on for"));
        assert_eq!(full.has_attachment, Some(false));
        assert_eq!(
            sparse,
            Email {
                id: "e2".into(),
                ..Email::default()
            }
        );
    }

    #[test]
    fn a_full_read_carries_headers_parts_and_values() {
        let result = Email::get(&fixture(include_str!("../fixtures/email_get_read.json"))).unwrap();
        assert!(result.not_found.is_empty());
        let [email] = <[Email; 1]>::try_from(result.list).unwrap();
        assert_eq!(email.sent_at.as_deref(), Some("2026-11-07T10:29:58+01:00"));
        assert_eq!(
            email.cc,
            vec![Address {
                name: None,
                email: "carol@example.com".into(),
            }]
        );
        assert!(email.bcc.is_empty());
        assert_eq!(
            email.reply_to,
            vec![Address {
                name: Some("Alice".into()),
                email: "alice@example.com".into(),
            }]
        );
        assert_eq!(email.message_id, vec!["m2@example.com".to_owned()]);
        assert_eq!(email.in_reply_to, vec!["m1@example.com".to_owned()]);
        assert_eq!(
            email.references,
            vec!["m0@example.com".to_owned(), "m1@example.com".to_owned()]
        );
        assert_eq!(
            email.text_body,
            vec![
                BodyPart {
                    part_id: Some("1".into()),
                    blob_id: Some("b1".into()),
                    size: 420,
                    name: None,
                    content_type: "text/plain".into(),
                    charset: Some("utf-8".into()),
                    disposition: None,
                    cid: None,
                },
                BodyPart {
                    part_id: Some("2".into()),
                    blob_id: Some("b2".into()),
                    size: 900,
                    name: None,
                    content_type: "text/html".into(),
                    charset: Some("utf-8".into()),
                    disposition: Some("inline".into()),
                    cid: None,
                },
            ]
        );
        assert_eq!(
            email.attachments,
            vec![BodyPart {
                part_id: Some("3".into()),
                blob_id: Some("b3".into()),
                size: 51200,
                name: Some("menu.pdf".into()),
                content_type: "application/pdf".into(),
                charset: None,
                disposition: Some("attachment".into()),
                cid: Some("menu@example.com".into()),
            }]
        );
        assert_eq!(
            email.body_values,
            BTreeMap::from([
                (
                    "1".to_owned(),
                    BodyValue {
                        value: "Are we still on for Friday?\n".into(),
                        is_encoding_problem: false,
                        is_truncated: false,
                    }
                ),
                (
                    "2".to_owned(),
                    BodyValue {
                        value: "<p>Are we still on".into(),
                        is_encoding_problem: true,
                        is_truncated: true,
                    }
                ),
            ])
        );
    }

    #[test]
    fn an_email_of_the_wrong_shape_is_malformed() {
        for email in [
            json!({}),
            json!({"id": 1}),
            json!({"id": "e1", "mailboxIds": ["m1"]}),
            json!({"id": "e1", "keywords": {"$seen": "yes"}}),
            json!({"id": "e1", "from": [{"name": "Alice"}]}),
            json!({"id": "e1", "from": {"email": "alice@example.com"}}),
            json!({"id": "e1", "messageId": [7]}),
            json!({"id": "e1", "textBody": [{"partId": "1", "size": 3}]}),
            json!({"id": "e1", "textBody": [{"partId": "1", "type": "text/plain"}]}),
            json!({"id": "e1", "bodyValues": {"1": {"isTruncated": false}}}),
            json!({"id": "e1", "bodyValues": {"1": {"value": "x", "isTruncated": 0}}}),
            json!({"id": "e1", "hasAttachment": "no"}),
        ] {
            let args = json!({"list": [email], "notFound": []});
            assert!(matches!(Email::get(&args), Err(Error::Shape(_))), "{email}");
        }
        assert!(matches!(
            Email::get(&json!({"list": [], "notFound": [1]})),
            Err(Error::Shape(_))
        ));
    }

    #[test]
    fn a_thread_lists_its_emails_and_names_the_ids_not_found() {
        let mut request = Request::mail();
        ThreadGet {
            account_id: "a1",
            ids: vec!["t1".into(), "t9".into()],
        }
        .push(&mut request);
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["Thread/get", {"accountId": "a1", "ids": ["t1", "t9"]}, "c0"]])
        );
        assert_eq!(
            Thread::get(&json!({
                "accountId": "a1", "state": "s1",
                "list": [{"id": "t1", "emailIds": ["e1", "e2", "e3"]}],
                "notFound": ["t9"]
            }))
            .unwrap(),
            GetResult {
                list: vec![Thread {
                    id: "t1".into(),
                    email_ids: vec!["e1".into(), "e2".into(), "e3".into()],
                }],
                not_found: vec!["t9".into()],
            }
        );
        for thread in [json!({"id": "t1"}), json!({"id": "t1", "emailIds": "e1"})] {
            let args = json!({"list": [thread], "notFound": []});
            assert!(
                matches!(Thread::get(&args), Err(Error::Shape(_))),
                "{thread}"
            );
        }
    }

    #[test]
    fn a_reader_takes_the_arguments_a_response_returns_for_its_call() {
        let mut request = Request::mail();
        let call = MailboxGet {
            account_id: "a1",
            properties: MAILBOX_PROPERTIES,
        }
        .push(&mut request);
        let response = Response::parse(json!({
            "methodResponses": [["Mailbox/get", {
                "accountId": "a1", "state": "s1",
                "list": [{"id": "m1", "name": "Inbox"}],
                "notFound": []
            }, "c0"]]
        }))
        .unwrap();
        let mailboxes = Mailbox::list(response.result(&call).unwrap()).unwrap();
        assert_eq!(mailboxes[0].name, "Inbox");
    }

    #[test]
    fn an_address_without_a_name_displays_bare() {
        let named = Address {
            name: Some("Alice Example".into()),
            email: "alice@example.com".into(),
        };
        let empty = Address {
            name: Some(String::new()),
            email: "alice@example.com".into(),
        };
        let none = Address {
            name: None,
            email: "alice@example.com".into(),
        };
        assert_eq!(named.display(), "Alice Example <alice@example.com>");
        assert_eq!(empty.display(), "alice@example.com");
        assert_eq!(none.display(), "alice@example.com");
    }
}
