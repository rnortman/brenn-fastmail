//! JMAP (RFC 8620) request building and response reading, with no I/O: no
//! URL beyond the two fixed paths, no HTTP, no credential. A caller sends
//! `Request::to_body` however it sends things and hands the answer back to
//! `Response::parse`.
//!
//! Its serde is brenn's guest instance, so the `Value`s here are the ones a
//! component built on brenn's provider kit is handed.

pub mod contacts;
pub mod error;
pub mod mail;
pub mod method;
pub mod request;
pub mod response;
pub mod session;
pub mod set;

pub use contacts::{
    AddressBook, AddressBookGet, Anniversary, AnniversaryDate, CardEmail, CardFilter, CardName,
    Component, ContactCard, ContactCardGet, ContactCardQuery, Note, OnlineService, Organization,
    Phone, PostalAddress, Title,
};
pub use error::{Error, RequestError};
pub use mail::{
    Address, BodyPart, BodyValue, Email, EmailCreate, EmailCreated, EmailGet, EmailQuery, EmailSet,
    Filter, FilterCondition, Identity, IdentityGet, Mailbox, MailboxCreate, MailboxCreated,
    MailboxGet, MailboxQuery, MailboxSet, Thread, ThreadGet,
};
pub use method::{GetResult, Ids, QueryResult, Sort};
pub use request::{CallId, Request, result_ref};
pub use response::{Response, request_error};
pub use session::Session;
pub use set::{Patch, SetError, SetResult};
