use clap::{Args, Subcommand};
use pk_cli_core::{output, CliError};
use serde_json::{json, Value};

use super::{confirm, device_row, emit_list, emit_one, require_confirmable, require_layout, Ctx};
use crate::homegraph::{resolve_room, Room};
use crate::spaces;

#[derive(Args, Debug, Clone)]
pub struct HomeFlag {
    /// Home (structure) id or name; defaults to `config home`, else every home.
    #[arg(long, value_name = "HOME")]
    pub home: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum RoomsCmd {
    /// List rooms with their device counts (room-list/v1).
    #[command(visible_alias = "ls")]
    List(HomeFlag),
    /// One room and the devices in it (room/v1).
    Get {
        /// Room id or name.
        room: String,
        #[command(flatten)]
        home: HomeFlag,
    },
    /// Google's room categories, for naming new rooms (room-type-list/v1).
    Types,
    /// Control every light in a room at once (room-set/v1): "turn off the
    /// office lights". Add --all to include plugs, switches and speakers.
    Set {
        /// Room id or name.
        room: String,
        #[command(flatten)]
        change: super::devices::ChangeArgs,
        /// Include every device that supports the change, not just lights.
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        home: HomeFlag,
    },
    /// Create a room (room/v1). Prompts unless --force.
    Create {
        /// Display name, e.g. "Loft".
        name: String,
        /// Category code from `rooms types`, e.g. OFFICE or OTHER.
        #[arg(long, value_name = "TYPE")]
        kind: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
    /// Rename a room (room/v1). Prompts unless --force.
    Rename {
        /// Room id or current name.
        room: String,
        /// New display name.
        name: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
}

fn validate_kind(kind: &str) -> Result<String, CliError> {
    let k = kind.trim().to_uppercase().replace(' ', "_");
    if k.is_empty() || !k.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return Err(CliError::Usage(format!(
            "--kind must be a category code like OFFICE or LIVING_ROOM (see `rooms types`), got `{kind}`"
        )));
    }
    Ok(k)
}

fn validate_name(name: &str) -> Result<String, CliError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(CliError::Usage("a room name is required".into()));
    }
    Ok(n.to_string())
}

