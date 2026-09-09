//! Contract tests over the Foyer wire shapes in `tests/fixtures/`: the
//! positional slots the codec reads must be where the codec expects them, and
//! every fixture must be scrubbed per `tests/fixtures/README.md`.

use serde_json::Value;

fn fixture(rel: &str) -> Value {
    let path = format!("{}/tests/fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parsing {path}: {e}"))
}

fn at(v: &Value, i: usize) -> &Value {
    v.get(i).unwrap_or(&Value::Null)
}

#[test]
fn home_graph_slots_the_codec_reads_are_present() {
    let v = fixture("get-home-graph.json");
    let homes = at(&v, 1).as_array().expect("slot 1 is the home list");
    assert!(!homes.is_empty());
    for home in homes {
        assert!(at(home, 0).is_string(), "home id at [0]");
        assert!(at(home, 1).is_string(), "home name at [1]");
        let rooms = at(home, 5).as_array().expect("rooms at [5]");
        for room in rooms {
            assert!(at(room, 0).is_string(), "room id at [0]");
            assert!(at(room, 2).is_string(), "room name at [2]");
            assert!(at(at(room, 3), 0).is_string(), "room category at [3][0]");
            if let Some(members) = at(room, 4).as_array() {
                for m in members {
                    assert!(at(at(m, 0), 0).is_string(), "member device id at [0][0]");
                }
            }
        }
        let devices = at(home, 6).as_array().expect("devices at [6]");
        assert!(!devices.is_empty());
        for d in devices {
            assert!(at(at(d, 0), 0).is_string(), "device id at [0][0]");
            assert!(at(at(at(d, 0), 1), 0).is_string(), "agent id at [0][1][0]");
            assert!(
                at(at(at(d, 0), 1), 1).is_string(),
                "partner id at [0][1][1]"
            );
            assert!(at(d, 3).is_string(), "device name at [3]");
            assert!(at(d, 5).is_string(), "device type at [5]");
            assert!(at(d, 6).is_array(), "traits at [6]");
            assert!(
                at(d, 27).is_null(),
                "local_auth_token slot [27] must be scrubbed"
            );
        }
    }
    assert!(at(&v, 3).is_array(), "room types at [3]");
    assert!(at(&v, 6).is_array(), "device types at [6]");
}

#[test]
fn fixtures_carry_only_dummy_identities() {
    let v = fixture("get-home-graph.json");
    let mut strings = Vec::new();
    collect_strings(&v, &mut strings);
    for s in strings {
        if s.contains('@') {
            assert!(
                s.ends_with("@example.com"),
                "email `{s}` is not an example.com dummy"
            );
        }
        if s.starts_with("home-") || s.starts_with("room-") || s.starts_with("dev-") {
            continue;
        }
        assert!(
            !looks_like_uuid(&s),
            "`{s}` looks like a real UUID; ids must be `home-…`/`room-…`/`dev-…`"
        );
        assert!(
            !s.starts_with("ya29") && !s.starts_with("aas_et/") && !s.starts_with("oauth2_4/"),
            "`{s}` looks like a live token"
        );
    }
}

fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

fn looks_like_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}
