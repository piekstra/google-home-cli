//! `ghome mcp` end to end: the real binary over stdio, as Claude Code and Claude Desktop launch it.
//! The credential lives only in the keychain, which no test may read, so a config that names no
//! account stops every call before it. `src/mcp/tests.rs` runs the live path against a stub of
//! Foyer.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

const TOOLS: [&str; 4] = [
    "ghome_devices",
    "ghome_device_state",
    "ghome_device_set",
    "ghome_room_lights",
];

fn ghome() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ghome"));
    c.env("GHOME_CONFIG", "/nonexistent/ghome-test-config.json");
    c
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Server {
    fn start() -> Self {
        let mut child = ghome()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("ghome starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            stdin,
            lines,
        };
        server.request(
            0,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        server
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .expect("a response within 30 s");
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line}"));
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn tools_are_listed_with_their_annotations_and_no_credential() {
    let mut server = Server::start();
    let list = server.request(1, "tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOLS);
    for read in &tools[..2] {
        assert_eq!(read["annotations"]["readOnlyHint"], true, "{read}");
    }
    for write in &tools[2..] {
        assert_eq!(write["annotations"]["readOnlyHint"], false, "{write}");
        assert_eq!(
            write["_meta"]["anthropic/requiresUserInteraction"], true,
            "{write}"
        );
    }
}

#[test]
fn the_list_flag_prints_the_tools() {
    let out = ghome().args(["mcp", "--list-tools"]).output().unwrap();
    assert!(out.status.success());
    let got: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(got["tools"].as_array().unwrap().len(), TOOLS.len());
}

/// These come back from the server's blocking pool, with no keychain read before them.
#[test]
fn a_bad_call_is_a_usage_error_through_the_real_server() {
    let mut server = Server::start();
    for (id, (tool, args)) in [
        ("ghome_device_state", json!({})),
        (
            "ghome_device_set",
            json!({"device": "Office Lamp", "power": "maybe"}),
        ),
        (
            "ghome_device_set",
            json!({"device": "Office Lamp", "power": "on", "all": true}),
        ),
        (
            "ghome_device_set",
            json!({"device": "Office Lamp", "brightness": 0}),
        ),
        (
            "ghome_room_lights",
            json!({"room": "Office", "color_temp_k": 2700, "color": "red"}),
        ),
        ("ghome_room_lights", json!({"room": "Office"})),
    ]
    .into_iter()
    .enumerate()
    {
        let call = server.request(
            id as u64 + 1,
            "tools/call",
            json!({"name": tool, "arguments": args}),
        );
        assert_eq!(call["result"]["isError"], true, "{tool}: {call}");
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("usage:"), "{tool}: {text}");
    }
}

/// A good call gets as far as the config, which names no account, and asks for the sign-in.
#[test]
fn no_account_asks_for_the_sign_in_before_the_keychain() {
    let mut server = Server::start();
    let call = server.request(
        1,
        "tools/call",
        json!({"name": "ghome_devices", "arguments": {}}),
    );
    assert_eq!(call["result"]["isError"], true, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("auth:"), "{text}");
    assert!(text.contains("ghome auth login"), "{text}");
}

#[test]
fn stdout_stays_protocol_only_and_a_bad_tool_is_an_error() {
    let mut server = Server::start();
    let call = server.request(
        1,
        "tools/call",
        json!({"name": "ghome_remove_device", "arguments": {}}),
    );
    assert!(call.get("error").is_some(), "{call}");
}
