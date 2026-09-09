//! Raw Foyer passthrough: `ghome api POST StructuresService/GetHomeGraph
//! --data '[]'`. Bodies are positional arrays, so `--data` must be a JSON
//! array (or a full URL for endpoints outside the RPC base).

use clap::Args;
use pk_cli_core::{output, CliError};
use pk_cli_http::ApiArgs;
use serde_json::{json, Value};

use super::Ctx;
use crate::b64;
use crate::foyer::{self, Foyer};

#[derive(Args, Debug, Clone)]
pub struct RawArgs {
    #[command(flatten)]
    pub api: ApiArgs,
    /// Send an already-serialized protobuf body (base64) as
    /// `application/x-protobuf` instead of positional JSON, and print the
    /// response bytes as base64 (api-binary/v1). For RPCs whose payloads the
    /// JSON gateway can't type-resolve.
    #[arg(long, value_name = "BASE64", conflicts_with = "data")]
    pub proto: Option<String>,
    /// With --proto: frame it as gRPC-web (`application/grpc-web+proto`) and
    /// unwrap the response frames; the trailer's grpc-status decides success.
    #[arg(long, requires = "proto", conflicts_with = "grpc")]
    pub grpc_web: bool,
    /// With --proto: native gRPC over HTTP/2 (`application/grpc`) at the
    /// plain `/<package>.<Service>/<Method>` path. Trailers are not visible,
    /// so only the body frames are reported.
    #[arg(long, requires = "proto")]
    pub grpc: bool,
}

pub enum Body {
    Json(Value),
    Proto(Vec<u8>),
}

/// Validate before touching the keychain: Foyer only speaks POST, and only
/// takes array bodies.
pub fn validate(args: &RawArgs) -> Result<Body, CliError> {
    let method = args.api.parsed_method()?;
    if method != reqwest::Method::POST {
        return Err(CliError::Usage(
            "foyer RPCs are POST-only; use `api POST <Service>/<Method> --data '[...]'`".into(),
        ));
    }
    if let Some(p) = &args.proto {
        return b64::decode(p.trim())
            .map(Body::Proto)
            .ok_or_else(|| CliError::Usage("--proto is not valid base64".into()));
    }
    match args.api.parsed_body()? {
        Some(v @ Value::Array(_)) => Ok(Body::Json(v)),
        Some(_) => Err(CliError::Usage(
            "--data must be a JSON array (foyer bodies are positional protobuf arrays)".into(),
        )),
        None => Ok(Body::Json(Value::Array(vec![]))),
    }
}

pub fn run(ctx: &Ctx, args: &RawArgs, body: Body) -> Result<(), CliError> {
    let mut session = ctx.session()?;
    let url = foyer::url_for(&args.api.path);
    if let (Body::Proto(bytes), true) = (&body, args.grpc) {
        let bearer = session.bearer(&ctx.http()?, ctx.creds)?;
        let ua = format!("{}/{}", crate::BIN, env!("CARGO_PKG_VERSION"));
        if ctx.verbose {
            eprintln!(
                "POST {url} ({} bytes, application/grpc over h2)",
                bytes.len()
            );
        }
        let reply = crate::grpc::unary(&url, &bearer, &ua, bytes)?;
        let mut dto = json!({
            "schema": "api-binary/v1",
            "status": reply.http_status,
            "bytes": reply.messages.len(),
            "body_base64": b64::encode(&reply.messages),
        });
        if let Some(code) = reply.grpc_status {
            dto["grpc_status"] = json!(code);
            dto["grpc_message"] = json!(reply.grpc_message);
        }
        output::json(&dto);
        return Ok(());
    }
    let mut client = Foyer::new(ctx.http()?, &mut session, ctx.creds, ctx.verbose);
    match body {
        Body::Json(v) => {
            let out = client.call(&url, &v)?;
            output::json(&out);
        }
        Body::Proto(bytes) => {
            let mode = if args.grpc_web {
                foyer::BinaryMode::GrpcWeb
            } else {
                foyer::BinaryMode::Proto
            };
            let (status, out) = client.call_binary(&url, &bytes, mode)?;
            let mut dto = json!({
                "schema": "api-binary/v1",
                "status": status,
                "bytes": out.len(),
                "body_base64": b64::encode(&out),
            });
            if let Some((code, msg)) = client.last_grpc_status() {
                dto["grpc_status"] = json!(code);
                dto["grpc_message"] = json!(msg);
            }
            output::json(&dto);
        }
    }
    Ok(())
}
