//! The instance's `config`.

use brenn_provider_kit::{host, required};

/// The API's display name in every refusal the model sees.
pub const API_NAME: &str = "Fastmail";

/// The instance's `config`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The `api_key_provider` every call names.
    pub provider: String,
    /// The `host[:port]` every call goes to.
    pub host: String,
}

impl Settings {
    /// Read the map through `get`. A missing or empty key, or a `host`
    /// containing `/`, is an error naming the key.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Settings, String> {
        Ok(Settings {
            provider: required(&get, "provider")?,
            host: host(&get, "host")?,
        })
    }
}
