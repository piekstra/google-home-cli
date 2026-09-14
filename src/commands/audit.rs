use std::io::Read;

use clap::Args;
use pk_cli_core::{output, CliError};
use serde_json::{json, Value};

use super::rooms::HomeFlag;
use super::{confirm, require_confirmable, require_layout, Ctx};
use crate::audit::{self, Action, Expectation, Fix, Status};
use crate::spaces;

#[derive(Args, Debug, Clone)]
pub struct AuditArgs {
    #[command(flatten)]
    pub home: HomeFlag,
    /// Expected placements as a device-rooms/v1 JSON file (`-` for stdin),
    /// e.g. the output of a vendor CLI's room listing. Without it, a device's
    /// own name is the only evidence (an "Office Lamp" outside Office is flagged).
    #[arg(long, value_name = "FILE")]
    pub expect: Option<String>,
    /// Only report findings that need action (hide `ok` rows).
    #[arg(long)]
    pub problems: bool,
    /// Include Google's own pseudo-devices (routines), which never live in a
    /// room and are skipped by default.
    #[arg(long)]
    pub all: bool,
    /// Fix what the audit found: one `devices move` / `devices place` per
    /// mismatch, unassigned or unplaced row whose expected room came from
    /// `--expect` (never from the name heuristic). Lists the changes and
    /// asks once (room-audit-apply/v1).
    #[arg(long, requires = "expect")]
    pub apply: bool,
    /// Skip the confirmation prompt (required when non-interactive).
    #[arg(long, requires = "apply")]
    pub force: bool,
}

/// The gates `--apply` must pass before any credential or network access.
pub fn validate(args: &AuditArgs, interactive: bool) -> Result<(), CliError> {
    if args.apply {
        require_confirmable(args.force, interactive, "applying audit fixes")?;
        require_layout()?;
    }
    Ok(())
}

/// Device types Google exposes as devices but that have no room to be in.
const ROOMLESS_KINDS: &[&str] = &[
    "action.devices.types.ROUTINE",
    "action.devices.types.SCENE",
    "action.devices.types.PHONE",
];

/// Read and validate `--expect` before any credential or network access.
pub fn load_expectations(arg: Option<&str>) -> Result<Vec<Expectation>, CliError> {
    let Some(path) = arg else {
        return Ok(Vec::new());
    };
    let raw = if path == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| CliError::Usage(format!("reading --expect from stdin: {e}")))?;
        s
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("reading --expect file `{path}`: {e}")))?
    };
    audit::parse_expectations(&raw)
}

pub fn run(ctx: &Ctx, args: &AuditArgs, expectations: Vec<Expectation>) -> Result<(), CliError> {
    let graph = ctx.graph()?;
    let mut findings = Vec::new();
    let mut fixes: Vec<Fix> = Vec::new();
    for h in ctx.homes(&graph, args.home.home.as_deref())? {
        let devices: Vec<_> = h
            .devices
            .iter()
            .filter(|d| {
                args.all
                    || !d
                        .kind
                        .as_deref()
                        .is_some_and(|k| ROOMLESS_KINDS.contains(&k))
            })
            .collect();
        let outside = ctx.unplaced(&h.id)?;
        let unplaced: Vec<_> = outside
            .iter()
            .filter(|d| {
                args.all
                    || !d
                        .kind
                        .as_deref()
                        .is_some_and(|k| ROOMLESS_KINDS.contains(&k))
            })
            .collect();
        let rooms: Vec<_> = h.rooms.iter().collect();
        let mine = audit::audit(Some(&h.name), &devices, &unplaced, &rooms, &expectations);
        if args.apply {
            fixes.extend(audit::plan(&mine, h));
        }
        findings.extend(mine);
    }
    let summary = audit::summarize(&findings);
    if args.apply {
        return apply(ctx, args, &summary, fixes);
    }
    let shown: Vec<Value> = findings
        .iter()
        .filter(|f| !args.problems || !matches!(f.status, Status::Ok | Status::LocalOnly))
        .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
        .collect();
    let payload = json!({
        "summary": summary,
        "items": shown,
    });
    output::emit(ctx.json, "room-audit", payload, |v| {
        let rows = output::rows_of(v, "items");
        if rows.is_empty() {
            println!("nothing to report");
        } else {
            output::table(&output::table_view(
                &rows,
                &[
                    "status",
                    "name",
                    "room",
                    "expected_room",
                    "source",
                    "vendor",
                    "home",
                ],
            ));
        }
        let line: Vec<String> = summary
            .counts()
            .iter()
            .map(|(label, n)| format!("{label} {n}"))
            .collect();
        eprintln!("{}", line.join(" · "));
    });
    Ok(())
}

