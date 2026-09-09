//! Raw Foyer passthrough: `ghome api POST StructuresService/GetHomeGraph
//! --data '[]'`. Bodies are positional arrays, so `--data` must be a JSON
//! array (or a full URL for endpoints outside the RPC base).

use pk_cli_core::{output, CliError};
use pk_cli_http::ApiArgs;
use serde_json::Value;

use super::Ctx;
use crate::foyer::{self, Foyer};

/// Validate before touching the keychain: Foyer only speaks POST, and only
/// takes array bodies.
pub fn validate(args: &ApiArgs) -> Result<Value, CliError> {
    let method = args.parsed_method()?;
    if method != reqwest::Method::POST {
        return Err(CliError::Usage(
            "foyer RPCs are POST-only; use `api POST <Service>/<Method> --data '[...]'`".into(),
        ));
    }
    match args.parsed_body()? {
        Some(v @ Value::Array(_)) => Ok(v),
        Some(_) => Err(CliError::Usage(
            "--data must be a JSON array (foyer bodies are positional protobuf arrays)".into(),
        )),
        None => Ok(Value::Array(vec![])),
    }
}

pub fn run(ctx: &Ctx, args: &ApiArgs, body: Value) -> Result<(), CliError> {
    let mut session = ctx.session()?;
    let mut client = Foyer::new(ctx.http()?, &mut session, ctx.creds, ctx.verbose);
    let v = client.call(&foyer::url_for(&args.path), &body)?;
    output::json(&v);
    Ok(())
}
