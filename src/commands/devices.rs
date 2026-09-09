use clap::{Args, Subcommand};
use pk_cli_core::CliError;
use serde_json::{json, Value};

use super::rooms::HomeFlag;
use super::{confirm, device_row, emit_list, emit_one, require_confirmable, require_layout, Ctx};
use crate::homegraph::{resolve_device, resolve_room, Device, Home};
use crate::spaces;

#[derive(Args, Debug, Clone)]
pub struct ListArgs {
    #[command(flatten)]
    pub home: HomeFlag,
    /// Only devices in this room (id or name).
    #[arg(long, value_name = "ROOM")]
    pub room: Option<String>,
    /// Only devices from this partner integration (agent id, see `agents`).
    #[arg(long, value_name = "AGENT")]
    pub agent: Option<String>,
    /// Only devices in the home but assigned to no room.
    #[arg(long)]
    pub unassigned: bool,
    /// Only devices linked to the account but placed in no home at all
    /// ("Linked to you" in the Home app). These answer to no room command.
    #[arg(long)]
    pub unplaced: bool,
    /// Only devices whose name contains this text (case-insensitive).
    #[arg(long, value_name = "TEXT")]
    pub name: Option<String>,
    /// Add an `online` column: whether the vendor currently reports the
    /// device reachable (one extra round-trip).
    #[arg(long)]
    pub status: bool,
    /// Only devices reported offline (implies --status). Stale hardware from
    /// a previous home shows up here.
    #[arg(long)]
    pub offline: bool,
}

