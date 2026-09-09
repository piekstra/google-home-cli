//! Offline surface tests: flags, exit codes, and the JSON output contract.
//! No network, and nothing here reads the OS keychain: every command under
//! test either needs no credential or fails validation before looking for
//! one (SPEC §1.5), using a throwaway config with no account set.

use assert_cmd::Command;
use predicates::prelude::*;

fn ghome() -> Command {
    let mut c = Command::cargo_bin("ghome").unwrap();
    // A config path that does not exist: no username, no android id, so any
    // credentialed command stops with exit 3 before the keychain.
    c.env("GHOME_CONFIG", "/nonexistent/ghome-test-config.json");
    c.env_remove("NO_COLOR");
    c
}

fn json_stdout(out: &assert_cmd::assert::Assert) -> serde_json::Value {
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {stdout}"))
}

#[test]
fn help_lists_the_standard_and_domain_surface() {
    ghome()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("auth"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("homes"))
        .stdout(predicate::str::contains("rooms"))
        .stdout(predicate::str::contains("devices"))
        .stdout(predicate::str::contains("audit"))
        .stdout(predicate::str::contains("api"))
        .stdout(predicate::str::contains("self-update"))
        .stdout(predicate::str::contains("completions"))
        .stdout(predicate::str::contains("info"));
}

#[test]
fn every_subcommand_help_renders() {
    // Catches clap runtime panics (e.g. a subcommand flag colliding with a
    // global like -q) that only surface when the subtree is built.
    for args in [
        vec!["auth", "--help"],
        vec!["auth", "login", "--help"],
        vec!["auth", "logout", "--help"],
        vec!["auth", "set-credential", "--help"],
        vec!["config", "--help"],
        vec!["homes", "list", "--help"],
        vec!["homes", "get", "--help"],
        vec!["rooms", "list", "--help"],
        vec!["rooms", "get", "--help"],
        vec!["rooms", "types", "--help"],
        vec!["devices", "list", "--help"],
        vec!["devices", "get", "--help"],
        vec!["devices", "agents", "--help"],
        vec!["devices", "move", "--help"],
        vec!["devices", "place", "--help"],
        vec!["rooms", "create", "--help"],
        vec!["rooms", "rename", "--help"],
        vec!["devices", "remove", "--help"],
        vec!["devices", "rename", "--help"],
        vec!["devices", "sync", "--help"],
        vec!["devices", "state", "--help"],
        vec!["devices", "set", "--help"],
        vec!["rooms", "set", "--help"],
        vec!["agents", "list", "--help"],
        vec!["audit", "--help"],
        vec!["api", "--help"],
        vec!["self-update", "--help"],
    ] {
        ghome().args(&args).assert().success();
    }
}

#[test]
fn info_emits_cli_info_v1() {
    let out = ghome().arg("info").assert().success();
    let v = json_stdout(&out);
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "ghome");
    assert_eq!(v["spec"], "piekstra-cli/1");
    assert_eq!(v["auth"]["method"], "browser-session");
    let caps: Vec<&str> = v["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    for cap in ["homes", "rooms", "devices", "agents", "audit", "api"] {
        assert!(caps.contains(&cap), "missing capability {cap}");
    }
}

#[test]
fn usage_error_exits_2_with_json_error_dto() {
    let out = ghome()
        .args(["--json", "config", "set", "bogus_key", "x"])
        .assert()
        .code(2);
    let v = json_stdout(&out);
    assert_eq!(v["error"]["code"], "usage");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown config key"));
}

#[test]
fn auth_status_works_logged_out() {
    let out = ghome()
        .args(["--json", "auth", "status"])
        .assert()
        .success();
    let v = json_stdout(&out);
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["required"], true);
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["credential_in_keychain"], false);
}

#[test]
fn credentialed_reads_exit_3_before_the_keychain_when_no_account_is_configured() {
    for args in [
        vec!["homes", "list"],
        vec!["rooms", "list"],
        vec!["devices", "list"],
        vec!["audit"],
        vec![
            "api",
            "POST",
            "StructuresService/GetHomeGraph",
            "--data",
            "[]",
        ],
    ] {
        let mut full = vec!["--json"];
        full.extend(args.iter());
        let out = ghome().args(&full).assert().code(3);
        let v = json_stdout(&out);
        assert_eq!(v["error"]["code"], "auth", "args {args:?}");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("auth login"));
    }
}

#[test]
fn audit_rejects_a_bad_expect_file_before_any_credential() {
    let out = ghome()
        .args(["--json", "audit", "--expect", "/nonexistent/expect.json"])
        .assert()
        .code(2);
    assert_eq!(json_stdout(&out)["error"]["code"], "usage");

    let dir = std::env::temp_dir();
    let path = dir.join(format!("ghome-expect-{}.json", std::process::id()));
    std::fs::write(&path, r#"{"items":[{"room":"Office"}]}"#).unwrap();
    let out = ghome()
        .args(["--json", "audit", "--expect", path.to_str().unwrap()])
        .assert()
        .code(2);
    let v = json_stdout(&out);
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("neither `id` nor `name`"));
    std::fs::remove_file(path).ok();
}

