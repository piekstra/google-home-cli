use std::time::Duration;

use clap::Args;
use pk_cli_core::resolve::pick;
use pk_cli_core::CliError;
use serde_json::{json, Value};

use super::rooms::HomeFlag;
use super::{emit_one, Ctx};
use crate::announce;
use crate::cast::{self, CastDevice};
use crate::homegraph::{resolve_device, resolve_room};
use crate::speech;

#[derive(Args, Debug, Clone)]
pub struct AnnounceArgs {
    /// What to say (with --url: the title shown while it plays).
    pub message: String,
    /// Only the speakers/displays in this room (id or name).
    #[arg(long, value_name = "ROOM", conflicts_with = "device")]
    pub room: Option<String>,
    /// Only this device (id or name).
    #[arg(long, value_name = "DEVICE")]
    pub device: Option<String>,
    #[command(flatten)]
    pub home: HomeFlag,
    /// How to deliver it. `local` speaks over the LAN with the Cast protocol
    /// (no Google account involved; interrupts whatever is playing). `cloud`
    /// is the Home app's broadcast path, which Google currently rejects
    /// (docs/api.md).
    #[arg(long, value_parser = ["local", "cloud"], default_value = "local")]
    pub via: String,
    /// Language of the generated speech (BCP-47: en, en-GB, de, …). Local only.
    #[arg(long, default_value = "en", value_name = "LANG")]
    pub lang: String,
    /// Play this audio URL instead of speech (the device fetches it). Local only.
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// Volume 0–100 for the announcement; the previous level is put back
    /// afterwards. Local only.
    #[arg(long, value_name = "PCT", value_parser = clap::value_parser!(u8).range(0..=100))]
    pub volume: Option<u8>,
    /// How long to listen for devices on the LAN. Local only.
    #[arg(long, default_value_t = 3, value_name = "SECS")]
    pub discover_secs: u64,
    /// Experiment knob: skip the UpdateToken handshake (cloud).
    #[arg(long, hide = true)]
    pub no_handshake: bool,
    /// Experiment knob: handshake target form (`bare` structure id, or `structure`).
    #[arg(long, hide = true, default_value = "bare")]
    pub handshake_target: String,
    /// Experiment knob: context enum value (cloud).
    #[arg(long, hide = true, default_value_t = 2)]
    pub ctx_enum: u64,
}

/// Argument checks that need no network.
pub fn validate(args: &AnnounceArgs) -> Result<(), CliError> {
    let message = args.message.trim();
    if message.is_empty() {
        return Err(CliError::Usage("nothing to announce".into()));
    }
    let limit = if args.via == "local" && args.url.is_none() {
        speech::MAX_CHARS
    } else {
        256
    };
    if message.chars().count() > limit {
        return Err(CliError::Usage(format!(
            "keep announcements under {limit} characters"
        )));
    }
    if let Some(u) = &args.url {
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err(CliError::Usage("--url must be an http(s) URL".into()));
        }
    }
    Ok(())
}

/// Returns how many devices did not play the announcement; `main` turns
/// that into the exit code once the report is out.
pub fn run(ctx: &Ctx, args: &AnnounceArgs) -> Result<usize, CliError> {
    validate(args)?;
    if args.via == "cloud" {
        run_cloud(ctx, args)?;
        return Ok(0);
    }
    run_local(ctx, args)
}

/// Which discovered devices an announcement goes to.
pub fn select<'a>(
    found: &'a [CastDevice],
    device: Option<&str>,
    room_members: Option<&[String]>,
) -> Result<Vec<&'a CastDevice>, CliError> {
    if found.is_empty() {
        return Err(CliError::NotFound(
            "no Cast devices answered on the LAN (are you on the home network?)".into(),
        ));
    }
    if let Some(q) = device {
        return Ok(vec![pick(
            found,
            q,
            |d| vec![d.id.clone()],
            |d| &d.name,
            "speaker or display",
        )?]);
    }
    if let Some(names) = room_members {
        let picked: Vec<&CastDevice> = found
            .iter()
            .filter(|d| names.iter().any(|n| n.eq_ignore_ascii_case(&d.name)))
            .collect();
        if picked.is_empty() {
            return Err(CliError::NotFound(
                "none of that room's devices answered on the LAN".into(),
            ));
        }
        return Ok(picked);
    }
    Ok(found.iter().collect())
}

