//! Fastmail calendars written over CalDAV (brenn:processor world).
//!
//! Each activation reads the instance's `config` and the activation's clock
//! and hands the activation to `brenn-provider-kit`'s loop, which runs it in
//! one store transaction: it sweeps rows no result can still arrive for,
//! answers every new result, issues queued requests, serves every new
//! request, and commits. At most `CALL_BUDGET` nested calls go out per
//! activation; requests past it wait in the store's backlog for a later
//! activation.
//!
//! Serving a request validates its arguments and issues its first CalDAV
//! call; each answer either completes the request or plans its next step, so
//! a request has at most one nested call outstanding. A request's own
//! problem — bad arguments, Brenn's refusal, Fastmail's error — is answered
//! with `complete_err`; only a structural fault (a bad `config`, an
//! activation without a clock, a store failure, a malformed envelope) fails
//! the activation.

mod logic;
mod resource;
mod spec;

use brenn_guest::{Activation, Error, Processor};
use fastmail_provider::settings::Settings;

use crate::logic::Context;
use crate::spec::config;

struct FastmailCalendarWrite;

impl Processor for FastmailCalendarWrite {
    fn receive(activation: Activation) -> Result<Option<String>, Error> {
        let settings = Settings::from_lookup(config::get).map_err(Error::failed)?;
        let now_ms = activation.now().ok_or_else(|| {
            Error::failed(
                "the activation carries no clock, which a new event's name and every \
                 written event's stamp are made from",
            )
        })?;
        let context = Context { settings, now_ms };
        brenn_provider_kit_guest::serve::<logic::Fastmail>(&activation, &context)?;
        Ok(None)
    }
}

brenn_guest::export_processor!(FastmailCalendarWrite);
