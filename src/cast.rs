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
//! IDLE (`FINISHED` is success; `CANCELLED`, `INTERRUPTED` and `ERROR` are
//! not) → restore the volume and STOP the app, on every path, so a display
//! returns to its ambient screen. Whatever was playing before is
//! interrupted, as with a broadcast.
//!
//! `Conn` is generic over the stream so the session can be driven offline
//! by a scripted device in the tests; `Conn::open` is the TLS constructor.

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use pk_cli_core::CliError;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

// ---- errors ----------------------------------------------------------------

/// Why an announcement did not play, by what the caller can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastError {
    /// No TCP connection to the device.
    Connect(String),
    /// The TLS handshake failed.
    Tls(String),
    /// The device refused to launch the media receiver (its reason).
    LaunchRefused(String),
    /// The device could not load the audio (usually it cannot fetch the URL).
    LoadFailed(String),
    /// Playback ended for a reason other than finishing (`CANCELLED`, …).
    Playback(String),
    /// A reply did not arrive in time (what we were waiting for).
    Timeout(String),
    /// Something on the wire was not a Cast message, or the socket died.
    Protocol(String),
}

impl CastError {
    /// A stable label for `--json` consumers.
    pub fn kind(&self) -> &'static str {
        match self {
            CastError::Connect(_) => "connect",
            CastError::Tls(_) => "tls",
            CastError::LaunchRefused(_) => "launch_refused",
            CastError::LoadFailed(_) => "load_failed",
            CastError::Playback(_) => "playback",
            CastError::Timeout(_) => "timeout",
            CastError::Protocol(_) => "protocol",
        }
    }
}

impl fmt::Display for CastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CastError::Connect(e) => write!(f, "could not connect: {e}"),
            CastError::Tls(e) => write!(f, "TLS handshake failed: {e}"),
            CastError::LaunchRefused(r) => {
                write!(f, "the device refused to launch the media receiver ({r})")
            }
            CastError::LoadFailed(r) => write!(
                f,
                "the device could not play the audio ({r}); it may not be able to fetch it"
            ),
            CastError::Playback(r) => write!(f, "playback ended early ({r})"),
            CastError::Timeout(what) => write!(f, "no answer in time while waiting for {what}"),
            CastError::Protocol(e) => write!(f, "{e}"),
        }
    }
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

