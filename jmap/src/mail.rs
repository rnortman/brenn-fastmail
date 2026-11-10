//! The mail methods this crate builds and reads (RFC 8621): `Mailbox/get`,
//! `Mailbox/query`, `Mailbox/set`, `Email/query`, `Email/get`, `Email/set`,
//! `Thread/get` and `Identity/get`. Each builder pushes one call with its own
//! method name; each reader takes the arguments `Response::result` returned
//! for that call and refuses any value whose type is not the one RFC 8621
//! gives the property.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::method::{Fields, GetResult, Ids, Sort, read_get};
use crate::request::{CallId, Request};
use crate::set::{Patch, SetResult, set_args};

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

/// What a draft reads of the email it replies to: where to address the
/// reply, its subject, and its threading headers.
pub const DRAFT_LOOKUP_PROPERTIES: &[&str] = &[
    "id",
    "threadId",
    "from",
    "replyTo",
    "subject",
    "messageId",
    "references",
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
        self.ids.insert_into(&mut args);
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

    /// The address as an `EmailAddress` object, `name` null when absent.
    pub fn to_value(&self) -> Value {
        json!({"name": self.name, "email": self.email})
    }

    /// An `EmailAddress` object as `to_value` writes it and JMAP answers it:
    /// `email` a string, `name` a string, null or absent.
    pub fn from_value(value: &Value) -> Result<Address, Error> {
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
            from: f.list("from", Address::from_value)?,
            to: f.list("to", Address::from_value)?,
            cc: f.list("cc", Address::from_value)?,
            bcc: f.list("bcc", Address::from_value)?,
            reply_to: f.list("replyTo", Address::from_value)?,
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

/// A new email in the creation form RFC 8621 section 4.6 gives it: one
/// `text/plain` body part whose value is `text`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailCreate {
    pub mailbox_ids: Vec<String>,
    pub keywords: Vec<String>,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub text: String,
}

impl EmailCreate {
    /// The object a `create` entry carries. An empty address or id list and
    /// an absent subject are left out.
    pub fn to_value(&self) -> Value {
        let mut email = Map::new();
        email.insert("mailboxIds".into(), true_set(&self.mailbox_ids));
        email.insert("keywords".into(), true_set(&self.keywords));
        let addresses = [
            ("from", &self.from),
            ("to", &self.to),
            ("cc", &self.cc),
            ("bcc", &self.bcc),
            ("replyTo", &self.reply_to),
        ];
        for (key, list) in addresses {
            if !list.is_empty() {
                let list: Vec<Value> = list.iter().map(Address::to_value).collect();
                email.insert(key.into(), Value::Array(list));
            }
        }
        if let Some(subject) = &self.subject {
            email.insert("subject".into(), json!(subject));
        }
        for (key, ids) in [
            ("inReplyTo", &self.in_reply_to),
            ("references", &self.references),
        ] {
            if !ids.is_empty() {
                email.insert(key.into(), json!(ids));
            }
        }
        email.insert(
            "textBody".into(),
            json!([{"partId": "1", "type": "text/plain"}]),
        );
        email.insert("bodyValues".into(), json!({"1": {"value": self.text}}));
        Value::Object(email)
    }
}

/// `Email/set` (RFC 8621 section 4.6): creates and updates. Nothing here
/// destroys.
#[derive(Debug, Clone)]
pub struct EmailSet<'a> {
    pub account_id: &'a str,
    /// Creation id → the new email.
    pub create: Vec<(String, EmailCreate)>,
    /// Email id → its patch.
    pub update: Vec<(String, Patch)>,
}

impl EmailSet<'_> {
    pub const METHOD: &'static str = "Email/set";

    /// `create` and `update` each appear only when non-empty.
    pub fn args(&self) -> Value {
        set_args(
            self.account_id,
            self.create
                .iter()
                .map(|(cid, email)| (cid.clone(), email.to_value())),
            &self.update,
            None,
        )
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }

    /// Reads the result of an `Email/set`.
    pub fn result(args: &Value) -> Result<SetResult<EmailCreated>, Error> {
        SetResult::read(args, EmailCreated::read)
    }
}

/// What the server set on a created email that this crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailCreated {
    pub id: String,
    pub thread_id: Option<String>,
}

impl EmailCreated {
    fn read(value: &Value) -> Result<EmailCreated, Error> {
        let f = Fields::of(value, "created Email")?;
        Ok(EmailCreated {
            id: f.string("id")?,
            thread_id: f.opt_string("threadId")?,
        })
    }
}

/// `Mailbox/query` (RFC 8621 section 2.3) for the mailbox with a role.
#[derive(Debug, Clone)]
pub struct MailboxQuery<'a> {
    pub account_id: &'a str,
    pub role: &'a str,
}

