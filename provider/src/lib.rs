//! What every Fastmail provider component shares, in `std`, `serde_json`, the
//! JMAP crate and `brenn-provider-kit`: the instance's `config`, argument
//! readers, the session step and the nested-call builders, and the mapping of
//! Fastmail's answers onto the request's failure. Nothing here names the guest
//! SDK, so all of it runs on the host under test.
//!
//! - [`settings`]: the instance's `config`.
//! - [`args`]: the `account`, id and keyword arguments, id lists, and the
//!   width a string has on the wire.
//! - [`steps`]: the session step and the JMAP API call.
//! - [`failure`]: Fastmail's answer as the request's body or failure.
//! - `testutil` (behind the `testutils` feature): fixtures and argument
//!   probes for a component's host tests.

pub mod args;
pub mod failure;
pub mod settings;
pub mod steps;
#[cfg(feature = "testutils")]
pub mod testutil;
