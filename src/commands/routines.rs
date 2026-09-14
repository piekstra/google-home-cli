//! Routines / automations (`AutomationService`). List shape and the execute
//! body were captured live by googlehome-mcp (2026-07); the script writes
//! (validate, upsert, delete) were captured from the Google Home web script
//! editor on 2026-09-14 (docs/api.md). Scripts are the editor's YAML.

use std::io::Read;

use clap::Subcommand;
use pk_cli_core::{output, CliError};
use serde_json::{json, Value};

use super::rooms::HomeFlag;
use super::{confirm, emit_list, emit_one, require_confirmable, Ctx};

const SERVICE: &str = "AutomationService";
const LIST: &str = "ListAutomations";
const EXECUTE: &str = "ExecuteAutomation";
const VALIDATE: &str = "ValidateAutomation";
const UPSERT: &str = "UpsertAutomation";
const DELETE: &str = "DeleteAutomation";
/// Scripts are small; anything bigger is not a script.
const MAX_SCRIPT_BYTES: usize = 64 * 1024;

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
    /// One routine in detail: kind, description, starter and action
    /// summaries (routine/v1). The list does not carry a script's YAML; keep
    /// your scripts in files.
    Get {
        /// Routine id, exact name, or unique partial name.
        routine: String,
        #[command(flatten)]
        home: HomeFlag,
    },
    /// Check a script against Google's validator without saving it
    /// (routine-validate/v1). The script is the Home script editor's YAML.
    Validate {
        /// Script file (`-` for stdin).
        #[arg(long, value_name = "FILE")]
        file: String,
        #[command(flatten)]
        home: HomeFlag,
    },
    /// Create an automation from a script file (routine/v1). Validated
    /// first; prompts unless --force. The script's `metadata.name` is the
    /// automation's name.
    Create {
        /// Script file (`-` for stdin).
        #[arg(long, value_name = "FILE")]
        file: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
    /// Replace an automation's script (routine/v1). Validated first; prompts
    /// unless --force.
    Update {
        /// Routine id, exact name, or unique partial name.
        routine: String,
        /// Script file (`-` for stdin).
        #[arg(long, value_name = "FILE")]
        file: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
    /// Delete an automation (routine-delete/v1). Prompts unless --force.
    Delete {
        /// Routine id, exact name, or unique partial name.
        routine: String,
        #[command(flatten)]
        home: HomeFlag,
        /// Skip the confirmation prompt (required when non-interactive).
        #[arg(long)]
        force: bool,
    },
}

/// What a list row is, from its own slot (index 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineKind {
    /// Google's Home/Away presence routines (`structure_<id>.sbr_00N`).
    Presence,
    /// A script-editor automation.
    Script,
    /// A legacy Assistant routine, edited in the Assistant settings.
    Assistant,
    /// A value this build has not seen.
    Other,
}

