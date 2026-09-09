//! One module per top-level command group. `Ctx::graph` is the one place the
//! network is touched for reads: every domain command starts from a fresh
//! `GetHomeGraph`.

pub mod agents;
pub mod announce;
pub mod api;
pub mod audit;
pub mod devices;
pub mod homes;
pub mod rooms;
pub mod routines;

use pk_cli_config::ConfigStore;
use pk_cli_core::{output, CliError};
use pk_cli_secrets::CredentialStore;
use serde_json::Value;

use crate::config::Config;
use crate::foyer::{self, Foyer};
use crate::homegraph::{self, Home, HomeGraph};
use crate::session::Session;

pub struct Ctx<'a> {
    pub json: bool,
    pub verbose: bool,
    /// Prompting is acceptable (stdin is a TTY and no `--json`).
    pub interactive: bool,
    pub store: &'a ConfigStore,
    pub creds: &'a CredentialStore,
    pub cfg: Config,
}

impl<'a> Ctx<'a> {
    /// The stored session, or exit 3 with a pointer at `auth login`.
    pub fn session(&self) -> Result<Session, CliError> {
        let email = self.cfg.username.clone().ok_or_else(|| {
            CliError::Auth("no Google account configured; run `ghome auth login`".into())
        })?;
        let android_id = self.cfg.android_id.clone().ok_or_else(|| {
            CliError::Auth("no device identity stored; run `ghome auth login`".into())
        })?;
        Session::load(self.creds, &email, &android_id)
    }

    pub fn http(&self) -> Result<reqwest::blocking::Client, CliError> {
        pk_cli_http::client(crate::BIN, env!("CARGO_PKG_VERSION"))
    }

    /// Fetch and parse the home graph.
    pub fn graph(&self) -> Result<HomeGraph, CliError> {
        let raw = self.raw_graph()?;
        Ok(homegraph::parse(&raw))
    }

    /// One write RPC with a fresh session; the caller verifies by reading back.
    pub fn write(&self, service: &str, method: &str, body: &Value) -> Result<Value, CliError> {
        let mut session = self.session()?;
        let mut foyer = Foyer::new(self.http()?, &mut session, self.creds, self.verbose);
        foyer.rpc(service, method, body)
    }

    pub fn raw_graph(&self) -> Result<Value, CliError> {
        let mut session = self.session()?;
        let mut foyer = Foyer::new(self.http()?, &mut session, self.creds, self.verbose);
        foyer.rpc(
            foyer::STRUCTURES,
            foyer::GET_HOME_GRAPH,
            &Value::Array(vec![]),
        )
    }

    /// Devices linked to the account but placed in no home. They show up in
    /// the Home app under "Linked to you" and answer to no room command.
    pub fn unplaced(&self, home_id: &str) -> Result<Vec<homegraph::Device>, CliError> {
        let mut session = self.session()?;
        let mut foyer = Foyer::new(self.http()?, &mut session, self.creds, self.verbose);
        let raw = foyer.rpc(
            foyer::HOME_DEVICES,
            foyer::LIST_UNASSIGNED_DEVICES,
            &Value::Array(vec![Value::String(home_id.to_string())]),
        )?;
        Ok(homegraph::parse_device_list(&raw))
    }

    /// Online state for `ids`, as the partner last reported it to Google
    /// (`GetTraits`, in batches). A device missing from the map reported no
    /// status.
    pub fn online(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, bool>, CliError> {
        let mut session = self.session()?;
        let mut foyer = Foyer::new(self.http()?, &mut session, self.creds, self.verbose);
        let mut out = std::collections::HashMap::new();
        for chunk in ids.chunks(60) {
            let raw = foyer.rpc(
                foyer::HOME_CONTROL,
                foyer::GET_TRAITS,
                &homegraph::get_traits_request(chunk),
            )?;
            out.extend(homegraph::parse_online(&raw));
        }
        Ok(out)
    }

    /// Full trait state for `ids` (`{id: {trait: {field: value}}}`), batched.
    pub fn states(&self, ids: &[&str]) -> Result<serde_json::Map<String, Value>, CliError> {
        let mut session = self.session()?;
        let mut foyer = Foyer::new(self.http()?, &mut session, self.creds, self.verbose);
        let mut out = serde_json::Map::new();
        for chunk in ids.chunks(60) {
            let raw = foyer.rpc(
                crate::traits::SERVICE,
                crate::traits::GET_TRAITS,
                &crate::traits::get_traits(chunk),
            )?;
            out.extend(crate::traits::parse_states(&raw));
        }
        Ok(out)
    }

    /// Homes to act on: `--home` if given, else the configured default, else all.
    pub fn homes<'g>(
        &self,
        graph: &'g HomeGraph,
        flag: Option<&str>,
    ) -> Result<Vec<&'g Home>, CliError> {
        let sel = flag.or(self.cfg.home.as_deref());
        graph.select_home(sel)
    }
}