#[derive(Subcommand, Debug)]
pub enum DevicesCmd {
    /// List devices with their rooms (device-list/v1).
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// One device in full (device/v1).
    Get {
        /// Device id, exact name, or unique partial name.
        device: String,
        #[command(flatten)]
        home: HomeFlag,
    },
    /// Partner integrations (agents) that own devices (agent-list/v1).
    Agents(HomeFlag),
    /// Move a device into a room (device-move/v1). Prompts unless --force.
    Move {
        /// Device id, exact name, or unique partial name.
        device: String,
        /// Target room (id or name).
        #[arg(long, value_name = "ROOM")]
        room: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
    /// Remove a device from Google Home (device-remove/v1). Prompts unless
    /// --force. A vendor that still lists the device will bring it back as
    /// unplaced on its next sync; delete it in the vendor app too.
    Remove {
        /// Device id, exact name, or unique partial name (in the home or unplaced).
        device: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
    /// Rename a device as Google shows it (device-rename/v1). Prompts unless
    /// --force. The vendor app keeps its own name.
    Rename {
        /// Device id, exact name, or unique partial name.
        device: String,
        /// New name.
        name: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
    /// Ask Google to re-sync every linked vendor ("sync my devices").
    Sync,
    /// Add a device that is linked to the account but in no home to the
    /// home, optionally straight into a room (device-place/v1).
    Place {
        /// Device id, exact name, or unique partial name (see `list --unplaced`).
        device: String,
        /// Room to put it in once it's in the home (id or name).
        #[arg(long, value_name = "ROOM")]
        room: Option<String>,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
}

/// The home a device belongs to, from the set the command is acting on.
fn home_of<'a>(homes: &[&'a Home], device_id: &str) -> &'a Home {
    homes
        .iter()
        .find(|h| h.devices.iter().any(|d| d.id == device_id))
        .expect("device came from one of these homes")
}

/// Read the room back and require the device to be listed in it: a 200 from
/// a write is not proof the graph changed.
fn verify_in_room(
    ctx: &Ctx,
    home_id: &str,
    room_id: &str,
    device_id: &str,
) -> Result<(), CliError> {
    let raw = ctx.write(
        spaces::SPACES,
        spaces::GET_SPACE,
        &spaces::get_space(home_id, room_id),
    )?;
    match spaces::parse_space(&raw) {
        Some(r) if r.device_ids.iter().any(|d| d == device_id) => Ok(()),
        _ => Err(CliError::Upstream(
            "Google accepted the write but the room does not list the device on read-back".into(),
        )),
    }
}

pub fn run(ctx: &Ctx, cmd: &DevicesCmd) -> Result<(), CliError> {
    match cmd {
        DevicesCmd::List(args) => {
            if (args.unassigned || args.unplaced) && args.room.is_some() {
                return Err(CliError::Usage(
                    "--unassigned/--unplaced and --room are mutually exclusive".into(),
                ));
            }
            if args.unassigned && args.unplaced {
                return Err(CliError::Usage(
                    "--unassigned (in the home, no room) and --unplaced (in no home) are mutually exclusive".into(),
                ));
            }
            let graph = ctx.graph()?;
            let mut items: Vec<Value> = Vec::new();
            let want_name = args.name.as_deref().map(str::to_lowercase);
            for h in ctx.homes(&graph, args.home.home.as_deref())? {
                if args.unplaced {
                    for d in ctx.unplaced(&h.id)? {
                        if let Some(a) = &args.agent {
                            if d.agent_id.as_deref() != Some(a) {
                                continue;
                            }
                        }
                        if let Some(n) = &want_name {
                            if !d.name.to_lowercase().contains(n) {
                                continue;
                            }
                        }
                        items.push(device_row(h, &d));
                    }
                    continue;
                }
                let room_id = match &args.room {
                    Some(q) => {
                        let rooms: Vec<_> = h.rooms.iter().collect();
                        Some(resolve_room(&rooms, q)?.id.clone())
                    }
                    None => None,
                };
                for d in &h.devices {
                    if let Some(rid) = &room_id {
                        if d.room_id.as_deref() != Some(rid) {
                            continue;
                        }
                    }
                    if args.unassigned && d.room.is_some() {
                        continue;
                    }
                    if let Some(a) = &args.agent {
                        if d.agent_id.as_deref() != Some(a) {
                            continue;
                        }
                    }
                    if let Some(n) = &want_name {
                        if !d.name.to_lowercase().contains(n) {
                            continue;
                        }
                    }
                    items.push(device_row(h, d));
                }
            }
            let mut columns = vec![
                "name",
                "room",
                "type",
                "model",
                "agent_id",
                "partner_device_id",
                "id",
            ];
            if args.status || args.offline {
                let ids: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect();
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                let online = ctx.online(&refs)?;
                for v in &mut items {
                    let id = v
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if let (Some(state), Value::Object(m)) = (online.get(&id), v) {
                        m.insert("online".into(), Value::Bool(*state));
                    }
                }
                if args.offline {
                    items.retain(|v| v.get("online") == Some(&Value::Bool(false)));
                }
                columns.insert(1, "online");
            }
            emit_list(ctx.json, "device", items, &columns);
            Ok(())
        }
        DevicesCmd::Get { device, home } => {
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let all: Vec<&Device> = homes.iter().flat_map(|h| h.devices.iter()).collect();
            let d = resolve_device(&all, device)?;
            let h = homes
                .iter()
                .find(|h| h.devices.iter().any(|x| x.id == d.id))
                .expect("device came from one of these homes");
            emit_one(ctx.json, "device", device_row(h, d));
            Ok(())
        }
        DevicesCmd::Move {
            device,
            room,
            home,
            force,
        } => {
            require_confirmable(*force, ctx.interactive, "moving a device between rooms")?;
            require_layout()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let all: Vec<&Device> = homes.iter().flat_map(|h| h.devices.iter()).collect();
            let d = resolve_device(&all, device)?;
            let h = home_of(&homes, &d.id);
            let rooms: Vec<_> = h.rooms.iter().collect();
            let target = resolve_room(&rooms, room)?;
            let from = d.room.clone();
            if d.room_id.as_deref() == Some(&target.id) {
                emit_one(
                    ctx.json,
                    "device-move",
                    json!({"device_id": d.id, "name": d.name, "room": target.name, "changed": false}),
                );
                return Ok(());
            }
            confirm(
                *force,
                &format!(
                    "Move \"{}\" from {} to {}?",
                    d.name,
                    from.as_deref().unwrap_or("(no room)"),
                    target.name
                ),
            )?;
            ctx.write(
                spaces::SPACES,
                spaces::BATCH_MODIFY_SPACES_DEVICES,
                &spaces::move_device(&target.id, &d.id),
            )?;
            verify_in_room(ctx, &h.id, &target.id, &d.id)?;
            emit_one(
                ctx.json,
                "device-move",
                json!({
                    "device_id": d.id,
                    "name": d.name,
                    "from_room": from,
                    "room": target.name,
                    "room_id": target.id,
                    "changed": true,
                }),
            );
            Ok(())
        }
        DevicesCmd::Remove {
            device,
            home,
            force,
        } => {
            require_confirmable(
                *force,
                ctx.interactive,
                "removing a device from Google Home",
            )?;
            require_layout()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let mut pool: Vec<Device> = homes
                .iter()
                .flat_map(|h| h.devices.iter().cloned())
                .collect();
            for h in &homes {
                pool.extend(ctx.unplaced(&h.id)?);
            }
            let refs: Vec<&Device> = pool.iter().collect();
            let d = resolve_device(&refs, device)?.clone();
            confirm(
                *force,
                &format!(
                    "Remove \"{}\" ({}) from Google Home?",
                    d.name,
                    d.room.as_deref().unwrap_or("no room")
                ),
            )?;
            ctx.write(
                spaces::HOME_DEVICES,
                spaces::DELETE_DEVICE,
                &spaces::delete_device(&d.id),
            )?;
            let after = ctx.graph()?;
            let still_home = after
                .homes
                .iter()
                .any(|h| h.devices.iter().any(|x| x.id == d.id));
            let mut still_unplaced = false;
            for h in &after.homes {
                if ctx.unplaced(&h.id)?.iter().any(|x| x.id == d.id) {
                    still_unplaced = true;
                }
            }
            if still_home || still_unplaced {
                return Err(CliError::Upstream(
                    "Google accepted the delete but still lists the device on read-back".into(),
                ));
            }
            emit_one(
                ctx.json,
                "device-remove",
                json!({"device_id": d.id, "name": d.name, "agent_id": d.agent_id, "partner_device_id": d.partner_device_id, "removed": true}),
            );
            Ok(())
        }
        DevicesCmd::Rename {
            device,
            name,
            home,
            force,
        } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(CliError::Usage("a new name is required".into()));
            }
            require_confirmable(*force, ctx.interactive, "renaming a device")?;
            require_layout()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let mut pool: Vec<Device> = homes
                .iter()
                .flat_map(|h| h.devices.iter().cloned())
                .collect();
            for h in &homes {
                pool.extend(ctx.unplaced(&h.id)?);
            }
            let refs: Vec<&Device> = pool.iter().collect();
            let d = resolve_device(&refs, device)?.clone();
            if d.name == name {
                emit_one(
                    ctx.json,
                    "device-rename",
                    json!({"device_id": d.id, "name": d.name, "changed": false}),
                );
                return Ok(());
            }
            confirm(*force, &format!("Rename \"{}\" to \"{name}\"?", d.name))?;
            let raw = ctx.write(
                spaces::HOME_DEVICES,
                spaces::UPDATE_DEVICE_SETTINGS,
                &spaces::rename_device(&d.id, &name),
            )?;
            let echoed = raw
                .get(1)
                .and_then(crate::homegraph::parse_device_value)
                .map(|x| x.name);
            let after_name = match echoed {
                Some(n) => n,
                None => {
                    let after = ctx.graph()?;
                    after
                        .homes
                        .iter()
                        .flat_map(|h| h.devices.iter())
                        .find(|x| x.id == d.id)
                        .map(|x| x.name.clone())
                        .unwrap_or_default()
                }
            };
            if after_name != name {
                return Err(CliError::Upstream(format!(
                    "Google accepted the rename but reports the name as `{after_name}` on read-back"
                )));
            }
            emit_one(
                ctx.json,
                "device-rename",
                json!({"device_id": d.id, "previous_name": d.name, "name": name, "changed": true}),
            );
            Ok(())
        }
        DevicesCmd::Sync => {
            ctx.write(
                spaces::HOME_DEVICES,
                spaces::SYNC_DEVICES,
                &spaces::sync_devices(),
            )?;
            emit_one(ctx.json, "device-sync", json!({"requested": true}));
            Ok(())
        }
        DevicesCmd::Place {
            device,
            room,
            home,
            force,
        } => {
            require_confirmable(*force, ctx.interactive, "adding a device to the home")?;
            require_layout()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            if homes.len() != 1 {
                return Err(CliError::Usage(
                    "pick one home with --home (or `config set home`) to place a device into"
                        .into(),
                ));
            }
            let h = homes[0];
            let outside = ctx.unplaced(&h.id)?;
            let candidates: Vec<&Device> = outside.iter().collect();
            let d = resolve_device(&candidates, device)?;
            let target = match room {
                Some(q) => {
                    let rooms: Vec<_> = h.rooms.iter().collect();
                    Some(resolve_room(&rooms, q)?)
                }
                None => None,
            };
            confirm(
                *force,
                &format!(
                    "Add \"{}\" to {}{}?",
                    d.name,
                    h.name,
                    target
                        .map(|r| format!(" in room {}", r.name))
                        .unwrap_or_default()
                ),
            )?;
            ctx.write(
                spaces::STRUCTURES,
                spaces::BATCH_MODIFY_STRUCTURES_DEVICES,
                &spaces::place_device(&h.id, &d.id),
            )?;
            if let Some(r) = target {
                ctx.write(
                    spaces::SPACES,
                    spaces::BATCH_MODIFY_SPACES_DEVICES,
                    &spaces::move_device(&r.id, &d.id),
                )?;
                verify_in_room(ctx, &h.id, &r.id, &d.id)?;
            } else {
                let after = ctx.graph()?;
                let placed = after
                    .homes
                    .iter()
                    .any(|x| x.id == h.id && x.devices.iter().any(|y| y.id == d.id));
                if !placed {
                    return Err(CliError::Upstream(
                        "Google accepted the write but the home does not list the device on read-back".into(),
                    ));
                }
            }
            emit_one(
                ctx.json,
                "device-place",
                json!({
                    "device_id": d.id,
                    "name": d.name,
                    "home": h.name,
                    "room": target.map(|r| r.name.clone()),
                    "changed": true,
                }),
            );
            Ok(())
        }
        DevicesCmd::Agents(flag) => {
            let graph = ctx.graph()?;
            let items = super::agent_counts(ctx, &graph, flag.home.as_deref())?;
            emit_list(ctx.json, "agent", items, &["agent_id", "label", "devices"]);
            Ok(())
        }
    }
}