impl RoutineKind {
    fn from_slot(v: &Value) -> RoutineKind {
        match v.as_i64() {
            Some(1) => RoutineKind::Presence,
            Some(2) => RoutineKind::Script,
            Some(3) => RoutineKind::Assistant,
            _ => RoutineKind::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub kind: RoutineKind,
    pub manual: bool,
    pub starters: Option<String>,
    pub actions: Option<String>,
    /// `metadata.description` of a script automation (row index 26).
    pub description: Option<String>,
}

fn at(v: &Value, i: usize) -> &Value {
    v.get(i).unwrap_or(&Value::Null)
}

/// One automation row: `[id, ?, manuallyRunnable, name, starters, actions,
/// ?, ?, kind, …, 26: description]`. (The save reply also carries the YAML
/// at 14; list rows never do, so it is not kept.)
pub fn parse_row(r: &Value) -> Option<Routine> {
    Some(Routine {
        id: at(r, 0).as_str()?.to_string(),
        name: at(r, 3).as_str()?.to_string(),
        kind: RoutineKind::from_slot(at(r, 8)),
        manual: at(r, 2).as_i64() == Some(1),
        starters: at(r, 4).as_str().map(str::to_string),
        actions: at(r, 5).as_str().map(str::to_string),
        description: at(r, 26).as_str().map(str::to_string),
    })
}

/// `[[ row, … ]]`.
pub fn parse_list(raw: &Value) -> Vec<Routine> {
    at(raw, 0)
        .as_array()
        .map(|a| a.iter().filter_map(parse_row).collect())
        .unwrap_or_default()
}

// ---- request bodies (captured from the web script editor) -----------------

/// The automation object the editor sends: `id` at 1 when updating, `status
/// {is_enabled}` at 8, kind 2 (a script) at 9, `script_details {content}` at 15.
fn automation(id: Option<&str>, script: &str, status: Option<bool>) -> Value {
    json!([
        id,
        null,
        null,
        null,
        null,
        null,
        null,
        status.map(|on| json!([[u8::from(on)]])),
        2,
        null,
        null,
        null,
        null,
        null,
        [script]
    ])
}

/// `id` is the automation being replaced: without it Google counts the
/// automation's own name and voice phrase as duplicates.
pub fn validate_body(home_id: &str, id: Option<&str>, script: &str) -> Value {
    json!([home_id, automation(id, script, None)])
}

/// Create (no id) or replace (id) a script; the mask names what is written.
pub fn upsert_body(home_id: &str, id: Option<&str>, script: &str) -> Value {
    json!([
        home_id,
        automation(id, script, Some(true)),
        [["script_details.content", "status.is_enabled"]]
    ])
}

pub fn delete_body(home_id: &str, id: &str) -> Value {
    json!([home_id, id])
}

/// One problem Google found: `[line, column, message, severity]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScriptError {
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub message: String,
}

impl ScriptError {
    /// `line 2:9: The script name [x] is already in use.`
    pub fn render(&self) -> String {
        match (self.line, self.column) {
            (Some(l), Some(c)) => format!("line {l}:{c}: {}", self.message),
            (Some(l), None) => format!("line {l}: {}", self.message),
            _ => self.message.clone(),
        }
    }
}

/// Google's messages carry a little HTML (`<br>`, `&#39;`).
fn plain_text(s: &str) -> String {
    s.replace("<br>", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Google answers `[]` when the script is fine and `[[[line, col, msg, sev], …]]`
/// otherwise: one list of problem rows, each `[line, column, message, severity]`.
pub fn validation_errors(raw: &Value) -> Vec<ScriptError> {
    let is_row = |v: &Value| {
        v.as_array()
            .is_some_and(|r| r.first().is_some_and(Value::is_number))
    };
    let rows: Vec<&Value> = match raw {
        Value::Array(a) => a
            .iter()
            .flat_map(|x| match x {
                // The wrapper list around the rows.
                Value::Array(inner) if !is_row(x) => inner.iter().collect::<Vec<_>>(),
                other => vec![other],
            })
            .collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    };
    rows.into_iter()
        .map(|r| match r {
            Value::Array(f) => ScriptError {
                line: at(r, 0).as_u64(),
                column: at(r, 1).as_u64(),
                message: plain_text(f.iter().find_map(Value::as_str).unwrap_or(&r.to_string())),
            },
            other => ScriptError {
                line: None,
                column: None,
                message: plain_text(&other.to_string()),
            },
        })
        .collect()
}

/// A rejection that names no valid devices at all is Google's device cache
/// answering cold (seen live: the same script passed a second later).
fn looks_cold(errors: &[ScriptError]) -> bool {
    errors
        .iter()
        .any(|e| e.message.contains("valid device names: []"))
}

/// Read the script before any credential or network access: `-` is stdin.
pub fn load_script(path: &str) -> Result<String, CliError> {
    let raw = if path == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| CliError::Usage(format!("reading the script from stdin: {e}")))?;
        s
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("reading script `{path}`: {e}")))?
    };
    check_script(&raw)?;
    Ok(raw)
}

/// The cheap local checks: not empty, not huge, looks like a script.
pub fn check_script(raw: &str) -> Result<(), CliError> {
    if raw.trim().is_empty() {
        return Err(CliError::Usage("the script is empty".into()));
    }
    if raw.len() > MAX_SCRIPT_BYTES {
        return Err(CliError::Usage(format!(
            "the script is {} bytes; scripts are under {MAX_SCRIPT_BYTES}",
            raw.len()
        )));
    }
    if !raw.contains("automations:") {
        return Err(CliError::Usage(
            "not an automation script: no `automations:` block (see the Home script editor's YAML)"
                .into(),
        ));
    }
    Ok(())
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
        "kind": r.kind,
        "runnable": r.manual,
        "starters": r.starters,
        "actions": r.actions,
        "home": home,
        "id": r.id,
    })
}

