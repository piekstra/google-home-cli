//! `HomeControlService/GetTraits` and `UpdateTraits`: device state and
//! control. Trait payloads are `[traitName, [[fieldName, wrapper], …]]`
//! where the wrapper puts a value at a type-specific slot — ints at 1,
//! strings at 2, bools (0/1) at 3. Layouts captured live by the
//! googlehome-mcp project (2026-07) and re-verified here.

use serde_json::{json, Map, Value};

use crate::homegraph::Device;

pub const SERVICE: &str = "HomeControlService";
pub const GET_TRAITS: &str = "GetTraits";
pub const UPDATE_TRAITS: &str = "UpdateTraits";

fn at(v: &Value, i: usize) -> &Value {
    v.get(i).unwrap_or(&Value::Null)
}

/// Unwrap a scalar wrapper into a plain JSON value.
pub fn unwrap_scalar(w: &Value) -> Value {
    if let Some(n) = at(w, 1).as_i64() {
        return json!(n);
    }
    if let Some(f) = at(w, 1).as_f64() {
        return json!(f);
    }
    if let Some(s) = at(w, 2).as_str() {
        return json!(s);
    }
    match at(w, 3) {
        Value::Number(n) => json!(n.as_i64() == Some(1)),
        Value::Bool(b) => json!(*b),
        _ => Value::Null,
    }
}

/// Parse one device's traits out of a GetTraits/UpdateTraits response into
/// `{trait: {field: value}}`, keyed by device id.
pub fn parse_states(raw: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for entry in at(raw, 0).as_array().into_iter().flatten() {
        let Some(id) = at(at(entry, 0), 0).as_str() else {
            continue;
        };
        let mut traits = Map::new();
        for t in at(entry, 1).as_array().into_iter().flatten() {
            let Some(name) = at(t, 0).as_str() else {
                continue;
            };
            let mut fields = Map::new();
            for f in at(t, 1).as_array().into_iter().flatten() {
                if let Some(fname) = at(f, 0).as_str() {
                    let v = unwrap_scalar(at(f, 1));
                    if !v.is_null() {
                        fields.insert(fname.to_string(), v);
                    }
                }
            }
            traits.insert(name.to_string(), Value::Object(fields));
        }
        out.insert(id.to_string(), Value::Object(traits));
    }
    out
}

/// The normalized view of a device's state (device-state/v1 payload).
pub fn summarize(id: &str, name: &str, traits: &Value) -> Value {
    let get = |t: &str, f: &str| traits.get(t).and_then(|x| x.get(f)).cloned();
    let mut v = Map::new();
    v.insert("device_id".into(), json!(id));
    v.insert("name".into(), json!(name));
    if let Some(o) = get("deviceStatus", "online") {
        v.insert("online".into(), o);
    }
    if let Some(o) = get("onOff", "onOff") {
        v.insert("on".into(), o);
    }
    if let Some(b) = get("brightness", "brightness") {
        v.insert("brightness".into(), b);
    }
    if let Some(k) = get("color", "colorTemperature") {
        v.insert("color_temperature_k".into(), k);
    }
    if let Some(x) = get("volume", "currentVolume") {
        v.insert("volume".into(), x);
    }
    if let Some(x) = get("volume", "isMuted") {
        v.insert("muted".into(), x);
    }
    if let Some(x) = get("lockUnlock", "isLocked") {
        v.insert("locked".into(), x);
    }
    if let Some(x) = get("mediaState", "playbackState") {
        v.insert("playback".into(), x);
    }
    v.insert("traits".into(), traits.clone());
    Value::Object(v)
}

pub fn get_traits(ids: &[&str]) -> Value {
    json!([ids.iter().map(|i| json!([i])).collect::<Vec<_>>()])
}

fn bool_w(b: bool) -> Value {
    json!([null, null, null, if b { 1 } else { 0 }])
}
fn int_w(n: i64) -> Value {
    json!([null, n])
}
fn str_w(s: &str) -> Value {
    json!([null, null, s])
}