fn msg_type(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

// ---- the connection --------------------------------------------------------

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

/// One Cast connection: the message pump over any async stream.
pub struct Conn<S> {
    stream: S,
    verbose: bool,
    request_id: u64,
}

impl Conn<Tls> {
    /// TLS to the device's Cast port.
    pub async fn open(dev: &CastDevice, verbose: bool) -> Result<Conn<Tls>, CastError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| CastError::Tls(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
            .with_no_client_auth();
        let tcp = tokio::time::timeout(
            REPLY_TIMEOUT,
            tokio::net::TcpStream::connect((dev.ip, dev.port)),
        )
        .await
        .map_err(|_| CastError::Connect(format!("{}:{} timed out", dev.ip, dev.port)))?
        .map_err(|e| CastError::Connect(format!("{}:{}: {e}", dev.ip, dev.port)))?;
        let name = rustls::pki_types::ServerName::IpAddress(dev.ip.into());
        let stream = tokio_rustls::TlsConnector::from(Arc::new(cfg))
            .connect(name, tcp)
            .await
            .map_err(|e| CastError::Tls(e.to_string()))?;
        Ok(Conn::new(stream, verbose))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    pub fn new(stream: S, verbose: bool) -> Conn<S> {
        Conn {
            stream,
            verbose,
            request_id: 0,
        }
    }

    fn next_id(&mut self) -> u64 {
        self.request_id += 1;
        self.request_id
    }

    async fn send(&mut self, dest: &str, ns: &str, payload: &Value) -> Result<(), CastError> {
        let text = payload.to_string();
        if self.verbose {
            eprintln!(
                "cast → {dest} {}: {text}",
                ns.rsplit('.').next().unwrap_or(ns)
            );
        }
        self.stream
            .write_all(&encode(SENDER, dest, ns, &text))
            .await
            .map_err(|e| CastError::Protocol(format!("send: {e}")))
    }

    /// The next message, answering heartbeats along the way.
    async fn recv(&mut self) -> Result<Message, CastError> {
        loop {
            let mut len = [0u8; 4];
            self.stream
                .read_exact(&mut len)
                .await
                .map_err(|e| CastError::Protocol(format!("read: {e}")))?;
            let n = u32::from_be_bytes(len) as usize;
            if n > 1 << 20 {
                return Err(CastError::Protocol(format!(
                    "frame of {n} bytes is not a Cast message"
                )));
            }
            let mut body = vec![0u8; n];
            self.stream
                .read_exact(&mut body)
                .await
                .map_err(|e| CastError::Protocol(format!("read: {e}")))?;
            let Some(m) = decode(&body) else {
                return Err(CastError::Protocol("undecodable Cast message".into()));
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

    /// The first message in `ns` whose JSON satisfies `want`; `what` names
    /// it for the timeout error.
    async fn wait_for(
        &mut self,
        ns: &str,
        what: &str,
        limit: Duration,
        mut want: impl FnMut(&Value) -> bool,
    ) -> Result<Value, CastError> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let left = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or_else(|| CastError::Timeout(what.to_string()))?;
            let m = tokio::time::timeout(left, self.recv())
                .await
                .map_err(|_| CastError::Timeout(what.to_string()))??;
            if m.namespace != ns {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&m.payload) else {
                continue;
            };
            if want(&v) {
                return Ok(v);
            }
        }
    }

    /// A receiver request whose RECEIVER_STATUS reply is awaited by request id.
    async fn receiver_request(&mut self, mut payload: Value) -> Result<Value, CastError> {
        let id = self.next_id();
        payload["requestId"] = json!(id);
        let what = format!("the reply to {}", msg_type(&payload));
        self.send(RECEIVER, NS_RECEIVER, &payload).await?;
        self.wait_for(NS_RECEIVER, &what, REPLY_TIMEOUT, |v| {
            v.get("requestId").and_then(Value::as_u64) == Some(id)
        })
        .await
    }
}

// ---- the session -----------------------------------------------------------

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
    /// `CastError::kind` for the error, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
    /// Cleanup that did not take (volume not restored, app not stopped);
    /// the announcement itself still played.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Play `url` on `dev` and wait for it to finish. `volume`, when given, is
/// set for the announcement and the previous level restored afterwards.
pub async fn play(
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
        error_kind: None,
        warnings: Vec::new(),
    };
    let (result, warnings) = match Conn::open(dev, verbose).await {
        Ok(mut c) => session(&mut c, url, title, volume).await,
        Err(e) => (Err(e), Vec::new()),
    };
    out.warnings = warnings;
    match result {
        Ok(state) => {
            out.played = true;
            out.final_state = Some(state);
        }
        Err(e) => {
            out.error_kind = Some(e.kind());
            out.error = Some(e.to_string());
        }
    }
    out
}

/// One full announcement on an open connection: the result of playback,
/// plus any cleanup (volume restore, STOP, CLOSE) that failed. Cleanup
/// runs whatever happened once the volume may have been changed.
pub async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    c: &mut Conn<S>,
    url: &str,
    title: &str,
    volume: Option<u8>,
) -> (Result<String, CastError>, Vec<String>) {
    let mut warnings = Vec::new();
    if let Err(e) = c
        .send(RECEIVER, NS_CONNECTION, &json!({"type": "CONNECT"}))
        .await
    {
        return (Err(e), warnings);
    }
    // Where the volume is now, so it can be put back.
    let status = match c.receiver_request(json!({"type": "GET_STATUS"})).await {
        Ok(s) => s,
        Err(e) => return (Err(e), warnings),
    };
    let previous = volume.and(volume_level(&status));

    let mut session_id: Option<String> = None;
    let played = playback(c, url, title, volume, &mut session_id).await;

    // Leave the device as it was: previous volume, no app on screen.
    if let Some(level) = previous {
        if let Err(e) = c
            .receiver_request(json!({"type": "SET_VOLUME", "volume": {"level": level}}))
            .await
        {
            warnings.push(format!("volume not restored: {e}"));
        }
    }
    if let Some(sid) = session_id {
        let id = c.next_id();
        if let Err(e) = c
            .send(
                RECEIVER,
                NS_RECEIVER,
                &json!({"type": "STOP", "requestId": id, "sessionId": sid}),
            )
            .await
        {
            warnings.push(format!("media receiver not stopped: {e}"));
        }
    }
    if let Err(e) = c
        .send(RECEIVER, NS_CONNECTION, &json!({"type": "CLOSE"}))
        .await
    {
        warnings.push(format!("connection not closed: {e}"));
    }
    (played, warnings)
}

/// SET_VOLUME → LAUNCH → LOAD → play to the end. `session_id` is filled as
/// soon as the app is up so the caller can STOP it even if this fails.
async fn playback<S: AsyncRead + AsyncWrite + Unpin>(
    c: &mut Conn<S>,
    url: &str,
    title: &str,
    volume: Option<u8>,
    session_id: &mut Option<String>,
) -> Result<String, CastError> {
    if let Some(pct) = volume {
        c.receiver_request(
            json!({"type": "SET_VOLUME", "volume": {"level": f64::from(pct) / 100.0}}),
        )
        .await?;
    }
    let launch_id = c.next_id();
    c.send(
        RECEIVER,
        NS_RECEIVER,
        &json!({"type": "LAUNCH", "requestId": launch_id, "appId": DEFAULT_MEDIA_RECEIVER}),
    )
    .await?;
    let launched = c
        .wait_for(
            NS_RECEIVER,
            "the media receiver to launch",
            REPLY_TIMEOUT,
            |v| msg_type(v) == "LAUNCH_ERROR" || transport_id(v).is_some(),
        )
        .await?;
    if msg_type(&launched) == "LAUNCH_ERROR" {
        return Err(CastError::LaunchRefused(
            launched
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        ));
    }
    let transport = transport_id(&launched).expect("checked by wait_for");
    *session_id = launched
        .pointer("/status/applications/0/sessionId")
        .and_then(Value::as_str)
        .map(str::to_string);

    c.send(&transport, NS_CONNECTION, &json!({"type": "CONNECT"}))
        .await?;
    let load_id = c.next_id();
    c.send(
        &transport,
        NS_MEDIA,
        &json!({
            "type": "LOAD",
            "requestId": load_id,
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
    // says it stopped. The receiver's first MEDIA_STATUS after LOAD is
    // empty and the next is a plain IDLE, so "stopped" is only meaningful
    // after "playing".
    let started = c
        .wait_for(NS_MEDIA, "playback to start", REPLY_TIMEOUT, |v| {
            let t = msg_type(v);
            t == "LOAD_FAILED"
                || t == "LOAD_CANCELLED"
                || player_state(v).is_some_and(|(s, _)| s == "PLAYING" || s == "BUFFERING")
        })
        .await?;
    if msg_type(&started) != "MEDIA_STATUS" {
        return Err(CastError::LoadFailed(msg_type(&started).to_string()));
    }
    let done = c
        .wait_for(NS_MEDIA, "playback to finish", PLAYBACK_TIMEOUT, |v| {
            player_state(v).is_some_and(|(s, _)| s == "IDLE")
        })
        .await?;
    let (state, reason) = player_state(&done).unwrap_or_default();
    match reason.as_deref() {
        Some("FINISHED") | None => Ok(match reason {
            Some(r) => format!("{state}/{r}"),
            None => state,
        }),
        Some(other) => Err(CastError::Playback(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

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

    /// How a scripted device answers LAUNCH and LOAD.
    #[derive(Clone, Copy)]
    struct Script {
        refuse_launch: bool,
        load: &'static str, // "finished" | "cancelled" | "load_failed"
    }

    /// The other end of the pipe: a Cast receiver that answers like a Nest
    /// Hub (the unsolicited empty and IDLE statuses included) and logs what
    /// it was sent as `destination:type`.
    async fn fake_device(
        mut s: tokio::io::DuplexStream,
        script: Script,
        log: Arc<Mutex<Vec<String>>>,
    ) {
        let mut pinged = false;
        loop {
            let mut len = [0u8; 4];
            if s.read_exact(&mut len).await.is_err() {
                return;
            }
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            if s.read_exact(&mut body).await.is_err() {
                return;
            }
            let m = decode(&body).unwrap();
            let v: Value = serde_json::from_str(&m.payload).unwrap();
            let t = msg_type(&v).to_string();
            log.lock().unwrap().push(format!("{}:{t}", m.destination));
            if m.namespace == NS_CONNECTION && t == "CLOSE" && m.destination == RECEIVER {
                return;
            }
            let rid = v.get("requestId").cloned().unwrap_or(json!(0));
            let mut replies: Vec<(&str, &str, Value)> = Vec::new();
            let media = |state: &str, reason: Option<&str>| {
                let mut st = json!({"mediaSessionId": 1, "playerState": state});
                if let Some(r) = reason {
                    st["idleReason"] = json!(r);
                }
                json!({"type": "MEDIA_STATUS", "requestId": 0, "status": [st]})
            };
            match (m.namespace.as_str(), t.as_str()) {
                (NS_RECEIVER, "GET_STATUS") => {
                    if !pinged {
                        pinged = true;
                        replies.push((RECEIVER, NS_HEARTBEAT, json!({"type": "PING"})));
                    }
                    replies.push((
                        RECEIVER,
                        NS_RECEIVER,
                        json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"volume": {"level": 0.5}}}),
                    ));
                }
                (NS_RECEIVER, "SET_VOLUME") => replies.push((
                    RECEIVER,
                    NS_RECEIVER,
                    json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {"volume": v["volume"].clone()}}),
                )),
                (NS_RECEIVER, "LAUNCH") if script.refuse_launch => replies.push((
                    RECEIVER,
                    NS_RECEIVER,
                    json!({"type": "LAUNCH_ERROR", "requestId": rid, "reason": "NOT_FOUND"}),
                )),
                (NS_RECEIVER, "LAUNCH") => replies.push((
                    RECEIVER,
                    NS_RECEIVER,
                    json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {
                        "applications": [{"appId": DEFAULT_MEDIA_RECEIVER, "transportId": "t1", "sessionId": "s1"}],
                        "volume": {"level": 0.5}}}),
                )),
                (NS_MEDIA, "LOAD") => {
                    replies.push((
                        "t1",
                        NS_MEDIA,
                        json!({"type": "MEDIA_STATUS", "requestId": 0, "status": []}),
                    ));
                    if script.load == "load_failed" {
                        replies.push((
                            "t1",
                            NS_MEDIA,
                            json!({"type": "LOAD_FAILED", "requestId": rid}),
                        ));
                    } else {
                        replies.push(("t1", NS_MEDIA, media("IDLE", None)));
                        replies.push(("t1", NS_MEDIA, media("BUFFERING", None)));
                        replies.push(("t1", NS_MEDIA, media("PLAYING", None)));
                        let reason = if script.load == "cancelled" {
                            "CANCELLED"
                        } else {
                            "FINISHED"
                        };
                        replies.push(("t1", NS_MEDIA, media("IDLE", Some(reason))));
                    }
                }
                (NS_RECEIVER, "STOP") => replies.push((
                    RECEIVER,
                    NS_RECEIVER,
                    json!({"type": "RECEIVER_STATUS", "requestId": rid, "status": {}}),
                )),
                _ => {}
            }
            // A reply the sender no longer reads (it may have hung up after
            // its last frame) is not this device's problem.
            for (src, ns, payload) in replies {
                let _ = s
                    .write_all(&encode(src, SENDER, ns, &payload.to_string()))
                    .await;
            }
        }
    }

    fn drive(
        script: Script,
        volume: Option<u8>,
    ) -> (Result<String, CastError>, Vec<String>, Vec<String>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let (ours, theirs) = tokio::io::duplex(64 * 1024);
            let log = Arc::new(Mutex::new(Vec::new()));
            let device = tokio::spawn(fake_device(theirs, script, log.clone()));
            let mut c = Conn::new(ours, false);
            let (result, warnings) = session(&mut c, "http://x/a.mp3", "t", volume).await;
            drop(c);
            let _ = device.await;
            let log = log.lock().unwrap().clone();
            (result, warnings, log)
        })
    }

    fn count(log: &[String], entry: &str) -> usize {
        log.iter().filter(|l| *l == entry).count()
    }

    #[test]
    fn a_full_announcement_plays_and_restores_the_device() {
        let (result, warnings, log) = drive(
            Script {
                refuse_launch: false,
                load: "finished",
            },
            Some(30),
        );
        assert_eq!(result, Ok("IDLE/FINISHED".to_string()));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            log.contains(&"receiver-0:PONG".to_string()),
            "heartbeat answered: {log:?}"
        );
        assert_eq!(
            count(&log, "receiver-0:SET_VOLUME"),
            2,
            "set, then restored: {log:?}"
        );
        assert_eq!(count(&log, "t1:LOAD"), 1);
        let stop = log
            .iter()
            .position(|l| l == "receiver-0:STOP")
            .expect("app stopped");
        let restore = log
            .iter()
            .rposition(|l| l == "receiver-0:SET_VOLUME")
            .unwrap();
        assert!(restore < stop, "volume back before the app goes: {log:?}");
        assert_eq!(log.last().map(String::as_str), Some("receiver-0:CLOSE"));
    }

    #[test]
    fn a_failed_load_still_restores_volume_and_stops_the_app() {
        let (result, warnings, log) = drive(
            Script {
                refuse_launch: false,
                load: "load_failed",
            },
            Some(80),
        );
        assert_eq!(result, Err(CastError::LoadFailed("LOAD_FAILED".into())));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(count(&log, "receiver-0:SET_VOLUME"), 2, "{log:?}");
        assert_eq!(count(&log, "receiver-0:STOP"), 1, "{log:?}");
        assert_eq!(log.last().map(String::as_str), Some("receiver-0:CLOSE"));
    }

    #[test]
    fn a_refused_launch_has_no_app_to_stop_but_still_restores_volume() {
        let (result, _, log) = drive(
            Script {
                refuse_launch: true,
                load: "finished",
            },
            Some(50),
        );
        assert_eq!(result, Err(CastError::LaunchRefused("NOT_FOUND".into())));
        assert_eq!(count(&log, "receiver-0:SET_VOLUME"), 2, "{log:?}");
        assert_eq!(count(&log, "receiver-0:STOP"), 0, "{log:?}");
        assert_eq!(log.last().map(String::as_str), Some("receiver-0:CLOSE"));
    }

    #[test]
    fn an_interrupted_announcement_is_not_a_success_and_no_volume_means_no_restore() {
        let (result, _, log) = drive(
            Script {
                refuse_launch: false,
                load: "cancelled",
            },
            None,
        );
        assert_eq!(result, Err(CastError::Playback("CANCELLED".into())));
        assert_eq!(count(&log, "receiver-0:SET_VOLUME"), 0, "{log:?}");
        assert_eq!(count(&log, "receiver-0:STOP"), 1, "{log:?}");
        assert_eq!(CastError::Playback("x".into()).kind(), "playback");
    }
}
