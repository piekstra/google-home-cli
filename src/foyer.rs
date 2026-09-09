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

/// How a binary body is carried on the HTTP/1.1 gateway: plain protobuf on
/// the `$rpc` path, or gRPC-web framing. Native gRPC lives in `crate::grpc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryMode {
    Proto,
    GrpcWeb,
}

pub struct Foyer<'a> {
    client: Client,
    session: &'a mut Session,
    creds: &'a CredentialStore,
    verbose: bool,
    last_grpc_status: Option<(i32, String)>,
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
            last_grpc_status: None,
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

    /// Same call, but the body is already-serialized protobuf and the
    /// response bytes come back untouched. Bypasses the JSON gateway's type
    /// resolution, which rejects `Any` payloads whose type it doesn't know.
    ///
    /// `grpc_web` wraps the body in a gRPC-web frame (`application/grpc-web+proto`)
    /// and unwraps the response: message frames are concatenated, and the
    /// trailer frame's `grpc-status` decides success, since gRPC-web answers
    /// HTTP 200 even for failed calls.
    /// gRPC status carried in response headers (a "trailers-only" reply) on
    /// the last binary call, if the server sent one.
    pub fn last_grpc_status(&self) -> Option<(i32, String)> {
        self.last_grpc_status.clone()
    }

    pub fn call_binary(
        &mut self,
        url: &str,
        body: &[u8],
        mode: BinaryMode,
    ) -> Result<(u16, Vec<u8>), CliError> {
        let grpc_web = mode != BinaryMode::Proto;
        let framed;
        let body = if grpc_web {
            let mut f = Vec::with_capacity(body.len() + 5);
            f.push(0);
            f.extend_from_slice(&(body.len() as u32).to_be_bytes());
            f.extend_from_slice(body);
            framed = f;
            &framed[..]
        } else {
            body
        };
        let bearer = self.session.bearer(&self.client, self.creds)?;
        let (status, bytes) = self.post_binary(url, body, &bearer, mode)?;
        let (status, bytes) = if status == 401 || status == 403 {
            let bearer = self.session.refresh_bearer(&self.client, self.creds)?;
            self.post_binary(url, body, &bearer, mode)?
        } else {
            (status, bytes)
        };
        if grpc_web && (200..=299).contains(&status) {
            return unframe_grpc_web(&bytes).map(|b| (status, b));
        }
        match status {
            200..=299 => Ok((status, bytes)),
            401 | 403 => Err(CliError::Auth(format!(
                "foyer rejected the credential (HTTP {status}); run `ghome auth login` again"
            ))),
            404 => Err(CliError::NotFound(format!("HTTP 404 for {url}"))),
            _ => Err(CliError::Upstream(format!(
                "HTTP {status}: {}",
                snippet(&String::from_utf8_lossy(&bytes))
            ))),
        }
    }

    fn post_binary(
        &mut self,
        url: &str,
        body: &[u8],
        bearer: &str,
        mode: BinaryMode,
    ) -> Result<(u16, Vec<u8>), CliError> {
        let content_type = match mode {
            BinaryMode::Proto => "application/x-protobuf",
            BinaryMode::GrpcWeb => "application/grpc-web+proto",
        };
        if self.verbose {
            eprintln!("POST {url} ({} bytes, {content_type})", body.len());
        }
        let mut req = self
            .client
            .post(url)
            .bearer_auth(bearer)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .header(reqwest::header::ACCEPT, content_type)
            .header("X-User-Agent", X_USER_AGENT);
        match mode {
            BinaryMode::GrpcWeb => req = req.header("X-Grpc-Web", "1"),
            BinaryMode::Proto => {}
        }
        let resp = req
            .body(body.to_vec())
            .send()
            .map_err(|e| CliError::Upstream(format!("request: {e}")))?;
        let status = resp.status().as_u16();
        let hdr = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        self.last_grpc_status = hdr("grpc-status").and_then(|c| {
            c.trim().parse::<i32>().ok().map(|code| {
                (
                    code,
                    percent_decode(&hdr("grpc-message").unwrap_or_default()),
                )
            })
        });
        if self.verbose {
            if let Some((code, msg)) = &self.last_grpc_status {
                eprintln!("grpc-status {code}: {msg}");
            }
        }
        let bytes = resp
            .bytes()
            .map_err(|e| CliError::Upstream(format!("reading response: {e}")))?
            .to_vec();
        if self.verbose {
            eprintln!("HTTP {status} ({} bytes)", bytes.len());
        }
        Ok((status, bytes))
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

/// Split a gRPC-web response into its message bytes and its trailer
/// status. Frames are `[flags:1][len:4 BE][data]`; flag bit 0x80 marks the
/// trailer frame, a text block of `grpc-status: N` / `grpc-message: …`.
pub fn unframe_grpc_web(bytes: &[u8]) -> Result<Vec<u8>, CliError> {
    let mut i = 0;
    let mut messages = Vec::new();
    let mut status: Option<(u32, String)> = None;
    while i + 5 <= bytes.len() {
        let flags = bytes[i];
        let len =
            u32::from_be_bytes([bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]]) as usize;
        let end = (i + 5 + len).min(bytes.len());
        let data = &bytes[i + 5..end];
        if flags & 0x80 != 0 {
            let text = String::from_utf8_lossy(data);
            let mut code = 0;
            let mut msg = String::new();
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("grpc-status:") {
                    code = v.trim().parse().unwrap_or(2);
                } else if let Some(v) = line.strip_prefix("grpc-message:") {
                    msg = percent_decode(v.trim());
                }
            }
            status = Some((code, msg));
        } else {
            messages.extend_from_slice(data);
        }
        i = end;
    }
    match status {
        Some((0, _)) | None => Ok(messages),
        Some((code, msg)) => Err(match code {
            16 | 7 => CliError::Auth(format!("grpc status {code}: {msg}")),
            5 => CliError::NotFound(format!("grpc status {code}: {msg}")),
            _ => CliError::Upstream(format!("grpc status {code}: {msg}")),
        }),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn grpc_web_frames_unwrap_and_trailers_decide() {
        let mut ok = vec![0, 0, 0, 0, 3, 1, 2, 3];
        let trailer = b"grpc-status: 0\r\n";
        ok.push(0x80);
        ok.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
        ok.extend_from_slice(trailer);
        assert_eq!(unframe_grpc_web(&ok).unwrap(), vec![1, 2, 3]);
        let t = b"grpc-status: 3\r\ngrpc-message: bad%20thing\r\n";
        let mut bad = vec![0x80];
        bad.extend_from_slice(&(t.len() as u32).to_be_bytes());
        bad.extend_from_slice(t);
        match unframe_grpc_web(&bad) {
            Err(CliError::Upstream(m)) => assert!(m.contains("bad thing")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn xssi_prefix_is_stripped() {
        assert_eq!(strip_xssi(")]}'\n[1,2]"), "[1,2]");
        assert_eq!(strip_xssi("[1,2]"), "[1,2]");
    }
}
