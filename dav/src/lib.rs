//! WebDAV (RFC 4918) and CalDAV (RFC 4791) request bodies and answer
//! reading, and the iCalendar (RFC 5545) the answers carry, with no I/O: no
//! HTTP, no credential. A caller sends the bodies
//! however it sends things and hands the answers back to the readers here.
//!
//! The XML reader this crate parses with is private to it: no type of it
//! appears below.

pub mod caldav;
pub mod error;
pub mod href;
pub mod ical;
mod multistatus;

pub use error::Error;
pub use multistatus::error_condition;
