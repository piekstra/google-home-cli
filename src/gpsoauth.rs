//! The Android account-token protocol (`android.clients.google.com/auth`),
//! the only credential path Foyer accepts from a non-browser client.
//!
//! Two exchanges, both `application/x-www-form-urlencoded` POSTs answered
//! with newline-delimited `Key=Value` text:
//!
//! 1. **oauth_token → master token.** The browser-issued `oauth_token`
//!    (`oauth2_4/…`, from `accounts.google.com/EmbeddedSetup`) becomes a
//!    long-lived master token (`aas_et/…`). Password-based master login is
//!    not offered: Google has rejected it for most accounts since 2025
//!    (`BadAuthentication`, `NeedsBrowser`, `MissingDroidguard`).
//! 2. **master token → bearer.** A short-lived `ya29…` token scoped to the
//!    Home app's Google project. `EncryptedPasswd` carries the master token
//!    verbatim despite its name.
//!
//! `android_id` must be the same value in both exchanges.

use std::collections::BTreeMap;

use pk_cli_core::CliError;
use reqwest::blocking::Client;

pub const AUTH_URL: &str = "https://android.clients.google.com/auth";
const PLAY_SERVICES_VERSION: &str = "240913000";
/// GMS core signing cert, used for the ac2dm master-token exchange.
const GMS_CLIENT_SIG: &str = "38918a453d07199354f8b19af05ec6562ced5788";
/// The Google Home Android app: bearers minted for it are what Foyer accepts.
pub const HOME_APP: &str = "com.google.android.apps.chromecast.app";
pub const HOME_APP_SIG: &str = "24bb24c05e47e0aefa68a58a766179d9b613a600";
pub const HOMEGRAPH_SCOPE: &str = "oauth2:https://www.googleapis.com/auth/homegraph";

pub const OAUTH_TOKEN_PREFIX: &str = "oauth2_4/";
pub const MASTER_TOKEN_PREFIX: &str = "aas_et/";

/// A minted bearer and, when Google reports one, its expiry (Unix seconds).
#[derive(Debug, Clone)]
pub struct AuthToken {
    pub auth: String,
    pub expiry: Option<u64>,
}

pub fn exchange_form(oauth_token: &str, android_id: &str, email: &str) -> Vec<(String, String)> {
    kv(&[
        ("accountType", "HOSTED_OR_GOOGLE"),
        ("Email", email),
        ("has_permission", "1"),
        ("add_account", "1"),
        ("ACCESS_TOKEN", "1"),
        ("Token", oauth_token),
        ("service", "ac2dm"),
        ("source", "android"),
        ("androidId", android_id),
        ("device_country", "us"),
        ("operatorCountry", "us"),
        ("lang", "en"),
        ("sdk_version", "17"),
        ("google_play_services_version", PLAY_SERVICES_VERSION),
        ("client_sig", GMS_CLIENT_SIG),
        ("callerSig", GMS_CLIENT_SIG),
        ("droidguard_results", "dummy123"),
    ])
}

pub fn auth_token_form(
    master_token: &str,
    android_id: &str,
    service: &str,
    email: &str,
) -> Vec<(String, String)> {
    kv(&[
        ("accountType", "HOSTED_OR_GOOGLE"),
        ("Email", email),
        ("has_permission", "1"),
        ("EncryptedPasswd", master_token),
        ("service", service),
        ("source", "android"),
        ("androidId", android_id),
        ("app", HOME_APP),
        ("client_sig", HOME_APP_SIG),
        ("device_country", "us"),
        ("operatorCountry", "us"),
        ("lang", "en"),
        ("sdk_version", "17"),
        ("google_play_services_version", PLAY_SERVICES_VERSION),
    ])
}

fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Parse the `Key=Value` response body. Values may themselves contain `=`.
pub fn parse_response(body: &str) -> BTreeMap<String, String> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (k, v) = line.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Map a failed exchange onto the exit-code contract. `BadAuthentication`
/// and friends are the credential's fault (exit 3); anything else is Google's.
fn failure(what: &str, fields: &BTreeMap<String, String>) -> CliError {
    let error = fields.get("Error").map(String::as_str).unwrap_or("unknown");
    let detail = fields
        .get("ErrorDetail")
        .map(|d| format!(" ({d})"))
        .unwrap_or_default();
    let msg = format!("{what} failed: {error}{detail}");
    match error {
        "BadAuthentication"
        | "NeedsBrowser"
        | "MissingDroidguard"
        | "DeviceManagementRequiredOrSyncDisabled" => CliError::Auth(msg),
        _ => CliError::Upstream(msg),
    }
}

