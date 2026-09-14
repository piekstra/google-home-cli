//! Speak on a speaker or display over the LAN with the Cast protocol: no
//! Google account involved. This is the fallback while the cloud broadcast
//! path stays closed (`announce.rs`, docs/api.md).
//!
//! Discovery is mDNS `_googlecast._tcp`; the TXT record's `fn` is the name
//! the Home app shows, `id` the device id, `md` the model. Transport is TLS
//! to port 8009 (the device presents a self-signed certificate, which is
//! why verification is off) carrying CastMessage v2 frames: a 4-byte
//! big-endian length, then a protobuf with `source_id`, `destination_id`,
//! `namespace` and a JSON `payload_utf8`.
//!
//! One announcement is: CONNECT to `receiver-0` → LAUNCH the Default Media
//! Receiver (`CC1AD845`) → RECEIVER_STATUS gives the app's `transportId` →
//! CONNECT to it → LOAD the audio URL → MEDIA_STATUS until the player goes
//! IDLE with `FINISHED` → STOP the app so a display returns to its ambient
//! screen. Whatever was playing before is interrupted, as with a broadcast.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use pk_cli_core::CliError;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SERVICE_TYPE: &str = "_googlecast._tcp.local.";
const DEFAULT_MEDIA_RECEIVER: &str = "CC1AD845";
const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
const SENDER: &str = "sender-ghome";
const RECEIVER: &str = "receiver-0";
/// How long one reply may take; Cast devices answer within a second.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a spoken message may play before we give up waiting.
const PLAYBACK_TIMEOUT: Duration = Duration::from_secs(90);

/// A Cast-capable device found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastDevice {
    /// The friendly name (TXT `fn`), which is what Google Home calls it too.
    pub name: String,
    /// TXT `id`, the Cast device id.
    pub id: String,
    /// TXT `md`, e.g. `Google Nest Hub Max`.
    pub model: String,
    pub ip: IpAddr,
    pub port: u16,
}

/// Browse the LAN for `window` and return every Cast device seen, sorted by
/// name. Devices answer within a second or two; three seconds is generous.
pub fn discover(window: Duration) -> Result<Vec<CastDevice>, CliError> {
    let daemon = mdns_sd::ServiceDaemon::new()
        .map_err(|e| CliError::Other(format!("mDNS is unavailable: {e}")))?;
    let rx = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| CliError::Other(format!("mDNS browse failed: {e}")))?;
    let deadline = std::time::Instant::now() + window;
    let mut found: Vec<CastDevice> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(event) = rx.recv_timeout(left) else {
            break;
        };
        if let mdns_sd::ServiceEvent::ServiceResolved(s) = event {
            let txt = |k: &str| s.txt_properties.get_property_val_str(k).unwrap_or("");
            let Some(ip) = s
                .addresses
                .iter()
                .map(|a| a.to_ip_addr())
                .find(|ip| ip.is_ipv4())
                .or_else(|| s.addresses.iter().map(|a| a.to_ip_addr()).next())
            else {
                continue;
            };
            let dev = CastDevice {
                name: txt("fn").to_string(),
                id: txt("id").to_string(),
                model: txt("md").to_string(),
                ip,
                port: s.port,
            };
            if !found.iter().any(|d| d.id == dev.id) {
                found.push(dev);
            }
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    found.sort_by_key(|d| d.name.to_lowercase());
    Ok(found)
}

/// Percent-encode for a query string (RFC 3986 unreserved characters pass).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Generated speech for `message`: Google Translate's text-to-speech
/// endpoint, which Cast devices fetch directly (the same trick the
/// home-automation crowd has used for years; unofficial, ≤ 200 characters).
pub fn tts_url(message: &str, lang: &str) -> String {
    format!(
        "https://translate.google.com/translate_tts?ie=UTF-8&client=tw-ob&tl={}&q={}",
        url_encode(lang),
        url_encode(message)
    )
}

// ---- CastMessage v2 framing --------------------------------------------

fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn str_field(field: u32, s: &str, out: &mut Vec<u8>) {
    varint(u64::from(field << 3 | 2), out);
    varint(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

/// `CastMessage{1: CASTV2_1_0, 2: source, 3: destination, 4: namespace, 5: STRING, 6: payload}`.
pub fn encode(source: &str, destination: &str, namespace: &str, payload: &str) -> Vec<u8> {
    let mut m = Vec::new();
    varint(u64::from(1u32 << 3), &mut m);
    varint(0, &mut m); // protocol_version CASTV2_1_0
    str_field(2, source, &mut m);
    str_field(3, destination, &mut m);
    str_field(4, namespace, &mut m);
    varint(u64::from(5u32 << 3), &mut m);
    varint(0, &mut m); // payload_type STRING
    str_field(6, payload, &mut m);
    let mut frame = (m.len() as u32).to_be_bytes().to_vec();
    frame.extend(m);
    frame
}

/// The parts of an incoming CastMessage we read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    pub source: String,
    pub destination: String,
    pub namespace: String,
    pub payload: String,
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut n = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*pos)?;
        *pos += 1;
        n |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(n);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Decode a CastMessage body (without the length prefix).
pub fn decode(buf: &[u8]) -> Option<Message> {
    let mut m = Message::default();
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let (field, wire) = (tag >> 3, tag & 7);
        match wire {
            0 => {
                read_varint(buf, &mut pos)?;
            }
            1 => pos += 8,
            5 => pos += 4,
            2 => {
                let len = read_varint(buf, &mut pos)? as usize;
                let bytes = buf.get(pos..pos + len)?;
                pos += len;
                let s = String::from_utf8_lossy(bytes).into_owned();
                match field {
                    2 => m.source = s,
                    3 => m.destination = s,
                    4 => m.namespace = s,
                    6 => m.payload = s,
                    _ => {}
                }
            }
            _ => return None,
        }
    }
    Some(m)
}

/// The Default Media Receiver's transport id from a RECEIVER_STATUS.
pub fn transport_id(status: &Value) -> Option<String> {
    status
        .pointer("/status/applications")?
        .as_array()?
        .iter()
        .find(|a| a.get("appId").and_then(Value::as_str) == Some(DEFAULT_MEDIA_RECEIVER))
        .and_then(|a| a.get("transportId").and_then(Value::as_str))
        .map(str::to_string)
}

/// The receiver's volume level (0.0–1.0) from a RECEIVER_STATUS.
pub fn volume_level(status: &Value) -> Option<f64> {
    status.pointer("/status/volume/level")?.as_f64()
}

/// The player state of the first media session in a MEDIA_STATUS, with
/// its idle reason (`FINISHED`, `ERROR`, `CANCELLED`, `INTERRUPTED`) when idle.
pub fn player_state(status: &Value) -> Option<(String, Option<String>)> {
    let first = status.get("status")?.as_array()?.first()?;
    let state = first.get("playerState")?.as_str()?.to_string();
    let reason = first
        .get("idleReason")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((state, reason))
}

// ---- the session ----------------------------------------------------------

/// What one announcement did on one device.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Outcome {
    pub name: String,
    pub model: String,
    pub ip: String,
    pub played: bool,
    /// The player's final state, e.g. `IDLE/FINISHED`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Play `url` on `dev` and wait for it to finish. `volume`, when given, is
/// set for the announcement and the previous level restored afterwards.
pub fn play(
    dev: &CastDevice,
    url: &str,
    title: &str,
    volume: Option<u8>,
    verbose: bool,
) -> Outcome {
    let mut out = Outcome {
        name: dev.name.clone(),
        model: dev.model.clone(),
        ip: dev.ip.to_string(),
        played: false,
        final_state: None,
        error: None,
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            out.error = Some(format!("runtime: {e}"));
            return out;
        }
    };
    match rt.block_on(session(dev, url, title, volume, verbose)) {
        Ok(state) => {
            out.played = true;
            out.final_state = Some(state);
        }
        Err(e) => out.error = Some(e),
    }
    out
}

/// A rustls verifier that accepts the device's self-signed certificate.
/// The connection is still encrypted; what is not checked is who is on
/// the other end, which on a LAN with a name-matched mDNS answer is the
/// same trust the Home app extends.
#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

type Tls = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

struct Conn {
    tls: Tls,
    verbose: bool,
    request_id: u64,
}

