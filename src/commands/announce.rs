use clap::Args;
use pk_cli_core::CliError;
use serde_json::json;

use super::rooms::HomeFlag;
use super::{emit_one, Ctx};
use crate::announce;
use crate::homegraph::{resolve_device, resolve_room};

#[derive(Args, Debug, Clone)]
pub struct AnnounceArgs {
    /// What to say.
    pub message: String,
    /// Only the speakers/displays in this room (id or name).
    #[arg(long, value_name = "ROOM", conflicts_with = "device")]
    pub room: Option<String>,
    /// Only this device (id or name).
    #[arg(long, value_name = "DEVICE")]
    pub device: Option<String>,
    #[command(flatten)]
    pub home: HomeFlag,
    /// Experiment knob: skip the UpdateToken handshake.
    #[arg(long, hide = true)]
    pub no_handshake: bool,
    /// Experiment knob: handshake target form (`bare` structure id, or `structure`).
    #[arg(long, hide = true, default_value = "bare")]
    pub handshake_target: String,
    /// Experiment knob: context enum value.
    #[arg(long, hide = true, default_value_t = 2)]
    pub ctx_enum: u64,
}

pub fn run(ctx: &Ctx, args: &AnnounceArgs) -> Result<(), CliError> {
    let message = args.message.trim();
    if message.is_empty() {
        return Err(CliError::Usage("nothing to announce".into()));
    }
    if message.chars().count() > 256 {
        return Err(CliError::Usage(
            "keep announcements under 256 characters".into(),
        ));
    }
    let graph = ctx.graph()?;
    let homes = ctx.homes(&graph, args.home.home.as_deref())?;
    if homes.len() != 1 {
        return Err(CliError::Usage(
            "pick one home with --home (or `config set home`) to announce in".into(),
        ));
    }
    let h = homes[0];
    let (target, label) = if let Some(d) = &args.device {
        let all: Vec<_> = h.devices.iter().collect();
        let dev = resolve_device(&all, d)?;
        (format!("device@{}", dev.id), dev.name.clone())
    } else if let Some(r) = &args.room {
        let rooms: Vec<_> = h.rooms.iter().collect();
        let room = resolve_room(&rooms, r)?;
        (format!("room@{}", room.id), room.name.clone())
    } else {
        (
            format!("structure@{}", h.id),
            format!("everywhere in {}", h.name),
        )
    };

    let session = ctx.session()?;
    let client = ctx.http()?;
    let bearer = session.bearer_for_scope(&client, announce::MESH_SCOPE)?;
    let ua = format!("{}/{}", crate::BIN, env!("CARGO_PKG_VERSION"));
    if !args.no_handshake {
        let hs_target = if args.handshake_target == "structure" {
            format!("structure@{}", h.id)
        } else {
            h.id.clone()
        };
        if ctx.verbose {
            eprintln!(
                "mesh: UpdateToken handshake → {hs_target} (ctx enum {})",
                args.ctx_enum
            );
        }
        announce::call(
            &bearer,
            &ua,
            &announce::update_token_request(&h.id, &hs_target, &bearer, args.ctx_enum),
        )?;
    }
    if ctx.verbose {
        eprintln!("mesh: broadcast to {target} (ctx enum {})", args.ctx_enum);
    }
    let reply = announce::call(
        &bearer,
        &ua,
        &announce::broadcast_request(&h.id, &target, message, args.ctx_enum),
    )?;
    emit_one(
        ctx.json,
        "announce",
        json!({
            "message": message,
            "target": label,
            "target_id": target,
            "http_status": reply.http_status,
            "response_bytes": reply.messages.len(),
            "sent": true,
        }),
    );
    Ok(())
}