fn run_local(ctx: &Ctx, args: &AnnounceArgs) -> Result<usize, CliError> {
    let message = args.message.trim();
    // A room is Google Home's idea, so only --room needs the graph; --device
    // and "everywhere" work with no Google account at all.
    let room_members: Option<Vec<String>> = match &args.room {
        Some(r) => {
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, args.home.home.as_deref())?;
            let rooms: Vec<_> = homes.iter().flat_map(|h| h.rooms.iter()).collect();
            let room = resolve_room(&rooms, r)?;
            Some(
                homes
                    .iter()
                    .flat_map(|h| h.devices.iter())
                    .filter(|d| room.device_ids.contains(&d.id))
                    .map(|d| d.name.clone())
                    .collect(),
            )
        }
        None => None,
    };
    if ctx.verbose {
        eprintln!("cast: listening for devices for {}s", args.discover_secs);
    }
    let found = cast::discover(Duration::from_secs(args.discover_secs))?;
    let targets = select(&found, args.device.as_deref(), room_members.as_deref())?;
    let url = match &args.url {
        Some(u) => u.clone(),
        None => speech::tts_url(message, &args.lang),
    };
    // Every device at once: a broadcast, not a relay, and one device that
    // hangs cannot hold the others up.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| CliError::Other(format!("runtime: {e}")))?;
    let outcomes: Vec<cast::Outcome> = rt.block_on(async {
        let mut set = tokio::task::JoinSet::new();
        for (i, d) in targets.iter().enumerate() {
            let (d, url, title) = ((*d).clone(), url.clone(), message.to_string());
            let (volume, verbose) = (args.volume, ctx.verbose);
            set.spawn(async move { (i, cast::play(&d, &url, &title, volume, verbose).await) });
        }
        let mut done = Vec::new();
        while let Some(r) = set.join_next().await {
            if let Ok(x) = r {
                done.push(x);
            }
        }
        done.sort_by_key(|(i, _)| *i);
        done.into_iter().map(|(_, o)| o).collect()
    });
    let failed = outcomes.iter().filter(|o| !o.played).count();
    let items: Vec<Value> = outcomes
        .iter()
        .map(|o| serde_json::to_value(o).unwrap_or(Value::Null))
        .collect();
    emit_one(
        ctx.json,
        "announce",
        json!({
            "message": message,
            "via": "local",
            "audio": if args.url.is_some() { "url" } else { "speech" },
            "targets": items,
            "played": outcomes.len() - failed,
            "failed": failed,
            "sent": failed == 0,
        }),
    );
    Ok(failed)
}

fn run_cloud(ctx: &Ctx, args: &AnnounceArgs) -> Result<(), CliError> {
    let message = args.message.trim();
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
            "via": "cloud",
            "target": label,
            "target_id": target,
            "http_status": reply.http_status,
            "response_bytes": reply.messages.len(),
            "sent": true,
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, id: &str) -> CastDevice {
        CastDevice {
            name: name.into(),
            id: id.into(),
            model: "Google Nest Hub".into(),
            ip: "192.0.2.10".parse().unwrap(),
            port: 8009,
        }
    }

    #[test]
    fn targets_resolve_by_name_room_or_everywhere() {
        let found = vec![dev("Kitchen display", "aa"), dev("Living Room TV", "bb")];
        assert_eq!(select(&found, Some("kitchen"), None).unwrap()[0].id, "aa");
        assert_eq!(
            select(&found, Some("bb"), None).unwrap()[0].name,
            "Living Room TV"
        );
        assert_eq!(select(&found, None, None).unwrap().len(), 2);
        let members = vec!["Living Room TV".to_string(), "Couch Lamp".to_string()];
        let in_room = select(&found, None, Some(&members)).unwrap();
        assert_eq!(in_room.len(), 1);
        assert_eq!(in_room[0].id, "bb");
        assert_eq!(
            select(&found, None, Some(&["Garage".to_string()]))
                .unwrap_err()
                .exit_code(),
            4
        );
        assert_eq!(select(&[], None, None).unwrap_err().exit_code(), 4);
        assert!(select(&found, Some("attic"), None).is_err());
    }

    #[test]
    fn arguments_are_checked_before_any_network() {
        let mut a = AnnounceArgs {
            message: "  ".into(),
            room: None,
            device: None,
            home: HomeFlag { home: None },
            via: "local".into(),
            lang: "en".into(),
            url: None,
            volume: None,
            discover_secs: 3,
            no_handshake: false,
            handshake_target: "bare".into(),
            ctx_enum: 2,
        };
        assert_eq!(validate(&a).unwrap_err().exit_code(), 2);
        a.message = "x".repeat(201);
        assert!(validate(&a).is_err(), "speech is capped at 200 characters");
        a.url = Some("https://example.com/a.mp3".into());
        assert!(validate(&a).is_ok(), "a URL is not spoken, so 256 applies");
        a.url = Some("ftp://x".into());
        assert!(validate(&a).is_err());
    }
}
