//! Room audit: for every device in Google Home, where is it, and where
//! should it be?
//!
//! "Should" comes from one of two places, in priority order:
//! 1. An explicit expectation (`device-rooms/v1`, from `--expect`) — the
//!    shape the vendor CLIs emit, joined on the partner device id and then
//!    the device name.
//! 2. The device's own name: a device called "Office Lamp" that sits in the
//!    Living Room is almost always misfiled. The longest room name found in
//!    the device name wins, so "Living Room Lamp" prefers Living Room over a
//!    room called "Room".

use serde::{Deserialize, Serialize};
use serde_json::Value;

use pk_cli_core::CliError;

use crate::homegraph::{Device, Room};

/// One expected placement, as a vendor CLI reports it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Expectation {
    /// The vendor's own device id (matched against `partner_device_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub room: String,
    /// Which vendor reported it (`govee`, `tplink`, …); free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Whether the vendor can reach the device from its cloud. `false`
    /// (Bluetooth-only) means Google Home can never see it, so a missing
    /// match is expected rather than a problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// In the expected room (or no expectation and the name doesn't disagree).
    Ok,
    /// In a room, but not the expected one.
    Mismatch,
    /// Not in any room.
    Unassigned,
    /// An expectation that matched no Google Home device.
    Unmatched,
    /// An expectation for a device the vendor can't expose to Google Home
    /// (Bluetooth-only); informational.
    LocalOnly,
    /// Linked to the account but placed in no home, so no room can hold it
    /// until it is added to the home.
    Unplaced,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_room: Option<String>,
    /// `expect` when an explicit expectation decided it, `name` when the
    /// device's name did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partner_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub ok: usize,
    pub mismatch: usize,
    pub unassigned: usize,
    pub unplaced: usize,
    pub unmatched: usize,
    pub local_only: usize,
}

/// Parse a `device-rooms/v1` document (or a bare array of expectations).
pub fn parse_expectations(raw: &str) -> Result<Vec<Expectation>, CliError> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|e| CliError::Usage(format!("--expect is not valid JSON: {e}")))?;
    let items = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(m) => m
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                CliError::Usage(
                    "--expect must be a device-rooms/v1 document with `items`, or a bare array"
                        .into(),
                )
            })?,
        _ => {
            return Err(CliError::Usage(
                "--expect must be a JSON array or object".into(),
            ))
        }
    };
    let parsed: Vec<Expectation> = items
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|e| CliError::Usage(format!("--expect item is malformed: {e}")))?;
    if let Some(bad) = parsed.iter().find(|e| e.id.is_none() && e.name.is_none()) {
        return Err(CliError::Usage(format!(
            "--expect item for room `{}` has neither `id` nor `name`",
            bad.room
        )));
    }
    Ok(parsed)
}

/// Vendor ids differ only in cosmetics across systems (`AA:BB` vs `aabb`),
/// so compare them stripped of punctuation and case.
fn norm_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn norm_name(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The room whose name appears in the device name. The device's current
/// room wins if it matches at all ("Storage Bathroom Light" in Storage is
/// filed on purpose); otherwise the longest match.
pub fn room_from_name<'a>(
    name: &str,
    current: Option<&str>,
    rooms: &[&'a Room],
) -> Option<&'a Room> {
    let n = norm_name(name);
    let matches: Vec<&Room> = rooms
        .iter()
        .copied()
        .filter(|r| {
            let rn = norm_name(&r.name);
            !rn.is_empty() && n.contains(&rn)
        })
        .collect();
    if let Some(cur) = current {
        if let Some(r) = matches
            .iter()
            .find(|r| norm_name(&r.name) == norm_name(cur))
        {
            return Some(r);
        }
    }
    matches.into_iter().max_by_key(|r| r.name.len())
}