impl MailboxQuery<'_> {
    pub const METHOD: &'static str = "Mailbox/query";

    pub fn args(&self) -> Value {
        json!({"accountId": self.account_id, "filter": {"role": self.role}})
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// A new mailbox; `parentId` is left out (top level) when `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxCreate {
    pub name: String,
    pub parent_id: Option<String>,
}

impl MailboxCreate {
    pub fn to_value(&self) -> Value {
        let mut mailbox = Map::new();
        mailbox.insert("name".into(), json!(self.name));
        if let Some(parent_id) = &self.parent_id {
            mailbox.insert("parentId".into(), json!(parent_id));
        }
        Value::Object(mailbox)
    }
}

/// `Mailbox/set` (RFC 8621 section 2.5): creates and updates. Nothing here
/// destroys.
#[derive(Debug, Clone)]
pub struct MailboxSet<'a> {
    pub account_id: &'a str,
    /// Creation id → the new mailbox.
    pub create: Vec<(String, MailboxCreate)>,
    /// Mailbox id → its patch.
    pub update: Vec<(String, Patch)>,
}

impl MailboxSet<'_> {
    pub const METHOD: &'static str = "Mailbox/set";

    /// `create` and `update` each appear only when non-empty.
    pub fn args(&self) -> Value {
        set_args(
            self.account_id,
            self.create
                .iter()
                .map(|(cid, mailbox)| (cid.clone(), mailbox.to_value())),
            &self.update,
            None,
        )
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }

    /// Reads the result of a `Mailbox/set`.
    pub fn result(args: &Value) -> Result<SetResult<MailboxCreated>, Error> {
        SetResult::read(args, MailboxCreated::read)
    }
}

/// What the server set on a created mailbox that this crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxCreated {
    pub id: String,
}

impl MailboxCreated {
    fn read(value: &Value) -> Result<MailboxCreated, Error> {
        Ok(MailboxCreated {
            id: Fields::of(value, "created Mailbox")?.string("id")?,
        })
    }
}

/// `Identity/get` (RFC 8621 section 6.1) over every identity in the account.
#[derive(Debug, Clone)]
pub struct IdentityGet<'a> {
    pub account_id: &'a str,
}

impl IdentityGet<'_> {
    pub const METHOD: &'static str = "Identity/get";

    pub fn args(&self) -> Value {
        json!({"accountId": self.account_id, "ids": null})
    }

    pub fn push(&self, request: &mut Request) -> CallId {
        request.push(Self::METHOD, self.args())
    }
}

/// A sending identity (RFC 8621 section 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub name: Option<String>,
    pub email: String,
    pub may_delete: Option<bool>,
}

impl Identity {
    /// The identities in an `Identity/get` result.
    pub fn get(args: &Value) -> Result<GetResult<Identity>, Error> {
        read_get(args, Identity::read)
    }

    fn read(value: &Value) -> Result<Identity, Error> {
        let f = Fields::of(value, "Identity")?;
        Ok(Identity {
            id: f.string("id")?,
            name: f.opt_string("name")?,
            email: f.string("email")?,
            may_delete: f.opt_bool("mayDelete")?,
        })
    }
}

