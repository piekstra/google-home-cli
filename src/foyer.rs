//! The Foyer RPC client — the private API behind the Google Home app and
//! home.google.com, spoken as gRPC-web JSON.
//!
//! Every call is `POST <BASE><Service>/<Method>` with
//! `Content-Type: application/json+protobuf`; request and response bodies are
//! **positional JSON arrays** (protobuf field number N → array index N-1,
//! `null` for absent fields), not JSON objects. See `docs/api.md`.
//!
//! With a Bearer token, send **no** `X-Goog-Api-Key`: the web app's key
//! belongs to a different Google project than the Android app the bearer is
//! minted for, and Foyer rejects the mismatch with HTTP 400 `CONSUMER_INVALID`.

use pk_cli_core::CliError;
use pk_cli_secrets::CredentialStore;
use reqwest::blocking::Client;
use serde_json::Value;

use crate::session::Session;

pub const BASE: &str =
    "https://googlehomefoyer-pa.clients6.google.com/$rpc/google.internal.home.foyer.v1.";
const CONTENT_TYPE: &str = "application/json+protobuf";
const X_USER_AGENT: &str = "grpc-web-javascript/0.1";

pub const STRUCTURES: &str = "StructuresService";
pub const GET_HOME_GRAPH: &str = "GetHomeGraph";
pub const HOME_DEVICES: &str = "HomeDevicesService";
pub const LIST_UNASSIGNED_DEVICES: &str = "ListUnassignedDevices";
pub const HOME_CONTROL: &str = "HomeControlService";
pub const GET_TRAITS: &str = "GetTraits";

/// Resolve an `api` path against the RPC base: `Service/Method` (with or
/// without a leading slash) or a full URL.
pub fn url_for(path: &str) -> String {
    if path.starts_with("https://") || path.starts_with("http://") {
        path.to_string()
    } else {
        format!("{BASE}{}", path.trim_start_matches('/'))
    }
}

/// Some clients6 endpoints prepend an XSSI guard line; strip it before parsing.
pub fn strip_xssi(body: &str) -> &str {
    let t = body.trim_start();
    match t.strip_prefix(")]}'") {
        Some(rest) => rest.trim_start_matches(['\r', '\n']),
        None => body,
    }
}

pub struct Foyer<'a> {
    client: Client,
    session: &'a mut Session,
    creds: &'a CredentialStore,
    verbose: bool,
}

impl<'a> Foyer<'a> {
    pub fn new(
        client: Client,
        session: &'a mut Session,
        creds: &'a CredentialStore,
        verbose: bool,
    ) -> Self {
        Foyer {
            client,
            session,
            creds,
            verbose,
        }
    }

    /// Call `Service/Method` with a positional-array body. A 401/403 mints a
    /// fresh bearer and retries exactly once; every RPC here is a read or an
    /// idempotent write, so the retry is safe.
    pub fn rpc(&mut self, service: &str, method: &str, body: &Value) -> Result<Value, CliError> {
        self.call(&url_for(&format!("{service}/{method}")), body)
    }

    pub fn call(&mut self, url: &str, body: &Value) -> Result<Value, CliError> {
        let bearer = self.session.bearer(&self.client, self.creds)?;
        let (status, text) = self.post(url, body, &bearer)?;
        let (status, text) = if status == 401 || status == 403 {
            if self.verbose {
                eprintln!("foyer: HTTP {status}, minting a fresh bearer and retrying once");
            }
            let bearer = self.session.refresh_bearer(&self.client, self.creds)?;
            self.post(url, body, &bearer)?
        } else {
            (status, text)
        };
        match status {
            200..=299 => serde_json::from_str(strip_xssi(&text))
                .map_err(|e| CliError::Upstream(format!("foyer returned non-JSON: {e}"))),
            401 | 403 => Err(CliError::Auth(format!(
                "foyer rejected the credential (HTTP {status}); run `ghome auth login` again"
            ))),
            404 => Err(CliError::NotFound(format!("HTTP 404 from foyer for {url}"))),
            _ => Err(CliError::Upstream(format!(
                "foyer HTTP {status}: {}",
                snippet(&text)
            ))),
        }
    }

    fn post(&self, url: &str, body: &Value, bearer: &str) -> Result<(u16, String), CliError> {
        if self.verbose {
            eprintln!("POST {url}");
        }
        let resp = self
            .client
            .post(url)
            .bearer_auth(bearer)
            .header(reqwest::header::CONTENT_TYPE, CONTENT_TYPE)
            .header("X-User-Agent", X_USER_AGENT)
            .body(body.to_string())
            .send()
            .map_err(|e| CliError::Upstream(format!("foyer request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .map_err(|e| CliError::Upstream(format!("reading foyer response: {e}")))?;
        if self.verbose {
            eprintln!("HTTP {status} ({} bytes)", text.len());
        }
        Ok((status, text))
    }
}

fn snippet(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() > 300 {
        format!("{}…", t.chars().take(300).collect::<String>())
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_against_the_rpc_base() {
        assert_eq!(
            url_for("StructuresService/GetHomeGraph"),
            format!("{BASE}StructuresService/GetHomeGraph")
        );
        assert_eq!(
            url_for("/StructuresService/GetHomeGraph"),
            format!("{BASE}StructuresService/GetHomeGraph")
        );
        assert_eq!(url_for("https://x.example/y"), "https://x.example/y");
    }

    #[test]
    fn xssi_prefix_is_stripped() {
        assert_eq!(strip_xssi(")]}'\n[1,2]"), "[1,2]");
        assert_eq!(strip_xssi("[1,2]"), "[1,2]");
    }
}