/// Describe one fix the way the confirmation prompt shows it.
fn describe(f: &Fix) -> String {
    match (&f.skipped, f.action) {
        (Some(why), _) => format!("Skip \"{}\": {why}", f.name),
        (None, Action::Place) => format!(
            "Place \"{}\" into {} in room {}",
            f.name, f.home, f.expected_room
        ),
        (None, Action::Move) => format!(
            "Move \"{}\" from {} to {}",
            f.name,
            f.room.as_deref().unwrap_or("(no room)"),
            f.expected_room
        ),
    }
}

/// One fix against Google, read back before it counts (the same writes as
/// `devices move` / `devices place`).
fn apply_one(ctx: &Ctx, f: &Fix) -> Result<(), CliError> {
    let room_id = f
        .room_id
        .as_deref()
        .ok_or_else(|| CliError::NotFound(f.skipped.clone().unwrap_or_default()))?;
    if f.action == Action::Place {
        ctx.write(
            spaces::STRUCTURES,
            spaces::BATCH_MODIFY_STRUCTURES_DEVICES,
            &spaces::place_device(&f.home_id, &f.device_id),
        )?;
    }
    ctx.write(
        spaces::SPACES,
        spaces::BATCH_MODIFY_SPACES_DEVICES,
        &spaces::move_device(room_id, &f.device_id),
    )?;
    super::devices::verify_in_room(ctx, &f.home_id, room_id, &f.device_id)
}

/// `--apply`: show the plan, ask once, do every fix, report each outcome.
/// A failed fix does not stop the rest; the run exits 5 if any failed.
fn apply(
    ctx: &Ctx,
    args: &AuditArgs,
    summary: &audit::Summary,
    fixes: Vec<Fix>,
) -> Result<(), CliError> {
    let doable = fixes.iter().filter(|f| f.skipped.is_none()).count();
    if doable > 0 {
        let plan: Vec<String> = fixes.iter().map(describe).collect();
        confirm(
            args.force,
            &format!("{}\nApply {doable} change(s)?", plan.join("\n")),
        )?;
    }
    let mut failed = 0usize;
    let mut applied = 0usize;
    let items: Vec<Value> = fixes
        .iter()
        .map(|f| {
            let mut row = serde_json::to_value(f).unwrap_or(Value::Null);
            let outcome = if f.skipped.is_some() {
                Ok(false)
            } else {
                apply_one(ctx, f).map(|()| true)
            };
            if let Value::Object(m) = &mut row {
                match outcome {
                    Ok(done) => {
                        if done {
                            applied += 1;
                        }
                        m.insert("applied".into(), json!(done));
                    }
                    Err(e) => {
                        failed += 1;
                        m.insert("applied".into(), json!(false));
                        m.insert("error".into(), json!(e.to_string()));
                    }
                }
            }
            row
        })
        .collect();
    let skipped = fixes.len() - doable;
    let payload = json!({
        "summary": summary,
        "applied": applied,
        "failed": failed,
        "skipped": skipped,
        "items": items,
    });
    output::emit(ctx.json, "room-audit-apply", payload, |v| {
        let rows = output::rows_of(v, "items");
        if rows.is_empty() {
            println!("nothing to apply");
        } else {
            output::table(&output::table_view(
                &rows,
                &[
                    "action",
                    "name",
                    "room",
                    "expected_room",
                    "applied",
                    "error",
                    "skipped",
                ],
            ));
        }
        eprintln!("applied {applied} · failed {failed} · skipped {skipped}");
    });
    if failed > 0 {
        // The document is out; a returned error would append a second one.
        let e = CliError::Upstream(format!(
            "{failed} of {doable} change(s) failed; see the items above"
        ));
        eprintln!("error: {e}");
        std::process::exit(e.exit_code());
    }
    Ok(())
}
