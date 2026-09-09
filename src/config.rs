//! Non-secret settings (`~/.config/ghome/config.json`). Credentials never
//! live here — they are keychain-only (`piekstra.ghome`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// Google account email the Home belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Home (structure) id or name to act on by default when the account has
    /// more than one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// The Android device id the credential was minted for. Generated on
    /// first login; the token exchanges must both use the same value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub android_id: Option<String>,
}

pub const KEYS: &[&str] = &["username", "home"];

impl Config {
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "username" => self.username = Some(value.to_string()),
            "home" => self.home = Some(value.to_string()),
            other => return Err(unknown(other)),
        }
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> Result<(), String> {
        match key {
            "username" => self.username = None,
            "home" => self.home = None,
            other => return Err(unknown(other)),
        }
        Ok(())
    }
}

fn unknown(key: &str) -> String {
    format!("unknown config key `{key}` (known: {})", KEYS.join(", "))
}