/// Run the audit over one home's devices and rooms. `unplaced` are the
/// account's devices outside the home; they are reported as such whatever
/// the expectations say, because a room can't hold them yet.
pub fn audit(
    home_name: Option<&str>,
    devices: &[&Device],
    unplaced: &[&Device],
    rooms: &[&Room],
    expectations: &[Expectation],
) -> Vec<Finding> {
    let mut used = vec![false; expectations.len()];
    let mut findings = Vec::new();

    for (d, placed) in devices
        .iter()
        .map(|d| (*d, true))
        .chain(unplaced.iter().map(|d| (*d, false)))
    {
        let by_id = d.partner_device_id.as_deref().map(norm_id);
        let by_name = norm_name(&d.name);
        // An id match beats a name match, and a row already claimed by
        // another device is never reused: twins with the same name (two
        // "Island Light"s) each keep their own row.
        let id_hit = expectations.iter().enumerate().find(|(i, e)| {
            !used[*i] && matches!((&by_id, &e.id), (Some(a), Some(b)) if *a == norm_id(b))
        });
        let hit = id_hit.or_else(|| {
            expectations.iter().enumerate().find(|(i, e)| {
                !used[*i] && e.name.as_deref().map(norm_name) == Some(by_name.clone())
            })
        });
        let (expected, source) = match hit {
            Some((i, e)) => {
                used[i] = true;
                (Some(e.room.clone()), Some("expect".to_string()))
            }
            None => (
                room_from_name(&d.name, d.room.as_deref(), rooms).map(|r| r.name.clone()),
                None,
            ),
        };
        let source = source.or_else(|| expected.as_ref().map(|_| "name".to_string()));
        let status = match (placed, &d.room, &expected) {
            (false, _, _) => Status::Unplaced,
            (_, None, _) => Status::Unassigned,
            (_, Some(cur), Some(exp)) if norm_name(cur) != norm_name(exp) => Status::Mismatch,
            _ => Status::Ok,
        };
        findings.push(Finding {
            status,
            device_id: Some(d.id.clone()),
            name: d.name.clone(),
            room: d.room.clone(),
            expected_room: if status == Status::Ok { None } else { expected },
            source: if status == Status::Ok { None } else { source },
            partner_device_id: d.partner_device_id.clone(),
            home: home_name.map(str::to_string),
        });
    }

    for (e, was_used) in expectations.iter().zip(used) {
        if !was_used {
            findings.push(Finding {
                status: if e.cloud == Some(false) {
                    Status::LocalOnly
                } else {
                    Status::Unmatched
                },
                device_id: None,
                name: e.name.clone().or_else(|| e.id.clone()).unwrap_or_default(),
                room: None,
                expected_room: Some(e.room.clone()),
                source: e.source.clone().or_else(|| Some("expect".into())),
                partner_device_id: e.id.clone(),
                home: home_name.map(str::to_string),
            });
        }
    }
    findings
}

