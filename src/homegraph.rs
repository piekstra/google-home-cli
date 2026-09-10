//! Codec for `StructuresService/GetHomeGraph`: the positional-array response
//! becomes typed homes, rooms and devices. Indices are protobuf field numbers
//! minus one; the ones not in the public proto (room membership, the
//! user-assigned device type) come from live captures — see `docs/api.md`.
//!
//! Every accessor is total: a missing or wrongly-typed slot yields `None` or
//! an empty list, so a Google-side shape change empties a column rather than
//! aborting the CLI.

use serde::Serialize;
use serde_json::Value;

use pk_cli_core::CliError;

#[derive(Debug, Clone, Serialize)]
pub struct Home {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    pub linked_users: Vec<String>,
    pub rooms: Vec<Room>,
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    /// Google's room category code, e.g. `KITCHEN`, `OFFICE`, `OTHER`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub device_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// `action.devices.types.LIGHT` and friends, as reported by the partner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The type the user picked in the Home app, when it overrides `kind`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_kind: Option<String>,
    /// The partner's Google project id — which vendor integration owns this
    /// device (see `agents list`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The device's id in the partner's own system — the join key against
    /// the vendor CLIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partner_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub traits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Code {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HomeGraph {
    pub homes: Vec<Home>,
    pub room_types: Vec<Code>,
    pub device_types: Vec<Code>,
    pub project_types: Vec<Code>,
}

fn at(v: &Value, i: usize) -> &Value {
    v.get(i).unwrap_or(&Value::Null)
}

fn string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(string).collect())
        .unwrap_or_default()
}