/// Every routine in the selected homes, with the home it belongs to.
fn all_routines(ctx: &Ctx, flag: &HomeFlag) -> Result<Vec<(Routine, String, String)>, CliError> {
    let graph = ctx.graph()?;
    let mut all = Vec::new();
    for h in ctx.homes(&graph, flag.home.as_deref())? {
        let raw = ctx.write(SERVICE, LIST, &json!([h.id]))?;
        all.extend(
            parse_list(&raw)
                .into_iter()
                .map(|r| (r, h.id.clone(), h.name.clone())),
        );
    }
    Ok(all)
}

/// Resolve `query` among the selected homes' routines.
fn find(ctx: &Ctx, flag: &HomeFlag, query: &str) -> Result<(Routine, String, String), CliError> {
    let all = all_routines(ctx, flag)?;
    let routines: Vec<Routine> = all.iter().map(|(r, _, _)| r.clone()).collect();
    let r = resolve(&routines, query)?.clone();
    let (_, home_id, home_name) = all
        .iter()
        .find(|(x, _, _)| x.id == r.id)
        .expect("resolved from this list");
    Ok((r, home_id.clone(), home_name.clone()))
}

/// The one home a new script goes to.
fn one_home(ctx: &Ctx, flag: &HomeFlag) -> Result<(String, String), CliError> {
    let graph = ctx.graph()?;
    let homes = ctx.homes(&graph, flag.home.as_deref())?;
    if homes.len() != 1 {
        return Err(CliError::Usage(
            "pick one home with --home (or `config set home`) for the automation".into(),
        ));
    }
    Ok((homes[0].id.clone(), homes[0].name.clone()))
}

/// Ask Google to check a script: the problems it found, none when the
/// script is fine. `id` is the automation being replaced, if any.
fn validate_with_google(
    ctx: &Ctx,
    home_id: &str,
    id: Option<&str>,
    script: &str,
) -> Result<Vec<ScriptError>, CliError> {
    let mut errors =
        validation_errors(&ctx.write(SERVICE, VALIDATE, &validate_body(home_id, id, script))?);
    if looks_cold(&errors) {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        errors = validation_errors(&ctx.write(
            SERVICE,
            VALIDATE,
            &validate_body(home_id, id, script),
        )?);
    }
    Ok(errors)
}

/// A write refuses a rejected script, naming what Google did not like.
fn require_valid(errors: Vec<ScriptError>) -> Result<(), CliError> {
    if errors.is_empty() {
        return Ok(());
    }
    Err(CliError::Usage(format!(
        "Google rejected the script:\n  {}",
        errors
            .iter()
            .map(ScriptError::render)
            .collect::<Vec<_>>()
            .join("\n  ")
    )))
}

/// Upsert and read back: the home's list must carry the automation as
/// Google's reply described it (name and the starter/action summaries).
/// A replacement that keeps those summaries is indistinguishable from the
/// old script here, which the DTO's `read_back: "listed"` says.
fn upsert(ctx: &Ctx, home_id: &str, id: Option<&str>, script: &str) -> Result<Routine, CliError> {
    let raw = ctx.write(SERVICE, UPSERT, &upsert_body(home_id, id, script))?;
    let saved = parse_row(&raw).ok_or_else(|| {
        CliError::Upstream(format!("Google accepted the script but answered {raw}"))
    })?;
    let listed = ctx.write(SERVICE, LIST, &json!([home_id]))?;
    match parse_list(&listed).into_iter().find(|r| r.id == saved.id) {
        Some(row)
            if row.name == saved.name
                && row.starters == saved.starters
                && row.actions == saved.actions =>
        {
            Ok(saved)
        }
        Some(row) => Err(CliError::Upstream(format!(
            "Google accepted the script but lists the automation as \"{}\" ({}, {}) on read-back",
            row.name,
            row.starters.as_deref().unwrap_or("?"),
            row.actions.as_deref().unwrap_or("?")
        ))),
        None => Err(CliError::Upstream(
            "Google accepted the script but the home does not list the automation on read-back"
                .into(),
        )),
    }
}

