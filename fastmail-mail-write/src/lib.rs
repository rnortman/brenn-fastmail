//! Fastmail mail organizing over JMAP (brenn:processor world).
//!
//! Each activation reads the instance's `config` and hands the activation to
//! `brenn-provider-kit`'s loop, which runs it in one store transaction: it
//! sweeps rows no result can still arrive for, answers every new result,
//! issues queued requests, serves every new request, and commits. At most
//! `CALL_BUDGET` nested calls go out per activation; requests past it wait in
//! the store's backlog for a later activation.
//!
//! Serving a request validates its arguments and asks for the JMAP session;
//! each answer either completes the request or plans its next step, so a
//! request has at most one nested call outstanding. A request's own problem —
//! bad arguments, Brenn's refusal, Fastmail's error — is answered with
//! `complete_err`; only a structural fault (a bad `config`, a store failure, a
//! malformed envelope) fails the activation.

mod logic;
mod spec;

use brenn_guest::{Activation, Error, Processor};
use fastmail_provider::settings::Settings;

use crate::spec::config;

struct FastmailMailWrite;

impl Processor for FastmailMailWrite {
    fn receive(activation: Activation) -> Result<Option<String>, Error> {
        let settings = Settings::from_lookup(config::get).map_err(Error::failed)?;
        brenn_provider_kit_guest::serve::<logic::FastmailWrite>(&activation, &settings)?;
        Ok(None)
    }
}

brenn_guest::export_processor!(FastmailMailWrite);
