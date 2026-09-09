//! `ghome` — audit and fix which room each smart device lives in, across
//! Google Home and the vendor apps behind it. Conforms to piekstra-cli/1.

mod audit;
mod b64;
mod commands;
mod config;
mod foyer;
mod gpsoauth;
mod grpc;
mod homegraph;
mod session;
mod spaces;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_auth::{AuthMethod, AuthStatus, LoginArgs, LogoutArgs, SetCredentialArgs};
use pk_cli_config::ConfigStore;
use pk_cli_core::dates::fmt_rfc3339;
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{output, CliError, CommonArgs};
use pk_cli_secrets::{CredentialStore, Secret};
use pk_cli_selfupdate::{SelfUpdateArgs, Updater};

use commands::{
    agents::AgentsCmd, audit::AuditArgs, devices::DevicesCmd, homes::HomesCmd, rooms::RoomsCmd, Ctx,
};
use config::Config;
use session::Session;

pub const BIN: &str = "ghome";
const REPO: &str = "piekstra/google-home-cli";

/// Google Home rooms and devices from the terminal (conforms to piekstra-cli/1).
#[derive(Parser, Debug)]
#[command(name = BIN, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    /// Override the config file location.
    #[arg(long, global = true, value_name = "PATH", env = "GHOME_CONFIG")]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Credential management and session status.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Non-secret settings.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Homes (structures) on the account.
    #[command(subcommand)]
    Homes(HomesCmd),
    /// Rooms and what's in them.
    #[command(subcommand)]
    Rooms(RoomsCmd),
    /// Devices and where they live.
    #[command(subcommand)]
    Devices(DevicesCmd),
    /// Partner integrations (Govee, Kasa, …) linked to the account.
    #[command(subcommand)]
    Agents(AgentsCmd),
    /// Compare every device's room against where it should be (room-audit/v1).
    Audit(AuditArgs),
    /// Raw Foyer RPC passthrough: `api POST <Service>/<Method> --data '[...]'`.
    Api(commands::api::RawArgs),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// Exchange a browser `oauth_token` for a stored Google credential.
    Login(LoginArgs),
    /// Report credential/session state (auth-status/v1).
    Status,
    /// Clear the cached session; --forget also removes the stored credential.
    Logout(LogoutArgs),
    /// Raw keychain write of a master token (`aas_et/…`) for headless setup.
    SetCredential(SetCredentialArgs),
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Print the resolved config file path.
    Path,
    /// Show the effective configuration.
    Show,
    /// Set a config key (`username`, `home`).
    Set { key: String, value: String },
    /// Remove a config key.
    Unset { key: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    let store = ConfigStore::new(BIN).with_override(cli.config.clone());
    let creds = CredentialStore::for_binary(BIN);

    // Config-only and offline commands first: they must never touch the
    // keychain or the network.
    match &cli.command {
        Command::Config(cmd) => return config_cmd(cli, cmd, &store),
        Command::SelfUpdate(args) => {
            return Updater {
                repo: REPO.into(),
                binary: BIN.into(),
                target: env!("BUILD_TARGET").into(),
                current: env!("CARGO_PKG_VERSION").into(),
            }
            .run(args, cli.common.json, cli.common.quiet)
        }
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
            return Ok(());
        }
        Command::Info => {
            let info = CliInfo::new(
                BIN,
                env!("CARGO_PKG_VERSION"),
                &format!("https://github.com/{REPO}"),
                AuthInfo {
                    required: true,
                    method: "browser-session".into(),
                    login_hint: Some(format!("{BIN} auth login")),
                },
                &["homes", "rooms", "devices", "agents", "audit", "api"],
            );
            output::json(&serde_json::to_value(&info).unwrap());
            return Ok(());
        }
        _ => {}
    }

    let cfg: Config = store.load()?;
    let ctx = Ctx {
        json: cli.common.json,
        verbose: cli.common.verbose,
        interactive: cli.common.interactive(),
        store: &store,
        creds: &creds,
        cfg,
    };

    match &cli.command {
        Command::Auth(cmd) => auth(cli, cmd, &ctx),
        Command::Homes(cmd) => commands::homes::run(&ctx, cmd),
        Command::Rooms(cmd) => commands::rooms::run(&ctx, cmd),
        Command::Devices(cmd) => commands::devices::run(&ctx, cmd),
        Command::Agents(cmd) => commands::agents::run(&ctx, cmd),
        Command::Audit(args) => {
            // Validate the expectations file before any credential is read.
            let expectations = commands::audit::load_expectations(args.expect.as_deref())?;
            commands::audit::run(&ctx, args, expectations)
        }
        Command::Api(args) => {
            let body = commands::api::validate(args)?;
            commands::api::run(&ctx, args, body)
        }
        Command::Config(_)
        | Command::SelfUpdate(_)
        | Command::Completions { .. }
        | Command::Info => {
            unreachable!("handled above")
        }
    }
}