/// `Ok(Some(error))` is a failure whose report is already on stdout
/// (`validate` on a rejected script); `main` turns it into the exit code
/// without a second document. Everything else is `Ok(None)`.
pub fn run(ctx: &Ctx, cmd: &RoutinesCmd) -> Result<Option<CliError>, CliError> {
    match cmd {
        RoutinesCmd::List(flag) => {
            let items: Vec<Value> = all_routines(ctx, flag)?
                .iter()
                .map(|(r, _, home)| row(r, home))
                .collect();
            emit_list(
                ctx.json,
                "routine",
                items,
                &["name", "kind", "runnable", "starters", "home", "id"],
            );
            Ok(None)
        }
        RoutinesCmd::Run {
            routine,
            home,
            force,
        } => {
            require_confirmable(*force, ctx.interactive, "running a routine")?;
            let (r, home_id, home_name) = find(ctx, home, routine)?;
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
            Ok(None)
        }
        RoutinesCmd::Get { routine, home } => {
            let (r, _, home_name) = find(ctx, home, routine)?;
            let mut v = row(&r, &home_name);
            v["description"] = json!(r.description);
            emit_one(ctx.json, "routine", v);
            Ok(None)
        }
        RoutinesCmd::Validate { file, home } => {
            let script = load_script(file)?;
            let (home_id, home_name) = one_home(ctx, home)?;
            let errors = validate_with_google(ctx, &home_id, None, &script)?;
            let valid = errors.is_empty();
            let payload = json!({
                "valid": valid,
                "home": home_name,
                "bytes": script.len(),
                "errors": errors,
            });
            output::emit(ctx.json, "routine-validate", payload, |_| {
                if valid {
                    println!("valid ({} bytes)", script.len());
                } else {
                    for e in &errors {
                        println!("{}", e.render());
                    }
                }
            });
            Ok((!valid).then(|| {
                CliError::Usage(format!(
                    "Google rejected the script ({} problem(s)); see the report",
                    errors.len()
                ))
            }))
        }
        RoutinesCmd::Create { file, home, force } => {
            let script = load_script(file)?;
            require_confirmable(*force, ctx.interactive, "creating an automation")?;
            let (home_id, home_name) = one_home(ctx, home)?;
            require_valid(validate_with_google(ctx, &home_id, None, &script)?)?;
            confirm(
                *force,
                &format!(
                    "Create this automation in {home_name} ({} bytes of script)?",
                    script.len()
                ),
            )?;
            let saved = upsert(ctx, &home_id, None, &script)?;
            let mut v = row(&saved, &home_name);
            v["created"] = json!(true);
            v["read_back"] = json!("listed");
            emit_one(ctx.json, "routine", v);
            Ok(None)
        }
        RoutinesCmd::Update {
            routine,
            file,
            home,
            force,
        } => {
            let script = load_script(file)?;
            require_confirmable(*force, ctx.interactive, "replacing an automation's script")?;
            let (r, home_id, home_name) = find(ctx, home, routine)?;
            require_valid(validate_with_google(ctx, &home_id, Some(&r.id), &script)?)?;
            confirm(
                *force,
                &format!("Replace the script of \"{}\" in {home_name}?", r.name),
            )?;
            let saved = upsert(ctx, &home_id, Some(&r.id), &script)?;
            let mut v = row(&saved, &home_name);
            v["previous_name"] = json!(r.name);
            v["updated"] = json!(true);
            v["read_back"] = json!("listed");
            emit_one(ctx.json, "routine", v);
            Ok(None)
        }
        RoutinesCmd::Delete {
            routine,
            home,
            force,
        } => {
            require_confirmable(*force, ctx.interactive, "deleting an automation")?;
            let (r, home_id, home_name) = find(ctx, home, routine)?;
            confirm(
                *force,
                &format!("Delete automation \"{}\" from {home_name}?", r.name),
            )?;
            ctx.write(SERVICE, DELETE, &delete_body(&home_id, &r.id))?;
            let listed = ctx.write(SERVICE, LIST, &json!([home_id]))?;
            if parse_list(&listed).iter().any(|x| x.id == r.id) {
                return Err(CliError::Upstream(
                    "Google accepted the delete but still lists the automation on read-back".into(),
                ));
            }
            emit_one(
                ctx.json,
                "routine-delete",
                json!({"id": r.id, "name": r.name, "home": home_name, "deleted": true}),
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "metadata:\n  name: t\nautomations:\n  - starters:\n      - type: time.schedule\n        at: 12:00\n    actions: []\n";

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
        assert_eq!(l[0].kind, RoutineKind::Other, "short rows carry no kind");
        let kinds = json!([[
            [
                "p",
                null,
                0,
                "Home",
                "2 starters",
                "1 action",
                null,
                null,
                1
            ],
            [
                "s",
                null,
                1,
                "Script",
                "1 starter",
                "1 action",
                null,
                null,
                2
            ],
            [
                "a",
                null,
                1,
                "Bedtime",
                "1 starter",
                "8 actions",
                null,
                null,
                3
            ]
        ]]);
        let k: Vec<RoutineKind> = parse_list(&kinds).iter().map(|r| r.kind).collect();
        assert_eq!(
            k,
            [
                RoutineKind::Presence,
                RoutineKind::Script,
                RoutineKind::Assistant
            ]
        );
        assert_eq!(resolve(&l, "good").unwrap().id, "r1");
        assert!(matches!(resolve(&l, "nope"), Err(CliError::NotFound(_))));
    }

    #[test]
    fn write_bodies_match_the_web_editor_captures() {
        // Captured 2026-09-14 from home.google.com/automations (see docs/api.md).
        assert_eq!(
            validate_body("h", None, "S"),
            json!([
                "h",
                [
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    2,
                    null,
                    null,
                    null,
                    null,
                    null,
                    ["S"]
                ]
            ])
        );
        assert_eq!(
            upsert_body("h", None, "S"),
            json!([
                "h",
                [
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    [[1]],
                    2,
                    null,
                    null,
                    null,
                    null,
                    null,
                    ["S"]
                ],
                [["script_details.content", "status.is_enabled"]]
            ])
        );
        assert_eq!(upsert_body("h", Some("a1"), "S")[1][0], json!("a1"));
        assert_eq!(delete_body("h", "a1"), json!(["h", "a1"]));
        // The upsert answers with the new row.
        let mut reply = json!([
            "a1",
            null,
            1,
            "ghome dogfood",
            "1 starter",
            "1 action",
            "assistant-settings://x",
            [[1]],
            2
        ]);
        let r = parse_row(&reply).unwrap();
        assert_eq!(
            (r.id.as_str(), r.name.as_str(), r.manual),
            ("a1", "ghome dogfood", true)
        );
        assert_eq!(r.kind, RoutineKind::Script);
        // A script automation's description rides at index 26.
        let arr = reply.as_array_mut().unwrap();
        arr.resize(27, Value::Null);
        arr[26] = json!("why");
        let r = parse_row(&reply).unwrap();
        assert_eq!(r.description.as_deref(), Some("why"));
        assert_eq!(validate_body("h", Some("a1"), "S")[1][0], json!("a1"));
        assert!(validation_errors(&json!([])).is_empty());
        assert!(validation_errors(&Value::Null).is_empty());
        // Captured live: [line, column, message, severity], HTML in the message.
        let errs = validation_errors(&json!([[
            [2, 9, "The script name [x] is already in use.", 3],
            [11, 18, "[Lamp - Nowhere] is an invalid device name. The list of valid device names:<br> [].", 3]
        ]]));
        assert_eq!(errs.len(), 2);
        assert_eq!(
            errs[0].render(),
            "line 2:9: The script name [x] is already in use."
        );
        assert_eq!(
            errs[1].message,
            "[Lamp - Nowhere] is an invalid device name. The list of valid device names: []."
        );
        assert!(looks_cold(&errs));
        assert!(!looks_cold(&errs[..1]));
        assert_eq!(plain_text("Amelia&#39;s Room<br>x"), "Amelia's Room x");
        assert!(require_valid(Vec::new()).is_ok());
        let e = require_valid(errs).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("line 2:9:"), "{e}");
    }

    #[test]
    fn scripts_are_checked_locally_first() {
        assert!(check_script(SCRIPT).is_ok());
        assert_eq!(check_script("   ").unwrap_err().exit_code(), 2);
        assert_eq!(check_script("hello: world\n").unwrap_err().exit_code(), 2);
        assert_eq!(
            check_script(&"x".repeat(MAX_SCRIPT_BYTES + 1))
                .unwrap_err()
                .exit_code(),
            2
        );
    }
}
