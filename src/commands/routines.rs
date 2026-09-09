//! Routines / automations (`AutomationService`). List shape and the execute
//! body were captured live by googlehome-mcp (2026-07) and hold here.

use clap::Subcommand;
use pk_cli_core::CliError;
use serde_json::{json, Value};

use super::rooms::HomeFlag;
use super::{confirm, emit_list, emit_one, require_confirmable, Ctx};

const SERVICE: &str = "AutomationService";
const LIST: &str = "ListAutomations";
const EXECUTE: &str = "ExecuteAutomation";

#[derive(Subcommand, Debug)]
pub enum RoutinesCmd {
    /// List routines and automations with whether they can be run on demand
    /// (routine-list/v1).
    #[command(visible_alias = "ls")]
    List(HomeFlag),
    /// Run a routine now (routine-run/v1). Prompts unless --force: a routine
    /// can do anything its actions say.
    Run {
        /// Routine id, exact name, or unique partial name.
        routine: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub manual: bool,
    pub starters: Option<String>,
    pub actions: Option<String>,
}

fn at(v: &Value, i: usize) -> &Value {
    v.get(i).unwrap_or(&Value::Null)
}

/// `[[ [id, ?, manuallyRunnable, name, starters, actions, …], … ]]`.
pub fn parse_list(raw: &Value) -> Vec<Routine> {
    at(raw, 0)
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(Routine {
                        id: at(r, 0).as_str()?.to_string(),
                        name: at(r, 3).as_str()?.to_string(),
                        manual: at(r, 2).as_i64() == Some(1),
                        starters: at(r, 4).as_str().map(str::to_string),
                        actions: at(r, 5).as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn resolve<'a>(all: &'a [Routine], query: &str) -> Result<&'a Routine, CliError> {
    if let Some(r) = all.iter().find(|r| r.id == query) {
        return Ok(r);
    }
    let want = query.trim().to_lowercase();
    let exact: Vec<&Routine> = all
        .iter()
        .filter(|r| r.name.trim().to_lowercase() == want)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    let partial: Vec<&Routine> = all
        .iter()
        .filter(|r| r.name.to_lowercase().contains(&want))
        .collect();
    match partial.len() {
        0 => Err(CliError::NotFound(format!("no routine matching `{query}`"))),
        1 => Ok(partial[0]),
        _ => Err(CliError::Usage(format!(
            "`{query}` matches more than one routine: {}",
            partial
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ))),
    }
}

fn row(r: &Routine, home: &str) -> Value {
    json!({
        "name": r.name,
        "runnable": r.manual,
        "starters": r.starters,
        "actions": r.actions,
        "home": home,
        "id": r.id,
    })
}

pub fn run(ctx: &Ctx, cmd: &RoutinesCmd) -> Result<(), CliError> {
    match cmd {
        RoutinesCmd::List(flag) => {
            let graph = ctx.graph()?;
            let mut items = Vec::new();
            for h in ctx.homes(&graph, flag.home.as_deref())? {
                let raw = ctx.write(SERVICE, LIST, &json!([h.id]))?;
                items.extend(parse_list(&raw).iter().map(|r| row(r, &h.name)));
            }
            emit_list(
                ctx.json,
                "routine",
                items,
                &["name", "runnable", "starters", "home", "id"],
            );
            Ok(())
        }
        RoutinesCmd::Run {
            routine,
            home,
            force,
        } => {
            require_confirmable(*force, ctx.interactive, "running a routine")?;
            let graph = ctx.graph()?;
            let homes = ctx.homes(&graph, home.home.as_deref())?;
            let mut all: Vec<(Routine, String, String)> = Vec::new();
            for h in &homes {
                let raw = ctx.write(SERVICE, LIST, &json!([h.id]))?;
                all.extend(
                    parse_list(&raw)
                        .into_iter()
                        .map(|r| (r, h.id.clone(), h.name.clone())),
                );
            }
            let routines: Vec<Routine> = all.iter().map(|(r, _, _)| r.clone()).collect();
            let r = resolve(&routines, routine)?.clone();
            let (_, home_id, home_name) = all
                .iter()
                .find(|(x, _, _)| x.id == r.id)
                .expect("resolved from this list");
            if !r.manual {
                return Err(CliError::Usage(format!(
                    "`{}` is condition- or schedule-triggered and can't be started on demand",
                    r.name
                )));
            }
            confirm(*force, &format!("Run routine \"{}\" now?", r.name))?;
            let raw = ctx.write(SERVICE, EXECUTE, &json!([home_id, r.id, null, 2]))?;
            let ok = at(&raw, 0)
                .as_str()
                .map(|s| s == "1")
                .or_else(|| at(&raw, 0).as_i64().map(|n| n == 1))
                .unwrap_or(false);
            if !ok {
                return Err(CliError::Upstream(format!(
                    "Google did not confirm the run (response {})",
                    raw
                )));
            }
            emit_one(
                ctx.json,
                "routine-run",
                json!({"id": r.id, "name": r.name, "home": home_name, "started": true}),
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses_the_captured_shape() {
        let raw = json!([[
            [
                "r1",
                null,
                1,
                "Good night",
                "voice: good night",
                "lights off",
                null,
                null,
                null,
                null,
                null
            ],
            ["r2", null, 0, "Sunset", "sunset", "porch on"]
        ]]);
        let l = parse_list(&raw);
        assert_eq!(l.len(), 2);
        assert!(l[0].manual);
        assert!(!l[1].manual);
        assert_eq!(resolve(&l, "good").unwrap().id, "r1");
        assert!(matches!(resolve(&l, "nope"), Err(CliError::NotFound(_))));
    }
}
