use clap::Subcommand;
use pk_cli_core::CliError;
use serde_json::json;

use super::{emit_list, emit_one, Ctx};

#[derive(Subcommand, Debug)]
pub enum HomesCmd {
    /// List the homes (structures) on the account (home-list/v1).
    #[command(visible_alias = "ls")]
    List,
    /// One home with its rooms and linked users (home/v1).
    Get {
        /// Home id or name.
        home: String,
    },
}

pub fn run(ctx: &Ctx, cmd: &HomesCmd) -> Result<(), CliError> {
    match cmd {
        HomesCmd::List => {
            let graph = ctx.graph()?;
            let items = graph
                .homes
                .iter()
                .map(|h| {
                    json!({
                        "id": h.id,
                        "name": h.name,
                        "rooms": h.rooms.len(),
                        "devices": h.devices.len(),
                        "timezone": h.timezone,
                        "linked_users": h.linked_users,
                    })
                })
                .collect();
            emit_list(
                ctx.json,
                "home",
                items,
                &["name", "rooms", "devices", "timezone", "id"],
            );
            Ok(())
        }
        HomesCmd::Get { home } => {
            let graph = ctx.graph()?;
            let h = graph.select_home(Some(home))?[0];
            let rooms: Vec<_> = h
                .rooms
                .iter()
                .map(|r| json!({"id": r.id, "name": r.name, "kind": r.kind, "devices": r.device_ids.len()}))
                .collect();
            emit_one(
                ctx.json,
                "home",
                json!({
                    "id": h.id,
                    "name": h.name,
                    "timezone": h.timezone,
                    "linked_users": h.linked_users,
                    "device_count": h.devices.len(),
                    "rooms": rooms,
                }),
            );
            Ok(())
        }
    }
}
