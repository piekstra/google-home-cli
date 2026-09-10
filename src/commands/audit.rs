use std::io::Read;

use clap::Args;
use pk_cli_core::{output, CliError};
use serde_json::{json, Value};

use super::rooms::HomeFlag;
use super::Ctx;
use crate::audit::{self, Expectation, Status};

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
        findings.extend(audit::audit(
            Some(&h.name),
            &devices,
            &unplaced,
            &rooms,
            &expectations,
        ));
    }
    let summary = audit::summarize(&findings);
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