impl Conn {
    async fn open(dev: &CastDevice, verbose: bool) -> Result<Conn, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
            .with_no_client_auth();
        let tcp = tokio::time::timeout(
            REPLY_TIMEOUT,
            tokio::net::TcpStream::connect((dev.ip, dev.port)),
        )
        .await
        .map_err(|_| format!("connect to {}:{} timed out", dev.ip, dev.port))?
        .map_err(|e| format!("connect to {}:{}: {e}", dev.ip, dev.port))?;
        let name = rustls::pki_types::ServerName::IpAddress(dev.ip.into());
        let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS to {}: {e}", dev.ip))?;
        Ok(Conn {
            tls,
            verbose,
            request_id: 0,
        })
    }

    fn next_id(&mut self) -> u64 {
        self.request_id += 1;
        self.request_id
    }

    async fn send(&mut self, dest: &str, ns: &str, payload: &Value) -> Result<(), String> {
        let text = payload.to_string();
        if self.verbose {
            eprintln!(
                "cast → {dest} {}: {text}",
                ns.rsplit('.').next().unwrap_or(ns)
            );
        }
        self.tls
            .write_all(&encode(SENDER, dest, ns, &text))
            .await
            .map_err(|e| format!("send: {e}"))
    }

    /// The next message, answering heartbeats along the way.
    async fn recv(&mut self) -> Result<Message, String> {
        loop {
            let mut len = [0u8; 4];
            self.tls
                .read_exact(&mut len)
                .await
                .map_err(|e| format!("read: {e}"))?;
            let n = u32::from_be_bytes(len) as usize;
            if n > 1 << 20 {
                return Err(format!("frame of {n} bytes is not a Cast message"));
            }
            let mut body = vec![0u8; n];
            self.tls
                .read_exact(&mut body)
                .await
                .map_err(|e| format!("read: {e}"))?;
            let Some(m) = decode(&body) else {
                return Err("undecodable Cast message".into());
            };
            if self.verbose {
                eprintln!(
                    "cast ← {} {}: {}",
                    m.source,
                    m.namespace.rsplit('.').next().unwrap_or(&m.namespace),
                    m.payload.chars().take(200).collect::<String>()
                );
            }
            if m.namespace == NS_HEARTBEAT {
                if m.payload.contains("PING") {
                    let src = m.source.clone();
                    self.send(&src, NS_HEARTBEAT, &json!({"type": "PONG"}))
                        .await?;
                }
                continue;
            }
            return Ok(m);
        }
    }

    /// Wait for a message in `ns` whose JSON satisfies `want`.
    async fn wait_for(
        &mut self,
        ns: &str,
        limit: Duration,
        mut want: impl FnMut(&Value) -> bool,
    ) -> Result<Value, String> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let left = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or_else(|| "the device did not answer in time".to_string())?;
            let m = tokio::time::timeout(left, self.recv())
                .await
                .map_err(|_| "the device did not answer in time".to_string())??;
            if m.namespace != ns {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&m.payload) else {
                continue;
            };
            if v.get("type").and_then(Value::as_str) == Some("LAUNCH_ERROR") {
                return Err(format!(
                    "the device refused to launch the media receiver: {}",
                    v.get("reason").and_then(Value::as_str).unwrap_or("unknown")
                ));
            }
            if want(&v) {
                return Ok(v);
            }
        }
    }
}