/// What to change on a device. Each field set becomes one trait update.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Change {
    pub on: Option<bool>,
    pub brightness: Option<u8>,
    pub color_temperature_k: Option<u32>,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
    pub playback: Option<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        *self == Change::default()
    }

    /// Human summary for the confirmation line, e.g. `on, brightness 40`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(o) = self.on {
            parts.push(if o {
                "on".to_string()
            } else {
                "off".to_string()
            });
        }
        if let Some(b) = self.brightness {
            parts.push(format!("brightness {b}"));
        }
        if let Some(k) = self.color_temperature_k {
            parts.push(format!("{k}K"));
        }
        if let Some(v) = self.volume {
            parts.push(format!("volume {v}"));
        }
        if let Some(m) = self.muted {
            parts.push(if m { "mute".into() } else { "unmute".into() });
        }
        if let Some(p) = &self.playback {
            parts.push(p.clone());
        }
        parts.join(", ")
    }

    fn traits(&self) -> Vec<Value> {
        let mut t = Vec::new();
        if let Some(o) = self.on {
            t.push(json!(["onOff", [["onOff", bool_w(o)]]]));
        }
        if let Some(b) = self.brightness {
            t.push(json!(["brightness", [["brightness", int_w(i64::from(b))]]]));
        }
        if let Some(k) = self.color_temperature_k {
            t.push(json!([
                "color",
                [["colorTemperature", int_w(i64::from(k))]]
            ]));
        }
        let mut vol = Vec::new();
        if let Some(v) = self.volume {
            vol.push(json!(["currentVolume", int_w(i64::from(v))]));
        }
        if let Some(m) = self.muted {
            vol.push(json!(["isMuted", bool_w(m)]));
        }
        if !vol.is_empty() {
            t.push(json!(["volume", vol]));
        }
        if let Some(p) = &self.playback {
            t.push(json!(["mediaState", [["playbackState", str_w(p)]]]));
        }
        t
    }
}

/// One `UpdateTraits` request applying `change` to every device given.
/// Shape: `[[[ [id,[agent,partner]], [trait, …] ], …]]`.
pub fn update_traits(devices: &[&Device], change: &Change) -> Value {
    let cmds: Vec<Value> = devices
        .iter()
        .map(|d| {
            json!([
                [
                    d.id,
                    [
                        d.agent_id.clone().unwrap_or_default(),
                        d.partner_device_id.clone().unwrap_or_default()
                    ]
                ],
                change.traits()
            ])
        })
        .collect();
    json!([cmds])
}

/// Does this device advertise the traits the change needs?
pub fn supports(d: &Device, change: &Change) -> bool {
    let has = |suffix: &str| d.traits.iter().any(|t| t.ends_with(suffix));
    (change.on.is_none() || has("OnOff"))
        && (change.brightness.is_none() || has("Brightness"))
        && (change.color_temperature_k.is_none() || has("ColorSetting"))
        && (change.volume.is_none() && change.muted.is_none() || has("Volume"))
        && (change.playback.is_none() || has("TransportControl") || has("MediaState"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, traits: &[&str]) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            kind: Some("action.devices.types.LIGHT".into()),
            assigned_kind: None,
            agent_id: Some("agent".into()),
            partner_device_id: Some("P".into()),
            model: None,
            traits: traits
                .iter()
                .map(|t| format!("action.devices.traits.{t}"))
                .collect(),
            room_id: None,
            room: None,
        }
    }

    #[test]
    fn states_parse_from_the_captured_shape() {
        let raw = json!([[[
            ["d1"],
            [
                ["deviceStatus", [["online", [null, null, null, 1]]]],
                ["onOff", [["onOff", [null, null, null, 0]]]],
                ["brightness", [["brightness", [null, 40]]]],
                ["mediaState", [["playbackState", [null, null, "paused"]]]]
            ]
        ]]]);
        let m = parse_states(&raw);
        let s = summarize("d1", "Lamp", &m["d1"]);
        assert_eq!(s["online"], true);
        assert_eq!(s["on"], false);
        assert_eq!(s["brightness"], 40);
        assert_eq!(s["playback"], "paused");
    }

    #[test]
    fn update_body_matches_the_captured_shape() {
        let d = dev("d1", &["OnOff", "Brightness"]);
        let change = Change {
            on: Some(false),
            ..Default::default()
        };
        assert_eq!(
            update_traits(&[&d], &change),
            json!([[[
                ["d1", ["agent", "P"]],
                [["onOff", [["onOff", [null, null, null, 0]]]]]
            ]]])
        );
        let change = Change {
            on: Some(true),
            brightness: Some(75),
            ..Default::default()
        };
        let body = update_traits(&[&d], &change);
        assert_eq!(body[0][0][1].as_array().unwrap().len(), 2);
        assert_eq!(
            body[0][0][1][1],
            json!(["brightness", [["brightness", [null, 75]]]])
        );
        assert_eq!(change.describe(), "on, brightness 75");
        assert!(supports(&d, &change));
        assert!(!supports(
            &d,
            &Change {
                volume: Some(3),
                ..Default::default()
            }
        ));
    }
}
