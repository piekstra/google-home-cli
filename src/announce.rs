//! Broadcast a spoken message to the home's speakers and displays, the way
//! the Home app does — through the Home-platform mesh service, natively over
//! gRPC (the JSON gateway cannot type-resolve the command payload).
//!
//! Recipe recovered from the Home app and Play services (docs/api.md):
//! 1. Mint a Bearer for `home.platform.selected.devices` (the mesh scope).
//! 2. `SendCommands` an `OAuthSessionTrait.UpdateToken{1: token}` to the
//!    structure — the in-band session handshake.
//! 3. `SendCommands` the `AssistantBroadcastTrait.BroadcastCommand{1: msg}`
//!    to `structure@<id>`, `room@<id>` or `device@<id>`.

use pk_cli_core::CliError;

pub const MESH_URL: &str = "https://homeplatformmesh-pa.googleapis.com/google.internal.home.platform.mesh.interaction.v1.MeshInteractionService/SendCommands";
pub const MESH_SCOPE: &str =
    "oauth2:https://www.googleapis.com/auth/home.platform.selected.devices";
const BROADCAST: &str = "home.platform.traits.AssistantBroadcastTrait.BroadcastCommand";
const UPDATE_TOKEN: &str = "home.internal.traits.OAuthSessionTrait.UpdateToken";

// ---- a minimal protobuf writer: varints, strings, nested messages ----

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

fn bytes_field(field: u32, payload: &[u8], out: &mut Vec<u8>) {
    varint(u64::from(field << 3 | 2), out);
    varint(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

fn str_field(field: u32, s: &str, out: &mut Vec<u8>) {
    bytes_field(field, s.as_bytes(), out);
}

fn varint_field(field: u32, n: u64, out: &mut Vec<u8>) {
    varint(u64::from(field << 3), out);
    varint(n, out);
}

/// `Any{1: type_url, 2: value}` for a one-string-field command message.
fn any_of_string_command(name: &str, value: &str) -> Vec<u8> {
    let mut inner = Vec::new();
    str_field(1, value, &mut inner);
    let mut any = Vec::new();
    str_field(1, &format!("type.googleapis.com/{name}"), &mut any);
    bytes_field(2, &inner, &mut any);
    any
}

/// `SendCommandsRequest{1: {1: structure_id, 2: enum}, 2: {1: [{1: target, 2: {1: name, 5: Any}}]}}`.
pub fn send_commands(
    structure_id: &str,
    target: &str,
    command: &str,
    any: &[u8],
    ctx_enum: u64,
) -> Vec<u8> {
    let mut ctx = Vec::new();
    str_field(1, structure_id, &mut ctx);
    varint_field(2, ctx_enum, &mut ctx);

    let mut cmd = Vec::new();
    str_field(1, command, &mut cmd);
    bytes_field(5, any, &mut cmd);

    let mut entry = Vec::new();
    str_field(1, target, &mut entry);
    bytes_field(2, &cmd, &mut entry);

    let mut list = Vec::new();
    bytes_field(1, &entry, &mut list);

    let mut req = Vec::new();
    bytes_field(1, &ctx, &mut req);
    bytes_field(2, &list, &mut req);
    req
}

pub fn update_token_request(
    structure_id: &str,
    target: &str,
    token: &str,
    ctx_enum: u64,
) -> Vec<u8> {
    send_commands(
        structure_id,
        target,
        UPDATE_TOKEN,
        &any_of_string_command(UPDATE_TOKEN, token),
        ctx_enum,
    )
}

pub fn broadcast_request(
    structure_id: &str,
    target: &str,
    message: &str,
    ctx_enum: u64,
) -> Vec<u8> {
    send_commands(
        structure_id,
        target,
        BROADCAST,
        &any_of_string_command(BROADCAST, message),
        ctx_enum,
    )
}

/// One gRPC call, mapped onto the family exit codes.
pub fn call(bearer: &str, user_agent: &str, body: &[u8]) -> Result<crate::grpc::Reply, CliError> {
    crate::grpc::unary(MESH_URL, bearer, user_agent, body)?.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_matches_hand_built_bytes() {
        // BroadcastCommand{1: "hi"} = 0A 02 68 69
        let any = any_of_string_command("x.Cmd", "hi");
        assert!(any.ends_with(&[0x12, 0x04, 0x0A, 0x02, b'h', b'i']));
        assert!(any.starts_with(&[0x0A, 0x19]));
        let mut v = Vec::new();
        varint(300, &mut v);
        assert_eq!(v, vec![0xAC, 0x02]);
        let req = broadcast_request("s", "structure@s", "hi", 2);
        assert_eq!(req[0], 0x0A); // field 1, length-delimited
        assert!(req.windows(11).any(|w| w == b"structure@s"));
    }
}