/// The mutation gate (SPEC §1.3). Call **before** any network work when the
/// answer is knowable up front: with `--force` it passes; non-interactive
/// without it is exit 6, so a driver never hangs on a prompt.
pub fn require_confirmable(force: bool, interactive: bool, what: &str) -> Result<(), CliError> {
    if force || interactive {
        Ok(())
    } else {
        Err(CliError::ConfirmationRequired(format!(
            "{what} — pass --force to run non-interactively"
        )))
    }
}

/// Interactive yes/no on stderr; only reached when `require_confirmable`
/// passed without `--force`.
pub fn confirm(force: bool, prompt: &str) -> Result<(), CliError> {
    if force {
        return Ok(());
    }
    eprint!("{prompt} [y/N] ");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| CliError::Other(format!("reading confirmation: {e}")))?;
    if matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CliError::ConfirmationRequired("cancelled".into()))
    }
}

/// Refuse to send a write while the wire layout is still a placeholder.
pub fn require_layout() -> Result<(), CliError> {
    if crate::spaces::LAYOUT_CONFIRMED {
        Ok(())
    } else {
        Err(CliError::Other(
            "this build's room-write layout is not yet confirmed against Google; see docs/api.md"
                .into(),
        ))
    }
}

/// Devices per partner integration, most first, labelled from the graph's
/// project table.
pub fn agent_counts(
    ctx: &Ctx,
    graph: &HomeGraph,
    home: Option<&str>,
) -> Result<Vec<Value>, CliError> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for h in ctx.homes(graph, home)? {
        for d in &h.devices {
            let a = d.agent_id.clone().unwrap_or_else(|| "(none)".into());
            match counts.iter_mut().find(|(k, _)| *k == a) {
                Some((_, n)) => *n += 1,
                None => counts.push((a, 1)),
            }
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(counts
        .into_iter()
        .map(|(agent_id, devices)| {
            let label = graph
                .project_types
                .iter()
                .find(|c| c.code == agent_id)
                .map(|c| c.name.clone());
            serde_json::json!({"agent_id": agent_id, "devices": devices, "label": label})
        })
        .collect())
}

/// Emit a list DTO: `{"schema": "<record>-list/v1", "items": [...]}` in JSON
/// mode, a pipe table of `columns` otherwise.
pub fn emit_list(json: bool, record: &str, items: Vec<Value>, columns: &[&str]) {
    let payload = serde_json::json!({ "items": items });
    output::emit(json, &format!("{record}-list"), payload, |v| {
        let rows = output::rows_of(v, "items");
        if rows.is_empty() {
            eprintln!("(no {record}s)");
        } else {
            output::table(&output::table_view(&rows, columns));
        }
    });
}

/// Emit a single resource: the DTO in JSON mode, a key/value block otherwise.
pub fn emit_one(json: bool, schema: &str, value: Value) {
    output::emit(json, schema, value, |v| output::kv(v, 0));
}

/// A device row flattened for tables: traits collapse to their short names.
pub fn device_row(home: &Home, d: &homegraph::Device) -> Value {
    let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert("home".into(), Value::String(home.name.clone()));
        let short: Vec<String> = d
            .traits
            .iter()
            .map(|t| t.rsplit('.').next().unwrap_or(t).to_string())
            .collect();
        m.insert("trait_names".into(), Value::String(short.join(",")));
        if let Some(k) = d.kind.as_deref() {
            m.insert(
                "type".into(),
                Value::String(k.rsplit('.').next().unwrap_or(k).to_string()),
            );
        }
    }
    v
}
