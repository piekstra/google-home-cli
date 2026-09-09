//! A unary gRPC call over HTTP/2 with the trailers read back. reqwest
//! negotiates h2 but discards trailers, and gRPC reports its status there,
//! so a failed call looks like `200` with an empty body. This client is the
//! minimum needed to see `grpc-status`/`grpc-message`: TLS with ALPN `h2`,
//! one stream, framed body, headers + data + trailers.

use std::sync::Arc;

use bytes::Bytes;
use pk_cli_core::CliError;

pub struct Reply {
    pub http_status: u16,
    pub grpc_status: Option<i32>,
    pub grpc_message: String,
    /// Concatenated message frames (5-byte gRPC frame headers stripped).
    pub messages: Vec<u8>,
}

impl Reply {
    /// Map onto the family exit codes: 0 is success, 16/7 auth, 5 not found,
    /// anything else upstream. The raw passthrough reports the status
    /// instead of failing, so callers that need an exit code use this.
    #[allow(dead_code)]
    pub fn into_result(self) -> Result<Reply, CliError> {
        match self.grpc_status {
            None | Some(0) => Ok(self),
            Some(code) => {
                let msg = format!("grpc status {code}: {}", self.grpc_message);
                Err(match code {
                    16 | 7 => CliError::Auth(msg),
                    5 => CliError::NotFound(msg),
                    _ => CliError::Upstream(msg),
                })
            }
        }
    }
}

fn split_url(url: &str) -> Result<(String, String), CliError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| CliError::Usage("gRPC needs an https:// URL".into()))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    Ok((host.to_string(), format!("/{path}")))
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(body.len() + 5);
    f.push(0);
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(body);
    f
}

fn unframe(bytes: &[u8]) -> Vec<u8> {
    let mut i = 0;
    let mut out = Vec::new();
    while i + 5 <= bytes.len() {
        let len =
            u32::from_be_bytes([bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]]) as usize;
        let end = (i + 5 + len).min(bytes.len());
        if bytes[i] & 0x80 == 0 {
            out.extend_from_slice(&bytes[i + 5..end]);
        }
        i = end;
    }
    out
}

/// POST `body` (an already-serialized request message) to `url` as
/// `application/grpc`, returning status, message bytes and trailer status.
pub fn unary(url: &str, bearer: &str, user_agent: &str, body: &[u8]) -> Result<Reply, CliError> {
    let (host, path) = split_url(url)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Other(format!("tokio runtime: {e}")))?;
    let framed = frame(body);
    rt.block_on(async move {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| CliError::Other(format!("tls config: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tcp = tokio::net::TcpStream::connect((host.as_str(), 443))
            .await
            .map_err(|e| CliError::Upstream(format!("connect {host}: {e}")))?;
        let server_name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|e| CliError::Usage(format!("bad host {host}: {e}")))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| CliError::Upstream(format!("tls {host}: {e}")))?;
        let (mut client, conn) = h2::client::handshake(tls)
            .await
            .map_err(|e| CliError::Upstream(format!("h2 handshake: {e}")))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let req = http::Request::builder()
            .method(http::Method::POST)
            .uri(format!("https://{host}{path}"))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .header("grpc-accept-encoding", "identity")
            .header("authorization", format!("Bearer {bearer}"))
            .header("user-agent", user_agent)
            .header("x-user-agent", "grpc-web-javascript/0.1")
            .body(())
            .map_err(|e| CliError::Other(format!("request: {e}")))?;
        let (response, mut stream) = client
            .send_request(req, false)
            .map_err(|e| CliError::Upstream(format!("h2 send: {e}")))?;
        stream
            .send_data(Bytes::from(framed), true)
            .map_err(|e| CliError::Upstream(format!("h2 body: {e}")))?;
        let resp = response
            .await
            .map_err(|e| CliError::Upstream(format!("h2 response: {e}")))?;
        let http_status = resp.status().as_u16();
        let header_status = grpc_status_of(resp.headers());
        let mut body = resp.into_body();
        let mut raw = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|e| CliError::Upstream(format!("h2 data: {e}")))?;
            let _ = body.flow_control().release_capacity(chunk.len());
            raw.extend_from_slice(&chunk);
        }
        let trailer_status = body
            .trailers()
            .await
            .map_err(|e| CliError::Upstream(format!("h2 trailers: {e}")))?
            .as_ref()
            .and_then(grpc_status_of);
        let (grpc_status, grpc_message) = match trailer_status.or(header_status) {
            Some((c, m)) => (Some(c), m),
            None => (None, String::new()),
        };
        Ok(Reply {
            http_status,
            grpc_status,
            grpc_message,
            messages: unframe(&raw),
        })
    })
}

fn grpc_status_of(h: &http::HeaderMap) -> Option<(i32, String)> {
    let code = h.get("grpc-status")?.to_str().ok()?.trim().parse().ok()?;
    let msg = h
        .get("grpc-message")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .unwrap_or_default();
    Some((code, msg))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_urls_split() {
        assert_eq!(unframe(&frame(b"abc")), b"abc");
        assert_eq!(
            split_url("https://h.example/pkg.Svc/M").unwrap(),
            ("h.example".to_string(), "/pkg.Svc/M".to_string())
        );
        assert!(split_url("http://x").is_err());
        assert_eq!(percent_decode("a%20b"), "a b");
    }
}