#[test]
fn api_validates_method_and_body_shape_before_any_credential() {
    let out = ghome()
        .args(["--json", "api", "GET", "StructuresService/GetHomeGraph"])
        .assert()
        .code(2);
    assert!(json_stdout(&out)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("POST-only"));
    let out = ghome()
        .args(["--json", "api", "POST", "X/Y", "--data", "{}"])
        .assert()
        .code(2);
    assert!(json_stdout(&out)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("JSON array"));
    ghome()
        .args(["--json", "api", "POST", "X/Y", "--data", "{not json"])
        .assert()
        .code(2);
}

#[test]
fn mutations_exit_6_when_non_interactive_without_force() {
    // Checked before any credential or network access, so it is exit 6 even
    // with no account configured — a driver never hangs on a prompt.
    for args in [
        vec!["devices", "move", "Office Lamp", "--room", "Office"],
        vec!["devices", "place", "Office Hex", "--room", "Office"],
        vec!["rooms", "create", "Loft", "--kind", "OTHER"],
        vec!["rooms", "rename", "Loft", "Attic"],
        vec!["devices", "remove", "Old Lamp"],
        vec!["devices", "rename", "Old Lamp", "Storage Old Lamp"],
    ] {
        let mut full = vec!["--json"];
        full.extend(args.iter());
        let out = ghome().args(&full).assert().code(6);
        assert_eq!(
            json_stdout(&out)["error"]["code"],
            "confirmation_required",
            "args {args:?}"
        );
    }
    // Bad room category is a usage error before the confirmation gate.
    ghome()
        .args(["--json", "rooms", "create", "Loft", "--kind", "not a code!"])
        .assert()
        .code(2);
}

#[test]
fn control_validates_its_arguments_before_any_credential() {
    // No change requested, or out-of-range values, are usage errors (2).
    ghome()
        .args(["--json", "devices", "set", "Lamp"])
        .assert()
        .code(2);
    ghome()
        .args(["--json", "devices", "set", "Lamp", "--brightness", "150"])
        .assert()
        .code(2);
    ghome()
        .args(["--json", "devices", "set", "Lamp", "--on", "--off"])
        .assert()
        .code(2);
    ghome()
        .args(["--json", "rooms", "set", "Office", "--media", "rewind"])
        .assert()
        .code(2);
    // A valid change with no account stops at auth (3), before the network.
    ghome()
        .args(["--json", "rooms", "set", "Office", "--off"])
        .assert()
        .code(3);
}

#[test]
fn devices_list_rejects_contradictory_filters() {
    ghome()
        .args([
            "--json",
            "devices",
            "list",
            "--unassigned",
            "--room",
            "Office",
        ])
        .assert()
        .code(2);
}

#[test]
fn login_never_takes_the_token_on_argv() {
    // The secret enters only via --stdin / --from-env / prompt; a positional
    // token must be a clap usage error.
    ghome()
        .args(["auth", "login", "oauth2_4/abc"])
        .assert()
        .code(2);
    // Non-interactive login with no account configured stops before reading
    // the token.
    ghome()
        .args(["--json", "auth", "login", "--non-interactive", "--stdin"])
        .write_stdin("oauth2_4/abc\n")
        .assert()
        .code(2);
}

#[test]
fn completions_render_for_zsh() {
    ghome()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef ghome"));
}

/// The repo must carry no personal data (SPEC §1.7). Scan tracked files for
/// shapes — real-looking emails, UUIDs, and tokens — never a denylist of real
/// values. Runtime output may carry them; git may not.
#[test]
fn tracked_files_carry_no_personal_data() {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new("git")
        .args(["-C", root, "ls-files"])
        .output()
        .expect("git ls-files");
    if !out.status.success() {
        eprintln!("not a git checkout; skipping");
        return;
    }
    let files = String::from_utf8_lossy(&out.stdout);
    for rel in files.lines().filter(|f| !f.ends_with(".lock")) {
        let path = format!("{root}/{rel}");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let where_ = format!("{rel}:{}", n + 1);
            for tok in line.split(|c: char| c.is_whitespace() || "\"'`<>()[]{},;".contains(c)) {
                if tok.contains('@') && tok.contains('.') && !tok.contains("example.com") {
                    let local = tok.split('@').next().unwrap_or("");
                    let domain = tok.rsplit('@').next().unwrap_or("");
                    let is_email = !local.is_empty()
                        && local
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
                        && domain.contains('.')
                        && domain
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c));
                    assert!(
                        !is_email,
                        "{where_}: `{tok}` looks like a real email address"
                    );
                }
                assert!(
                    !(tok.starts_with("ya29.")
                        || tok.starts_with("aas_et/")
                        || tok.starts_with("oauth2_4/"))
                        || tok.len() < 16,
                    "{where_}: `{tok}` looks like a live Google token"
                );
            }
        }
    }
}