/// An `Id[Boolean]` set with every member `true`.
fn true_set(ids: &[String]) -> Value {
    Value::Object(
        ids.iter()
            .map(|id| (id.clone(), Value::Bool(true)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::QueryResult;
    use crate::request::result_ref;
    use crate::response::Response;
    use crate::set::SetError;

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

    fn alice() -> Address {
        Address {
            name: Some("Alice Example".into()),
            email: "alice@example.com".into(),
        }
    }

    #[test]
    fn an_address_object_round_trips_with_a_null_name_when_absent() {
        assert_eq!(
            alice().to_value(),
            json!({"name": "Alice Example", "email": "alice@example.com"})
        );
        let bare = Address {
            name: None,
            email: "bob@example.com".into(),
        };
        assert_eq!(
            bare.to_value(),
            json!({"name": null, "email": "bob@example.com"})
        );
        for address in [alice(), bare] {
            assert_eq!(Address::from_value(&address.to_value()), Ok(address));
        }
        assert_eq!(
            Address::from_value(&json!({"email": "bob@example.com"})).map(|a| a.name),
            Ok(None)
        );
        for malformed in [
            json!("bob@example.com"),
            json!({"name": "Bob"}),
            json!({"name": 1, "email": "bob@example.com"}),
            json!({"email": null}),
        ] {
            assert!(
                matches!(Address::from_value(&malformed), Err(Error::Shape(_))),
                "{malformed}"
            );
        }
    }

    #[test]
    fn a_new_email_is_one_text_part_with_its_headers() {
        let email = EmailCreate {
            mailbox_ids: vec!["m5".into()],
            keywords: vec!["$draft".into(), "$seen".into()],
            from: vec![alice()],
            to: vec![Address {
                name: None,
                email: "bob@example.com".into(),
            }],
            cc: vec![Address {
                name: None,
                email: "carol@example.com".into(),
            }],
            bcc: vec![Address {
                name: None,
                email: "dave@example.com".into(),
            }],
            reply_to: vec![alice()],
            subject: Some("Re: lunch".into()),
            in_reply_to: vec!["m2@example.com".into()],
            references: vec!["m1@example.com".into(), "m2@example.com".into()],
            text: "Friday works.\n".into(),
        };
        assert_eq!(
            email.to_value(),
            json!({
                "mailboxIds": {"m5": true},
                "keywords": {"$draft": true, "$seen": true},
                "from": [{"name": "Alice Example", "email": "alice@example.com"}],
                "to": [{"name": null, "email": "bob@example.com"}],
                "cc": [{"name": null, "email": "carol@example.com"}],
                "bcc": [{"name": null, "email": "dave@example.com"}],
                "replyTo": [{"name": "Alice Example", "email": "alice@example.com"}],
                "subject": "Re: lunch",
                "inReplyTo": ["m2@example.com"],
                "references": ["m1@example.com", "m2@example.com"],
                "textBody": [{"partId": "1", "type": "text/plain"}],
                "bodyValues": {"1": {"value": "Friday works.\n"}}
            })
        );
    }

    #[test]
    fn a_new_email_leaves_out_empty_headers() {
        let email = EmailCreate {
            mailbox_ids: vec!["m5".into()],
            text: "hi".into(),
            ..EmailCreate::default()
        };
        assert_eq!(
            email.to_value(),
            json!({
                "mailboxIds": {"m5": true},
                "keywords": {},
                "textBody": [{"partId": "1", "type": "text/plain"}],
                "bodyValues": {"1": {"value": "hi"}}
            })
        );
    }

    #[test]
    fn an_email_set_sends_only_its_non_empty_parts() {
        let mut request = Request::mail();
        let call = EmailSet {
            account_id: "a1",
            create: vec![],
            update: vec![
                ("e1".into(), Patch::new().keyword("$seen", true)),
                ("e2".into(), Patch::new().mailbox("m1", false)),
            ],
        }
        .push(&mut request);
        assert_eq!(call.name(), "Email/set");
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["Email/set", {
                "accountId": "a1",
                "update": {
                    "e1": {"keywords/$seen": true},
                    "e2": {"mailboxIds/m1": null}
                }
            }, "c0"]])
        );
        let create = EmailSet {
            account_id: "a1",
            create: vec![(
                "draft".into(),
                EmailCreate {
                    mailbox_ids: vec!["m5".into()],
                    text: "hi".into(),
                    ..EmailCreate::default()
                },
            )],
            update: vec![],
        }
        .args();
        assert_eq!(create.get("update"), None);
        assert_eq!(create["create"]["draft"]["mailboxIds"], json!({"m5": true}));
        let empty = EmailSet {
            account_id: "a1",
            create: vec![],
            update: vec![],
        };
        assert_eq!(empty.args(), json!({"accountId": "a1"}));
    }

    #[test]
    fn an_email_set_result_reads_the_created_ids_and_each_refusal() {
        let result =
            EmailSet::result(&fixture(include_str!("../fixtures/email_set.json"))).unwrap();
        assert_eq!(
            result.created,
            BTreeMap::from([(
                "draft".to_owned(),
                EmailCreated {
                    id: "e9".into(),
                    thread_id: Some("t4".into()),
                }
            )])
        );
        assert_eq!(result.updated, vec!["e1".to_owned(), "e2".to_owned()]);
        assert_eq!(result.not_created["other"].kind, "overQuota");
        assert_eq!(
            result.not_updated["e3"],
            SetError {
                kind: "invalidProperties".into(),
                description: Some("an email must be in at least one mailbox".into()),
                properties: vec!["mailboxIds".into()],
            }
        );
        assert_eq!(
            result.not_updated["e4"],
            SetError {
                kind: "notFound".into(),
                description: None,
                properties: vec![],
            }
        );
        for created in [
            json!({}),
            json!({"id": 7}),
            json!({"id": "e9", "threadId": 4}),
        ] {
            let args = json!({"created": {"draft": created}});
            assert!(
                matches!(EmailSet::result(&args), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }

    #[test]
    fn a_mailbox_query_filters_by_role_and_reads_as_a_query() {
        let mut request = Request::mail();
        let call = MailboxQuery {
            account_id: "a1",
            role: "drafts",
        }
        .push(&mut request);
        assert_eq!(call.name(), "Mailbox/query");
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["Mailbox/query", {"accountId": "a1", "filter": {"role": "drafts"}}, "c0"]])
        );
        assert_eq!(
            QueryResult::read(&fixture(include_str!("../fixtures/mailbox_query.json"))).unwrap(),
            QueryResult {
                ids: vec!["m5".into()],
                position: 0,
                total: None,
            }
        );
    }

    #[test]
    fn a_mailbox_set_creates_and_renames() {
        let mut request = Request::mail();
        let call = MailboxSet {
            account_id: "a1",
            create: vec![
                (
                    "mailbox".into(),
                    MailboxCreate {
                        name: "Receipts".into(),
                        parent_id: Some("m1".into()),
                    },
                ),
                (
                    "top".into(),
                    MailboxCreate {
                        name: "Projects".into(),
                        parent_id: None,
                    },
                ),
            ],
            update: vec![(
                "m2".into(),
                Patch::new()
                    .property("name", json!("Old receipts"))
                    .property("parentId", Value::Null),
            )],
        }
        .push(&mut request);
        assert_eq!(call.name(), "Mailbox/set");
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["Mailbox/set", {
                "accountId": "a1",
                "create": {
                    "mailbox": {"name": "Receipts", "parentId": "m1"},
                    "top": {"name": "Projects"}
                },
                "update": {"m2": {"name": "Old receipts", "parentId": null}}
            }, "c0"]])
        );
        let result =
            MailboxSet::result(&fixture(include_str!("../fixtures/mailbox_set.json"))).unwrap();
        assert_eq!(
            result.created,
            BTreeMap::from([("mailbox".to_owned(), MailboxCreated { id: "m7".into() })])
        );
        assert_eq!(result.updated, vec!["m2".to_owned()]);
        assert_eq!(result.not_updated["m3"].properties, vec!["name".to_owned()]);
        assert!(matches!(
            MailboxSet::result(&json!({"created": {"mailbox": {"name": "x"}}})),
            Err(Error::Shape(_))
        ));
    }

    #[test]
    fn identities_are_read_from_every_identity_in_the_account() {
        let mut request = Request::mail_with_submission();
        let call = IdentityGet { account_id: "a1" }.push(&mut request);
        assert_eq!(call.name(), "Identity/get");
        assert_eq!(
            body(&request)["methodCalls"],
            json!([["Identity/get", {"accountId": "a1", "ids": null}, "c0"]])
        );
        let result =
            Identity::get(&fixture(include_str!("../fixtures/identity_get.json"))).unwrap();
        assert!(result.not_found.is_empty());
        assert_eq!(
            result.list,
            vec![
                Identity {
                    id: "i1".into(),
                    name: Some("Alice Example".into()),
                    email: "alice@example.com".into(),
                    may_delete: Some(false),
                },
                Identity {
                    id: "i2".into(),
                    name: Some(String::new()),
                    email: "*@example.org".into(),
                    may_delete: Some(true),
                },
                Identity {
                    id: "i3".into(),
                    name: None,
                    email: "alias@example.net".into(),
                    may_delete: None,
                },
            ]
        );
        for identity in [
            json!({"id": "i1"}),
            json!({"email": "alice@example.com"}),
            json!({"id": "i1", "email": "alice@example.com", "mayDelete": "no"}),
            json!({"id": "i1", "email": "alice@example.com", "name": 7}),
        ] {
            let args = json!({"list": [identity], "notFound": []});
            assert!(
                matches!(Identity::get(&args), Err(Error::Shape(_))),
                "{identity}"
            );
        }
    }

    #[test]
    fn a_draft_lookup_reads_the_originals_threading_and_addresses() {
        let args = EmailGet {
            account_id: "a1",
            ids: Ids::Listed(vec!["e1".into()]),
            properties: DRAFT_LOOKUP_PROPERTIES,
            fetch_text_body_values: false,
            max_body_value_bytes: None,
        }
        .args();
        assert_eq!(
            args["properties"],
            json!([
                "id",
                "threadId",
                "from",
                "replyTo",
                "subject",
                "messageId",
                "references"
            ])
        );
        let result = Email::get(&json!({
            "list": [{
                "id": "e1", "threadId": "t1",
                "from": [{"name": "Bob", "email": "bob@example.com"}],
                "replyTo": [{"name": null, "email": "list@example.com"}],
                "subject": "lunch",
                "messageId": ["m2@example.com"],
                "references": ["m1@example.com"]
            }],
            "notFound": []
        }))
        .unwrap();
        let [original] = <[Email; 1]>::try_from(result.list).unwrap();
        assert_eq!(original.reply_to[0].email, "list@example.com");
        assert_eq!(original.message_id, vec!["m2@example.com".to_owned()]);
        assert_eq!(original.references, vec!["m1@example.com".to_owned()]);
    }
}