/// One full announcement; returns the player's final state.
async fn session(
    dev: &CastDevice,
    url: &str,
    title: &str,
    volume: Option<u8>,
    verbose: bool,
) -> Result<String, String> {
    let mut c = Conn::open(dev, verbose).await?;
    c.send(RECEIVER, NS_CONNECTION, &json!({"type": "CONNECT"}))
        .await?;

    // Where the volume is now, so it can be put back.
    let id = c.next_id();
    c.send(
        RECEIVER,
        NS_RECEIVER,
        &json!({"type": "GET_STATUS", "requestId": id}),
    )
    .await?;
    let status = c
        .wait_for(NS_RECEIVER, REPLY_TIMEOUT, |v| {
            v.get("requestId").and_then(Value::as_u64) == Some(id)
        })
        .await?;
    let previous = volume_level(&status);
    if let Some(pct) = volume {
        let id = c.next_id();
        c.send(
            RECEIVER,
            NS_RECEIVER,
            &json!({"type": "SET_VOLUME", "requestId": id, "volume": {"level": f64::from(pct) / 100.0}}),
        )
        .await?;
        c.wait_for(NS_RECEIVER, REPLY_TIMEOUT, |v| {
            v.get("requestId").and_then(Value::as_u64) == Some(id)
        })
        .await?;
    }

    let id = c.next_id();
    c.send(
        RECEIVER,
        NS_RECEIVER,
        &json!({"type": "LAUNCH", "requestId": id, "appId": DEFAULT_MEDIA_RECEIVER}),
    )
    .await?;
    let launched = c
        .wait_for(NS_RECEIVER, REPLY_TIMEOUT, |v| transport_id(v).is_some())
        .await?;
    let transport = transport_id(&launched).expect("checked by wait_for");
    let session_id = launched
        .pointer("/status/applications/0/sessionId")
        .and_then(Value::as_str)
        .map(str::to_string);

    c.send(&transport, NS_CONNECTION, &json!({"type": "CONNECT"}))
        .await?;
    let id = c.next_id();
    c.send(
        &transport,
        NS_MEDIA,
        &json!({
            "type": "LOAD",
            "requestId": id,
            "autoplay": true,
            "media": {
                "contentId": url,
                "contentType": "audio/mpeg",
                "streamType": "BUFFERED",
                "metadata": {"metadataType": 0, "title": title}
            }
        }),
    )
    .await?;
    // First a status that says it is playing (or failed to), then one that
    // says it finished.
    let started = c
        .wait_for(NS_MEDIA, REPLY_TIMEOUT, |v| {
            let t = v.get("type").and_then(Value::as_str);
            t == Some("LOAD_FAILED")
                || t == Some("LOAD_CANCELLED")
                || player_state(v).is_some_and(|(s, _)| s == "PLAYING" || s == "BUFFERING")
        })
        .await?;
    if started.get("type").and_then(Value::as_str) != Some("MEDIA_STATUS") {
        return Err(format!(
            "the device could not play the audio ({})",
            started.get("type").and_then(Value::as_str).unwrap_or("?")
        ));
    }
    let done = c
        .wait_for(NS_MEDIA, PLAYBACK_TIMEOUT, |v| {
            player_state(v).is_some_and(|(s, _)| s == "IDLE")
        })
        .await?;
    let (state, reason) = player_state(&done).unwrap_or_default();
    let final_state = match reason {
        Some(r) => format!("{state}/{r}"),
        None => state,
    };

    // Leave the device as it was: previous volume, no app on screen.
    if volume.is_some() {
        if let Some(level) = previous {
            let id = c.next_id();
            let _ = c
                .send(
                    RECEIVER,
                    NS_RECEIVER,
                    &json!({"type": "SET_VOLUME", "requestId": id, "volume": {"level": level}}),
                )
                .await;
            let _ = c
                .wait_for(NS_RECEIVER, REPLY_TIMEOUT, |v| {
                    v.get("requestId").and_then(Value::as_u64) == Some(id)
                })
                .await;
        }
    }
    if let Some(sid) = session_id {
        let id = c.next_id();
        let _ = c
            .send(
                RECEIVER,
                NS_RECEIVER,
                &json!({"type": "STOP", "requestId": id, "sessionId": sid}),
            )
            .await;
    }
    let _ = c
        .send(RECEIVER, NS_CONNECTION, &json!({"type": "CLOSE"}))
        .await;
    if final_state.ends_with("/ERROR") {
        return Err(format!(
            "the device stopped with an error ({final_state}); it may not be able to fetch the audio"
        ));
    }
    Ok(final_state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let frame = encode(
            "sender-0",
            "receiver-0",
            NS_CONNECTION,
            r#"{"type":"CONNECT"}"#,
        );
        let len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(len, frame.len() - 4);
        let m = decode(&frame[4..]).unwrap();
        assert_eq!(m.source, "sender-0");
        assert_eq!(m.destination, "receiver-0");
        assert_eq!(m.namespace, NS_CONNECTION);
        assert_eq!(m.payload, r#"{"type":"CONNECT"}"#);
        assert!(decode(&[0xff, 0xff]).is_none());
    }

    #[test]
    fn statuses_parse() {
        let rs = json!({"type": "RECEIVER_STATUS", "requestId": 1, "status": {
            "applications": [{"appId": "CC1AD845", "transportId": "web-7", "sessionId": "s1"}],
            "volume": {"level": 0.35, "muted": false}}});
        assert_eq!(transport_id(&rs).as_deref(), Some("web-7"));
        assert_eq!(volume_level(&rs), Some(0.35));
        let idle = json!({"type": "RECEIVER_STATUS", "status": {"volume": {"level": 0.5}}});
        assert!(transport_id(&idle).is_none());
        let ms = json!({"type": "MEDIA_STATUS", "status": [{"mediaSessionId": 1, "playerState": "IDLE", "idleReason": "FINISHED"}]});
        assert_eq!(
            player_state(&ms),
            Some(("IDLE".to_string(), Some("FINISHED".to_string())))
        );
        assert!(player_state(&json!({"type": "MEDIA_STATUS", "status": []})).is_none());
    }

    #[test]
    fn speech_urls_are_encoded() {
        let u = tts_url("Dinner's ready, come down!", "en-GB");
        assert!(u.starts_with(
            "https://translate.google.com/translate_tts?ie=UTF-8&client=tw-ob&tl=en-GB&q="
        ));
        assert!(u.ends_with("Dinner%27s%20ready%2C%20come%20down%21"));
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(url_encode("é"), "%C3%A9");
    }
}