fn codes(v: &Value) -> Vec<Code> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(Code {
                        code: string(at(e, 0))?,
                        name: string(at(e, 1)).unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `[deviceId, [agentId, partnerId]]` → (id, agent, partner).
fn device_key(v: &Value) -> Option<(String, Option<String>, Option<String>)> {
    let id = string(at(v, 0))?;
    let pair = at(v, 1);
    Some((id, string(at(pair, 0)), string(at(pair, 1))))
}

/// A device record as it appears in the home graph and in write responses.
pub fn parse_device_value(v: &Value) -> Option<Device> {
    parse_device(v)
}

/// A room/space record: `[id, null, name, [category], members]`.
pub fn parse_room_value(v: &Value) -> Option<Room> {
    parse_room(v)
}

fn parse_room(v: &Value) -> Option<Room> {
    let id = string(at(v, 0))?;
    let device_ids = at(v, 4)
        .as_array()
        .map(|members| {
            members
                .iter()
                .filter_map(|m| device_key(at(m, 0)).map(|k| k.0))
                .collect()
        })
        .unwrap_or_default();
    Some(Room {
        id,
        name: string(at(v, 2)).unwrap_or_default(),
        kind: string(at(at(v, 3), 0)),
        device_ids,
    })
}

fn parse_device(v: &Value) -> Option<Device> {
    let (id, agent_id, partner_device_id) = device_key(at(v, 0))?;
    Some(Device {
        id,
        name: string(at(v, 3)).unwrap_or_default(),
        kind: string(at(v, 5)),
        assigned_kind: string(at(at(v, 20), 0)).filter(|s| !s.trim().is_empty()),
        agent_id,
        partner_device_id,
        model: string(at(at(v, 16), 1)),
        traits: strings(at(v, 6)),
        room_id: None,
        room: None,
    })
}

fn parse_home(v: &Value) -> Option<Home> {
    let id = string(at(v, 0))?;
    let rooms: Vec<Room> = at(v, 5)
        .as_array()
        .map(|a| a.iter().filter_map(parse_room).collect())
        .unwrap_or_default();
    let mut devices: Vec<Device> = at(v, 6)
        .as_array()
        .map(|a| a.iter().filter_map(parse_device).collect())
        .unwrap_or_default();
    for d in &mut devices {
        if let Some(r) = rooms.iter().find(|r| r.device_ids.contains(&d.id)) {
            d.room_id = Some(r.id.clone());
            d.room = Some(r.name.clone());
        }
    }
    let location = at(v, 2);
    Some(Home {
        id,
        name: string(at(v, 1)).unwrap_or_default(),
        timezone: string(at(location, 5)),
        linked_users: at(v, 3)
            .as_array()
            .map(|a| a.iter().filter_map(|u| string(at(u, 0))).collect())
            .unwrap_or_default(),
        rooms,
        devices,
    })
}

/// Parse a `HomeDevicesService/ListUnassignedDevices` response: devices
/// linked to the account but placed in no home. Same device shape as the
/// home graph, under slot 0.
pub fn parse_device_list(raw: &Value) -> Vec<Device> {
    at(raw, 0)
        .as_array()
        .map(|a| a.iter().filter_map(parse_device).collect())
        .unwrap_or_default()
}

/// Parse a raw `GetHomeGraph` response.
pub fn parse(raw: &Value) -> HomeGraph {
    let slot = at(raw, 1);
    // Field 2 is `Home home` in the public proto but arrives as a list of
    // homes on multi-home accounts; accept either.
    let homes = match slot.as_array() {
        Some(a) if a.first().map(Value::is_string).unwrap_or(false) => {
            parse_home(slot).into_iter().collect()
        }
        Some(a) => a.iter().filter_map(parse_home).collect(),
        None => Vec::new(),
    };
    HomeGraph {
        homes,
        room_types: codes(at(raw, 3)),
        device_types: codes(at(raw, 6)),
        project_types: codes(at(raw, 8)),
    }
}

/// `HomeControlService/GetTraits` body: `[[["id1"], ["id2"], …]]`.
pub fn get_traits_request(ids: &[&str]) -> Value {
    Value::Array(vec![Value::Array(
        ids.iter()
            .map(|i| Value::Array(vec![Value::String((*i).to_string())]))
            .collect(),
    )])
}

/// Online flags out of a `GetTraits` response:
/// `[[[ [id], [ ["deviceStatus", [ ["online", [null,null,null,<0|1>]] ]], … ] ], …]]`.
/// Devices the response doesn't mention are absent from the map.
pub fn parse_online(raw: &Value) -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::new();
    for entry in at(raw, 0).as_array().into_iter().flatten() {
        let Some(id) = string(at(at(entry, 0), 0)) else {
            continue;
        };
        for trait_ in at(entry, 1).as_array().into_iter().flatten() {
            if at(trait_, 0).as_str() != Some("deviceStatus") {
                continue;
            }
            for field in at(trait_, 1).as_array().into_iter().flatten() {
                if at(field, 0).as_str() == Some("online") {
                    let v = at(at(field, 1), 3);
                    out.insert(
                        id.clone(),
                        v.as_i64() == Some(1) || v.as_bool() == Some(true),
                    );
                }
            }
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

impl HomeGraph {
    /// Narrow to one home by id or (case-insensitive) name. `None` keeps all.
    pub fn select_home(&self, selector: Option<&str>) -> Result<Vec<&Home>, CliError> {
        let Some(sel) = selector else {
            return Ok(self.homes.iter().collect());
        };
        let want = norm(sel);
        let hit: Vec<&Home> = self
            .homes
            .iter()
            .filter(|h| h.id == sel || norm(&h.name) == want)
            .collect();
        if hit.is_empty() {
            return Err(CliError::NotFound(format!(
                "no home matching `{sel}` (have: {})",
                self.homes
                    .iter()
                    .map(|h| h.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(hit)
    }
}

/// Resolve a user-supplied device reference with the family ladder
/// (`pk_cli_core::resolve::pick`): exact name, exact id, case-insensitive
/// name, then a unique partial name; ambiguity names the candidates.
pub fn resolve_device<'a>(devices: &[&'a Device], query: &str) -> Result<&'a Device, CliError> {
    pk_cli_core::resolve::pick(
        devices,
        query,
        |d| vec![d.id.clone()],
        |d| d.name.as_str(),
        "device",
    )
    .copied()
}

/// Same ladder for rooms.
pub fn resolve_room<'a>(rooms: &[&'a Room], query: &str) -> Result<&'a Room, CliError> {
    pk_cli_core::resolve::pick(
        rooms,
        query,
        |r| vec![r.id.clone()],
        |r| r.name.as_str(),
        "room",
    )
    .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph() -> HomeGraph {
        parse(&json!([
            1700000000,
            [[
                "home-1",
                "My Home",
                [
                    "1 Example St",
                    [0.0, 0.0],
                    null,
                    null,
                    0,
                    "America/New_York"
                ],
                [["owner@example.com"]],
                null,
                [
                    [
                        "room-office",
                        null,
                        "Office",
                        ["OFFICE"],
                        [[["dev-1", ["agent-a", "P1"]]]]
                    ],
                    [
                        "room-living",
                        null,
                        "Living Room",
                        ["LIVING_ROOM"],
                        [[["dev-2", ["agent-b", "P2"]]]]
                    ],
                    ["room-empty", null, "Garage", ["GARAGE"], null]
                ],
                [
                    [
                        ["dev-1", ["agent-a", "P1"]],
                        null,
                        null,
                        "Office Lamp",
                        null,
                        "action.devices.types.LIGHT",
                        ["action.devices.traits.OnOff"],
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        [null, "H6076"]
                    ],
                    [
                        ["dev-2", ["agent-b", "P2"]],
                        null,
                        null,
                        "Desk Plug",
                        null,
                        "action.devices.types.OUTLET",
                        ["action.devices.traits.OnOff"],
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        [null, "KP115"],
                        null,
                        null,
                        null,
                        ["action.devices.types.LIGHT"]
                    ],
                    [
                        ["dev-3", ["agent-b", "P3"]],
                        null,
                        null,
                        "Lonely Plug",
                        null,
                        "action.devices.types.OUTLET",
                        []
                    ]
                ]
            ]],
            null,
            [["OFFICE", "Office"], ["LIVING_ROOM", "Living Room"]],
            null,
            null,
            [["LIGHT", "Light"]],
            null,
            [["agent-a", "Acme Lights"]]
        ]))
    }

    #[test]
    fn homes_rooms_and_membership_parse() {
        let g = graph();
        assert_eq!(g.homes.len(), 1);
        let h = &g.homes[0];
        assert_eq!(h.name, "My Home");
        assert_eq!(h.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(h.linked_users, vec!["owner@example.com"]);
        assert_eq!(h.rooms.len(), 3);
        assert_eq!(h.rooms[0].kind.as_deref(), Some("OFFICE"));
        assert_eq!(h.rooms[0].device_ids, vec!["dev-1"]);
        assert!(h.rooms[2].device_ids.is_empty());
        let d1 = &h.devices[0];
        assert_eq!(d1.room.as_deref(), Some("Office"));
        assert_eq!(d1.partner_device_id.as_deref(), Some("P1"));
        assert_eq!(d1.model.as_deref(), Some("H6076"));
        assert_eq!(
            h.devices[1].assigned_kind.as_deref(),
            Some("action.devices.types.LIGHT")
        );
        assert!(h.devices[2].room.is_none());
        assert_eq!(g.room_types[0].code, "OFFICE");
        assert_eq!(g.project_types[0].name, "Acme Lights");
    }

    #[test]
    fn get_traits_round_trip() {
        assert_eq!(get_traits_request(&["a", "b"]), json!([[["a"], ["b"]]]));
        let resp = json!([[
            [
                ["a"],
                [
                    ["deviceStatus", [["online", [null, null, null, 1]]]],
                    ["onOff", [["onOff", [null, null, null, 0]]]]
                ]
            ],
            [
                ["b"],
                [["deviceStatus", [["online", [null, null, null, 0]]]]]
            ],
            [["c"], null]
        ]]);
        let m = parse_online(&resp);
        assert_eq!(m.get("a"), Some(&true));
        assert_eq!(m.get("b"), Some(&false));
        assert_eq!(m.get("c"), None);
    }

    #[test]
    fn single_home_slot_is_accepted() {
        let g = parse(&json!([0, ["home-x", "Solo", null, null, null, [], []]]));
        assert_eq!(g.homes.len(), 1);
        assert_eq!(g.homes[0].id, "home-x");
    }

    #[test]
    fn garbage_parses_to_empty_not_panic() {
        assert!(parse(&json!(null)).homes.is_empty());
        assert!(parse(&json!([1, "nope"])).homes.is_empty());
        assert!(parse(&json!([1, [[42]]])).homes.is_empty());
    }

    #[test]
    fn device_resolution_ladder() {
        let g = graph();
        let devs: Vec<&Device> = g.homes[0].devices.iter().collect();
        assert_eq!(resolve_device(&devs, "dev-2").unwrap().name, "Desk Plug");
        assert_eq!(resolve_device(&devs, "office lamp").unwrap().id, "dev-1");
        assert_eq!(resolve_device(&devs, "desk").unwrap().id, "dev-2");
        assert!(matches!(
            resolve_device(&devs, "plug"),
            Err(CliError::NotFound(_))
        ));
        assert!(matches!(
            resolve_device(&devs, "toaster"),
            Err(CliError::NotFound(_))
        ));
    }

    #[test]
    fn room_resolution_and_home_selection() {
        let g = graph();
        let rooms: Vec<&Room> = g.homes[0].rooms.iter().collect();
        assert_eq!(resolve_room(&rooms, "OFFICE").unwrap().id, "room-office");
        assert_eq!(resolve_room(&rooms, "living").unwrap().id, "room-living");
        assert!(matches!(
            resolve_room(&rooms, "attic"),
            Err(CliError::NotFound(_))
        ));
        assert_eq!(g.select_home(Some("my home")).unwrap().len(), 1);
        assert!(matches!(
            g.select_home(Some("other")),
            Err(CliError::NotFound(_))
        ));
        assert_eq!(g.select_home(None).unwrap().len(), 1);
    }
}