pub fn run(ctx: &Ctx, cmd: &RoomsCmd) -> Result<(), CliError> {
    match cmd {
        RoomsCmd::List(flag) => {
            let graph = ctx.graph()?;
            let mut items = Vec::new();
            for h in ctx.homes(&graph, flag.home.as_deref())? {
                for r in &h.rooms {
                    items.push(json!({
                        "name": r.name,
                        "devices": r.device_ids.len(),
                        "kind": r.kind,
                        "home": h.name,
                        "id": r.id,
                    }));
                }
            }
            emit_list(
                ctx.json,
                "room",
                items,
                &["name", "devices", "kind", "home", "id"],
            );
            Ok(())
        }
        RoomsCmd::Get { room, home } => {
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let all_rooms: Vec<&Room> = homes.iter().flat_map(|h| h.rooms.iter()).collect();
            let r = resolve_room(&all_rooms, room)?;
            let h = homes
                .iter()
                .find(|h| h.rooms.iter().any(|x| x.id == r.id))
                .expect("room came from one of these homes");
            let devices: Vec<Value> = h
                .devices
                .iter()
                .filter(|d| r.device_ids.contains(&d.id))
                .map(|d| device_row(h, d))
                .collect();
            emit_one(
                ctx.json,
                "room",
                json!({
                    "id": r.id,
                    "name": r.name,
                    "kind": r.kind,
                    "home": h.name,
                    "devices": devices,
                }),
            );
            Ok(())
        }
        RoomsCmd::Create {
            name,
            kind,
            home,
            force,
        } => {
            let name = validate_name(name)?;
            let kind = validate_kind(kind)?;
            require_confirmable(*force, ctx.interactive, "creating a room")?;
            require_layout()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            if homes.len() != 1 {
                return Err(CliError::Usage(
                    "pick one home with --home (or `config set home`) to create the room in".into(),
                ));
            }
            let h = homes[0];
            let kind_name = graph
                .room_types
                .iter()
                .find(|c| c.code == kind)
                .map(|c| c.name.clone())
                .ok_or_else(|| {
                    CliError::Usage(format!(
                        "unknown room category `{kind}` (see `rooms types`)"
                    ))
                })?;
            if let Some(r) = h.rooms.iter().find(|r| r.name.eq_ignore_ascii_case(&name)) {
                return Err(CliError::Usage(format!(
                    "a room named `{}` already exists ({})",
                    r.name, r.id
                )));
            }
            confirm(
                *force,
                &format!("Create room \"{name}\" ({kind}) in {}?", h.name),
            )?;
            let raw = ctx.write(
                spaces::SPACES,
                spaces::CREATE_SPACE,
                &spaces::create_space(&h.id, &name, &kind, &kind_name),
            )?;
            let created = spaces::parse_space(&raw)
                .or_else(|| spaces::parse_spaces(&raw).into_iter().next())
                .ok_or_else(|| {
                    CliError::Upstream("Google accepted the create but returned no room".into())
                })?;
            emit_one(
                ctx.json,
                "room",
                json!({"id": created.id, "name": created.name, "kind": created.kind, "home": h.name, "devices": [], "created": true}),
            );
            Ok(())
        }
        RoomsCmd::Rename {
            room,
            name,
            home,
            force,
        } => {
            let name = validate_name(name)?;
            require_confirmable(*force, ctx.interactive, "renaming a room")?;
            require_layout()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let all_rooms: Vec<&Room> = homes.iter().flat_map(|h| h.rooms.iter()).collect();
            let r = resolve_room(&all_rooms, room)?;
            let h = homes
                .iter()
                .find(|h| h.rooms.iter().any(|x| x.id == r.id))
                .expect("room came from one of these homes");
            confirm(*force, &format!("Rename \"{}\" to \"{name}\"?", r.name))?;
            ctx.write(
                spaces::SPACES,
                spaces::UPDATE_SPACE,
                &spaces::rename_space(&h.id, &r.id, &name),
            )?;
            let raw = ctx.write(
                spaces::SPACES,
                spaces::GET_SPACE,
                &spaces::get_space(&h.id, &r.id),
            )?;
            match spaces::parse_space(&raw) {
                Some(after) if after.name == name => {
                    emit_one(
                        ctx.json,
                        "room",
                        json!({"id": r.id, "name": after.name, "previous_name": r.name, "kind": after.kind, "home": h.name, "changed": true}),
                    );
                    Ok(())
                }
                _ => Err(CliError::Upstream(
                    "Google accepted the rename but the room still has the old name on read-back"
                        .into(),
                )),
            }
        }
        RoomsCmd::Set {
            room,
            change,
            all,
            home,
        } => {
            let change = change.to_change()?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let all_rooms: Vec<&Room> = homes.iter().flat_map(|h| h.rooms.iter()).collect();
            let r = resolve_room(&all_rooms, room)?;
            let h = homes
                .iter()
                .find(|h| h.rooms.iter().any(|x| x.id == r.id))
                .expect("room came from one of these homes");
            let targets: Vec<&crate::homegraph::Device> = h
                .devices
                .iter()
                .filter(|d| r.device_ids.contains(&d.id))
                .filter(|d| {
                    *all || d
                        .assigned_kind
                        .as_deref()
                        .or(d.kind.as_deref())
                        .is_some_and(|k| k.ends_with(".LIGHT"))
                })
                .filter(|d| crate::traits::supports(d, &change))
                .collect();
            if targets.is_empty() {
                return Err(CliError::NotFound(format!(
                    "no {} in {} support that change",
                    if *all {
                        "devices"
                    } else {
                        "lights (try --all)"
                    },
                    r.name
                )));
            }
            let items = super::devices::apply_change(ctx, &targets, &change)?;
            output::emit(
                ctx.json,
                "room-set",
                json!({"room": r.name, "requested": change.describe(), "items": items}),
                |v| {
                    let rows = output::rows_of(v, "items");
                    output::table(&output::table_view(
                        &rows,
                        &[
                            "name",
                            "online",
                            "on",
                            "brightness",
                            "color_temperature_k",
                            "volume",
                        ],
                    ));
                },
            );
            Ok(())
        }
        RoomsCmd::Types => {
            let graph = ctx.graph()?;
            let items = graph
                .room_types
                .iter()
                .map(|c| json!({"code": c.code, "name": c.name}))
                .collect();
            emit_list(ctx.json, "room-type", items, &["code", "name"]);
            Ok(())
        }
    }
}
