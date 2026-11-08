//! JMAP (RFC 8620) request building and response reading, with no I/O: no
//! URL beyond the two fixed paths, no HTTP, no credential. A caller sends
//! `Request::to_body` however it sends things and hands the answer back to
//! `Response::parse`.
//!
//! Its serde is brenn's guest instance, so the `Value`s here are the ones a
//! component built on brenn's provider kit is handed.

pub mod error;
pub mod mail;
pub mod request;
pub mod response;
pub mod session;

pub use error::{Error, RequestError};
pub use mail::{
    Address, BodyPart, BodyValue, Email, EmailGet, EmailQuery, Filter, FilterCondition, GetResult,
    Ids, Mailbox, MailboxGet, QueryResult, Sort, Thread, ThreadGet,
};
pub use request::{CallId, Request, result_ref};
pub use response::{Response, request_error};
pub use session::Session;