const LOGIN_HELP: &str = "\
How to get the token (Google no longer accepts passwords from non-browser clients):
  1. In a browser, open https://accounts.google.com/EmbeddedSetup and sign in
     with the Google account that owns your home.
  2. Click \"I agree\" on the consent page. The page will look stuck — expected.
  3. Open DevTools → Application → Cookies → https://accounts.google.com and
     copy the value of the `oauth_token` cookie (it starts with oauth2_4/).
The token is single-use and expires in minutes; paste it now.";

fn auth(cli: &Cli, cmd: &AuthCmd, ctx: &Ctx) -> Result<(), CliError> {
    let cfg = &ctx.cfg;
    let creds = ctx.creds;

    // `auth status` must work logged-out (SPEC §1.2), so it is the one branch
    // that tolerates a missing identity.
    if let AuthCmd::Status = cmd {
        let mut status = AuthStatus::new(true, false, AuthMethod::BrowserSession);
        status.account = cfg.home.clone();
        match (&cfg.username, &cfg.android_id) {
            (Some(user), Some(android_id)) => {
                status.username = Some(user.clone());
                match Session::load(creds, user, android_id) {
                    Ok(s) => {
                        status.authenticated = true;
                        status.credential_in_keychain = Some(true);
                        status.session_valid = Some(s.bearer_is_valid());
                        status.expires_at = s.bearer_expires_at().map(|t| fmt_rfc3339(t as i64));
                    }
                    Err(CliError::Auth(_)) => status.credential_in_keychain = Some(false),
                    Err(e) => return Err(e),
                }
            }
            (user, _) => {
                status.username = user.clone();
                status.credential_in_keychain = Some(false);
            }
        }
        status.emit(cli.common.json);
        return Ok(());
    }

    match cmd {
        AuthCmd::Login(args) => {
            let user = match &cfg.username {
                Some(u) => u.clone(),
                None if args.non_interactive || !cli.common.interactive() => {
                    return Err(CliError::Usage(format!(
                        "no Google account configured; run `{BIN} config set username you@example.com` first"
                    )))
                }
                None => {
                    eprint!("Google account email: ");
                    let mut s = String::new();
                    std::io::stdin()
                        .read_line(&mut s)
                        .map_err(|e| CliError::Other(format!("reading email: {e}")))?;
                    let s = s.trim().to_string();
                    if s.is_empty() {
                        return Err(CliError::Usage("an email is required".into()));
                    }
                    s
                }
            };
            if creds.get(&user)?.is_some() && !args.overwrite {
                return Err(CliError::Usage(
                    "a credential is already stored; pass --overwrite to replace it".into(),
                ));
            }
            let prompt = if args.non_interactive {
                None
            } else {
                // The walkthrough only helps someone about to be prompted;
                // a piped token means they already have one.
                if !cli.common.quiet && !args.source.stdin && args.source.from_env.is_none() {
                    eprintln!("{LOGIN_HELP}");
                }
                Some("oauth_token")
            };
            let pasted = args.source.read(prompt)?;
            let token = pasted.expose().trim().to_string();

            let android_id = cfg
                .android_id
                .clone()
                .unwrap_or_else(gpsoauth::new_android_id);
            let client = ctx.http()?;
            let master = if token.starts_with(gpsoauth::MASTER_TOKEN_PREFIX) {
                Secret::new(token)
            } else if token.starts_with(gpsoauth::OAUTH_TOKEN_PREFIX) {
                if !cli.common.quiet {
                    eprintln!("exchanging oauth_token for a master token…");
                }
                Secret::new(gpsoauth::exchange_oauth_token(
                    &client,
                    &token,
                    &android_id,
                    &user,
                )?)
            } else {
                return Err(CliError::Usage(format!(
                    "expected an oauth_token ({}…) or a master token ({}…)",
                    gpsoauth::OAUTH_TOKEN_PREFIX,
                    gpsoauth::MASTER_TOKEN_PREFIX
                )));
            };

            let mut session = Session::fresh(&user, &android_id, master);
            if !args.no_verify {
                if !cli.common.quiet {
                    eprintln!("verifying against Google Home…");
                }
                let mut foyer = foyer::Foyer::new(client, &mut session, creds, cli.common.verbose);
                let raw = foyer.rpc(
                    foyer::STRUCTURES,
                    foyer::GET_HOME_GRAPH,
                    &serde_json::Value::Array(vec![]),
                )?;
                let graph = homegraph::parse(&raw);
                if !cli.common.quiet {
                    let names: Vec<&str> = graph.homes.iter().map(|h| h.name.as_str()).collect();
                    eprintln!(
                        "ok: {} home(s) visible ({})",
                        graph.homes.len(),
                        names.join(", ")
                    );
                }
            }
            session.persist_master(creds)?;

            // Persist the identity the credential is bound to.
            let mut cfg: Config = ctx.store.load()?;
            cfg.username = Some(user);
            cfg.android_id = Some(android_id);
            ctx.store.save(&cfg)?;
            if !cli.common.quiet {
                eprintln!("credential stored in the OS keychain ({})", creds.service());
            }
            Ok(())
        }
        AuthCmd::Status => unreachable!("handled above"),
        AuthCmd::Logout(args) => {
            let user = cfg.username.clone().ok_or_else(|| {
                CliError::Usage("nothing to log out of: no Google account configured".into())
            })?;
            Session::logout(creds, &user, args.forget)?;
            if args.forget {
                ctx.store.clear()?;
            }
            if !cli.common.quiet {
                eprintln!("logged out");
            }
            Ok(())
        }
        AuthCmd::SetCredential(args) => {
            let user = cfg.username.clone().ok_or_else(|| {
                CliError::Usage(format!(
                    "run `{BIN} config set username you@example.com` first"
                ))
            })?;
            if creds.get(&user)?.is_some() && !args.overwrite {
                return Err(CliError::Usage(
                    "a credential is already stored; pass --overwrite to replace it".into(),
                ));
            }
            let secret = args.source.read(None)?;
            if !secret.expose().starts_with(gpsoauth::MASTER_TOKEN_PREFIX) {
                return Err(CliError::Usage(format!(
                    "set-credential takes a master token ({}…); use `auth login` for an oauth_token",
                    gpsoauth::MASTER_TOKEN_PREFIX
                )));
            }
            creds.set(&user, &secret)?;
            if cfg.android_id.is_none() {
                let mut cfg: Config = ctx.store.load()?;
                cfg.android_id = Some(gpsoauth::new_android_id());
                ctx.store.save(&cfg)?;
            }
            if !cli.common.quiet {
                eprintln!("credential stored");
            }
            Ok(())
        }
    }
}

fn config_cmd(cli: &Cli, cmd: &ConfigCmd, store: &ConfigStore) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            println!("{}", store.path()?.display());
            Ok(())
        }
        ConfigCmd::Show => {
            let cfg: Config = store.load()?;
            let v = serde_json::to_value(&cfg).unwrap_or_default();
            if cli.common.json {
                output::json(&v);
            } else {
                output::render(&v);
            }
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg: Config = store.load()?;
            cfg.set(key, value).map_err(CliError::Usage)?;
            store.save(&cfg)
        }
        ConfigCmd::Unset { key } => {
            let mut cfg: Config = store.load()?;
            cfg.unset(key).map_err(CliError::Usage)?;
            store.save(&cfg)
        }
    }
}
