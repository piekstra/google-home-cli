use clap::Subcommand;
use pk_cli_core::CliError;
use serde_json::json;

use super::{confirm, emit_list, emit_one, require_confirmable, require_layout, Ctx};
use crate::spaces;

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
    /// Rename a home (home-rename/v1). Prompts unless --force; everyone in
    /// the home sees the new name.
    Rename {
        /// Home id or current name.
        home: String,
        /// New name.
        name: String,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
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
        HomesCmd::Rename { home, name, force } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(CliError::Usage("a new name is required".into()));
            }
            require_confirmable(*force, ctx.interactive, "renaming a home")?;
            require_layout()?;
            let graph = ctx.graph()?;
            let h = graph.select_home(Some(home))?[0];
            if h.name == name {
                emit_one(
                    ctx.json,
                    "home-rename",
                    json!({"id": h.id, "name": h.name, "changed": false}),
                );
                return Ok(());
            }
            confirm(
                *force,
                &format!("Rename home \"{}\" to \"{name}\"?", h.name),
            )?;
            ctx.write(
                spaces::STRUCTURES,
                spaces::UPDATE_STRUCTURE_V2,
                &spaces::rename_structure(&h.id, &name),
            )?;
            let after = ctx.graph()?;
            let now = after
                .homes
                .iter()
                .find(|x| x.id == h.id)
                .map(|x| x.name.clone())
                .unwrap_or_default();
            if now != name {
                return Err(CliError::Upstream(format!(
                    "Google accepted the rename but reports the home as `{now}` on read-back"
                )));
            }
            emit_one(
                ctx.json,
                "home-rename",
                json!({"id": h.id, "previous_name": h.name, "name": name, "changed": true}),
            );
            Ok(())
        }
    }
}