pub fn summarize(findings: &[Finding]) -> Summary {
    let mut s = Summary::default();
    for f in findings {
        match f.status {
            Status::Ok => s.ok += 1,
            Status::Mismatch => s.mismatch += 1,
            Status::Unassigned => s.unassigned += 1,
            Status::Unplaced => s.unplaced += 1,
            Status::Unmatched => s.unmatched += 1,
            Status::LocalOnly => s.local_only += 1,
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, name: &str) -> Room {
        Room {
            id: id.into(),
            name: name.into(),
            kind: None,
            device_ids: vec![],
        }
    }

    fn device(id: &str, name: &str, room: Option<&str>, partner: Option<&str>) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            kind: None,
            assigned_kind: None,
            agent_id: None,
            partner_device_id: partner.map(str::to_string),
            model: None,
            traits: vec![],
            room_id: None,
            room: room.map(str::to_string),
        }
    }

    #[test]
    fn name_heuristic_prefers_the_longest_room_name() {
        let living = room("r1", "Living Room");
        let plain = room("r2", "Room");
        let office = room("r3", "Office");
        let rooms = vec![&living, &plain, &office];
        assert_eq!(
            room_from_name("Living Room Lamp", None, &rooms).unwrap().id,
            "r1"
        );
        assert_eq!(
            room_from_name("office desk light", None, &rooms)
                .unwrap()
                .id,
            "r3"
        );
        assert!(room_from_name("Hallway Sensor", None, &rooms).is_none());
        // Filed on purpose: the current room matches, so it wins over a longer match.
        let storage = room("r4", "Storage");
        let rooms2 = vec![&living, &storage];
        assert_eq!(
            room_from_name("Storage Living Room Lamp", Some("Storage"), &rooms2)
                .unwrap()
                .id,
            "r4"
        );
        assert_eq!(
            room_from_name("Storage Living Room Lamp", Some("Office"), &rooms2)
                .unwrap()
                .id,
            "r1"
        );
    }

    #[test]
    fn audit_flags_mismatch_unassigned_and_unmatched() {
        let office = room("r1", "Office");
        let living = room("r2", "Living Room");
        let rooms = vec![&office, &living];
        let misfiled = device("d1", "Office Lamp", Some("Living Room"), Some("AA:BB:CC"));
        let fine = device("d2", "Couch Light", Some("Living Room"), Some("P2"));
        let lost = device("d3", "Desk Plug", None, Some("P3"));
        let devices = vec![&misfiled, &fine, &lost];
        let expectations = vec![
            Expectation {
                id: Some("aabbcc".into()),
                name: None,
                room: "Office".into(),
                source: Some("govee".into()),
                cloud: None,
            },
            Expectation {
                id: None,
                name: Some("couch light".into()),
                room: "Living Room".into(),
                source: None,
                cloud: None,
            },
            Expectation {
                id: Some("ZZZ".into()),
                name: Some("Ghost".into()),
                room: "Attic".into(),
                source: Some("tplink".into()),
                cloud: None,
            },
        ];
        let f = audit(Some("Home"), &devices, &[], &rooms, &expectations);
        assert_eq!(f.len(), 4);
        assert_eq!(f[0].status, Status::Mismatch);
        assert_eq!(f[0].expected_room.as_deref(), Some("Office"));
        assert_eq!(f[0].source.as_deref(), Some("expect"));
        assert_eq!(f[1].status, Status::Ok);
        assert!(f[1].expected_room.is_none());
        assert_eq!(f[2].status, Status::Unassigned);
        assert_eq!(f[3].status, Status::Unmatched);
        assert_eq!(f[3].name, "Ghost");
        let s = summarize(&f);
        assert_eq!(
            (s.ok, s.mismatch, s.unassigned, s.unplaced, s.unmatched),
            (1, 1, 1, 0, 1)
        );
    }

    #[test]
    fn twins_with_the_same_name_each_keep_their_own_row() {
        let kitchen = room("r1", "Kitchen");
        let rooms = vec![&kitchen];
        let a = device("d1", "Island Light", Some("Kitchen"), Some("H6004_AA"));
        let b = device("d2", "Island Light", Some("Kitchen"), Some("H6004_BB"));
        let expectations = vec![
            Expectation {
                id: Some("H6004_AA".into()),
                name: Some("Island Light".into()),
                room: "Kitchen".into(),
                source: None,
                cloud: None,
            },
            Expectation {
                id: Some("H6004_BB".into()),
                name: Some("Island Light".into()),
                room: "Kitchen".into(),
                source: None,
                cloud: None,
            },
        ];
        let f = audit(None, &[&a, &b], &[], &rooms, &expectations);
        assert_eq!(f.len(), 2, "no unmatched row: {f:?}");
        assert!(f.iter().all(|x| x.status == Status::Ok));
    }

    #[test]
    fn without_expectations_the_name_decides() {
        let office = room("r1", "Office");
        let living = room("r2", "Living Room");
        let rooms = vec![&office, &living];
        let misfiled = device("d1", "Office Lamp", Some("Living Room"), None);
        let f = audit(None, &[&misfiled], &[], &rooms, &[]);
        assert_eq!(f[0].status, Status::Mismatch);
        assert_eq!(f[0].source.as_deref(), Some("name"));
    }

    #[test]
    fn unplaced_devices_are_reported_as_such_even_when_expected_somewhere() {
        let office = room("r1", "Office");
        let rooms = vec![&office];
        let outside = device("d9", "Office Hex", None, Some("P9"));
        let f = audit(None, &[], &[&outside], &rooms, &[]);
        assert_eq!(f[0].status, Status::Unplaced);
        assert_eq!(f[0].expected_room.as_deref(), Some("Office"));
        assert_eq!(summarize(&f).unplaced, 1);
    }

    #[test]
    fn bluetooth_only_expectations_are_informational() {
        let f = audit(
            None,
            &[],
            &[],
            &[],
            &[Expectation {
                id: Some("H617A_X".into()),
                name: Some("Shelf Strip".into()),
                room: "Office".into(),
                source: Some("govee".into()),
                cloud: Some(false),
            }],
        );
        assert_eq!(f[0].status, Status::LocalOnly);
        assert_eq!(summarize(&f).local_only, 1);
        assert_eq!(summarize(&f).unmatched, 0);
    }

    #[test]
    fn expectations_parse_from_envelope_or_bare_array() {
        let env = r#"{"schema":"device-rooms/v1","items":[{"id":"x","room":"Office"}]}"#;
        assert_eq!(parse_expectations(env).unwrap().len(), 1);
        let bare = r#"[{"name":"Lamp","room":"Office"}]"#;
        assert_eq!(parse_expectations(bare).unwrap().len(), 1);
        assert!(matches!(parse_expectations("{"), Err(CliError::Usage(_))));
        assert!(matches!(
            parse_expectations(r#"[{"room":"Office"}]"#),
            Err(CliError::Usage(_))
        ));
    }
}