fn post(client: &Client, form: &[(String, String)]) -> Result<BTreeMap<String, String>, CliError> {
    let resp = client
        .post(AUTH_URL)
        .header(reqwest::header::USER_AGENT, "GoogleAuth/1.4")
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .form(form)
        .send()
        .map_err(|e| CliError::Upstream(format!("android auth endpoint: {e}")))?;
    // The endpoint answers 403 for a rejected token but still carries the
    // `Error=` line, so read the body regardless of status.
    let body = resp
        .text()
        .map_err(|e| CliError::Upstream(format!("reading android auth response: {e}")))?;
    Ok(parse_response(&body))
}

/// oauth_token → master token.
pub fn exchange_oauth_token(
    client: &Client,
    oauth_token: &str,
    android_id: &str,
    email: &str,
) -> Result<String, CliError> {
    let fields = post(client, &exchange_form(oauth_token, android_id, email))?;
    match fields.get("Token") {
        Some(t) if t.starts_with(MASTER_TOKEN_PREFIX) => Ok(t.clone()),
        Some(_) => Err(CliError::Upstream(
            "oauth_token exchange returned a token of an unexpected shape".into(),
        )),
        None => Err(failure("oauth_token exchange", &fields)),
    }
}

/// master token → bearer for `service` (an `oauth2:<scope>` string).
pub fn get_auth_token(
    client: &Client,
    master_token: &str,
    android_id: &str,
    service: &str,
    email: &str,
) -> Result<AuthToken, CliError> {
    let fields = post(
        client,
        &auth_token_form(master_token, android_id, service, email),
    )?;
    match fields.get("Auth") {
        Some(auth) => Ok(AuthToken {
            auth: auth.clone(),
            expiry: fields.get("Expiry").and_then(|e| e.parse().ok()),
        }),
        None => Err(failure("bearer mint", &fields)),
    }
}

/// A fresh 16-hex-digit Android id. Any value works — Google only requires
/// that the same id be used for both exchanges — so it needs no crypto RNG.
pub fn new_android_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_value_lines_and_keeps_equals_in_values() {
        let body =
            "Token=aas_et/abc==\nEmail=user@example.com\n\nAuth=ya29.x=y\nExpiry=1700000000\n";
        let m = parse_response(body);
        assert_eq!(m["Token"], "aas_et/abc==");
        assert_eq!(m["Auth"], "ya29.x=y");
        assert_eq!(m["Expiry"], "1700000000");
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn exchange_form_carries_the_ac2dm_service_and_gms_signature() {
        let f = exchange_form("oauth2_4/tok", "0123456789abcdef", "owner@example.com");
        let get = |k: &str| f.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
        assert_eq!(get("service"), Some("ac2dm"));
        assert_eq!(get("Token"), Some("oauth2_4/tok"));
        assert_eq!(get("client_sig"), Some(GMS_CLIENT_SIG));
        assert_eq!(get("callerSig"), Some(GMS_CLIENT_SIG));
        assert_eq!(get("androidId"), Some("0123456789abcdef"));
    }

    #[test]
    fn auth_token_form_targets_the_home_app() {
        let f = auth_token_form(
            "aas_et/m",
            "0123456789abcdef",
            HOMEGRAPH_SCOPE,
            "owner@example.com",
        );
        let get = |k: &str| f.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
        assert_eq!(get("EncryptedPasswd"), Some("aas_et/m"));
        assert_eq!(get("app"), Some(HOME_APP));
        assert_eq!(get("client_sig"), Some(HOME_APP_SIG));
        assert_eq!(get("service"), Some(HOMEGRAPH_SCOPE));
        assert!(get("Token").is_none());
    }

    #[test]
    fn bad_authentication_is_an_auth_error_and_the_rest_is_upstream() {
        let mut m = BTreeMap::new();
        m.insert("Error".to_string(), "BadAuthentication".to_string());
        assert!(matches!(failure("x", &m), CliError::Auth(_)));
        m.insert("Error".to_string(), "ServiceUnavailable".to_string());
        assert!(matches!(failure("x", &m), CliError::Upstream(_)));
    }

    #[test]
    fn android_id_is_sixteen_hex_digits() {
        let id = new_android_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
