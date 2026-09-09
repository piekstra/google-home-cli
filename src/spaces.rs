//! Encoders for the Foyer write RPCs (rooms are "spaces" internally) and
//! the parsers for what they return. Every body here is a positional array;
//! the field layouts come from the decompiled Home app — see `docs/api.md`.
//!
//! Layouts were decoded from Google Home for Android 4.28.27.0 on
//! 2026-09-08 (protobuf-lite message info; see `docs/api.md`). The commands
//! that use them refuse to run while `LAYOUT_CONFIRMED` is false, so a build
//! carrying an unverified change can never send a guessed write at a home.

use serde_json::{json, Value};

use crate::homegraph::{self, Room};

pub const SPACES: &str = "SpacesService";
pub const STRUCTURES: &str = "StructuresService";
pub const BATCH_MODIFY_SPACES_DEVICES: &str = "BatchModifySpacesDevices";
pub const BATCH_MODIFY_STRUCTURES_DEVICES: &str = "BatchModifyStructuresDevices";
pub const CREATE_SPACE: &str = "CreateSpace";
pub const UPDATE_SPACE: &str = "UpdateSpace";
pub const GET_SPACE: &str = "GetSpace";
pub const DELETE_SPACE: &str = "DeleteSpace";
pub const HOME_DEVICES: &str = "HomeDevicesService";
pub const DELETE_DEVICE: &str = "DeleteDevice";
pub const UPDATE_DEVICE_SETTINGS: &str = "UpdateDeviceSettings";
pub const SYNC_DEVICES: &str = "SyncDevices";

/// True once every layout below has been verified against a live home.
pub const LAYOUT_CONFIRMED: bool = true;

/// A `DeviceId` message carrying the home-graph (hgs) id in field 1.
fn device_id(id: &str) -> Value {
    json!([id])
}

/// Assign `device_id` to `space_id` (a full `structure.uuid` space id).
///
/// `BatchModifySpacesDevicesRequest{ 2: instructions{ 1: [ {1: space_id,
/// 2: assign[DeviceId], 3: unassign[DeviceId]} ] } }` — note field 1 of the
/// request does not exist. The app sends only the assign; the server drops
/// the device from its previous room itself.
pub fn move_device(space_id: &str, device_id_: &str) -> Value {
    json!([null, [[[space_id, [device_id(device_id_)]]]]])
}

/// Assign an account device that is in no home to `structure_id`.
///
/// `BatchModifyStructuresDevicesRequest{ 1: instructions{ 1: [ {1:
/// structure_id, 2: assign[DeviceId], 3: unassign[DeviceId]} ] } }`.
pub fn place_device(structure_id: &str, device_id_: &str) -> Value {
    json!([[[[structure_id, [device_id(device_id_)]]]]])
}

/// Create a space named `name` of category `kind` (e.g. `OFFICE`).
///
/// `CreateSpaceRequest{ 1: parent_structure_id, 2: Space{ 1: id, 3:
/// display_name, 4: SpaceType{1: id, 2: localized_name}, 5: device_refs } }`;
/// the app fills the type's localized name from the category table.
pub fn create_space(structure_id: &str, name: &str, kind: &str, kind_name: &str) -> Value {
    json!([structure_id, [null, null, name, [kind, kind_name]]])
}

/// Rename `space_id` to `name`.
///
/// `UpdateSpaceRequest{ 1: parent_structure_id, 2: space_id, 3: Space, 4:
/// FieldMask{1: paths} }` with the mask on `display_name`, exactly as the app.
pub fn rename_space(structure_id: &str, space_id: &str, name: &str) -> Value {
    json!([
        structure_id,
        space_id,
        [null, null, name],
        [["display_name"]]
    ])
}

/// Remove a device from the account's home graph.
///
/// `DeleteDeviceRequest{ 2: DeviceId }` — field 1 does not exist. Empty
/// response; the app's "Remove device" path.
pub fn delete_device(device_id_: &str) -> Value {
    json!([null, device_id(device_id_)])
}

/// Rename a device (the Google-side name; the vendor keeps its own).
///
/// `UpdateDeviceSettingsRequest{ 1: DeviceId, 2: DeviceSettings{ 1:
/// BasicSettings{ 1: name } }, 3: FieldMask{1: paths} }` with the mask the
/// app sends, `basic_settings.name`. Returns `{2: Device}`. One bracket
/// too many around the name gets a 400 "Unexpected list for single
/// non-message field" — Google's only descriptive rejection so far.
pub fn rename_device(device_id_: &str, name: &str) -> Value {
    json!([device_id(device_id_), [[name]], [["basic_settings.name"]]])
}

/// Ask Google to re-SYNC every linked partner ("sync my devices").
pub fn sync_devices() -> Value {
    json!([])
}

/// Unlink a partner integration ("Works with Google Home → Unlink account").
///
/// `UnlinkApplicationRequest{ 1: linkable_app_id }`. Decoded, **not
/// wired**: the id is not the agent id (`tuya-smart-cee31` was rejected
/// with 400) and `GetLinkableApplications` did not reveal it. Kept for the
/// day the id source is found.
#[allow(dead_code)]
pub fn unlink_application(linkable_app_id: &str) -> Value {
    json!([linkable_app_id])
}

/// `DeleteSpaceRequest{ 1: structure_id, 2: space_id }`.
pub fn delete_space(structure_id: &str, space_id: &str) -> Value {
    json!([structure_id, space_id])
}

/// `GetSpace` request.
pub fn get_space(structure_id: &str, space_id: &str) -> Value {
    json!([structure_id, space_id])
}

/// A single `space` (GetSpace) or a list under slot 0 (BatchModify…, ListSpaces).
pub fn parse_space(v: &Value) -> Option<Room> {
    homegraph::parse_room_value(v)
}

pub fn parse_spaces(v: &Value) -> Vec<Room> {
    v.get(0)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(homegraph::parse_room_value).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_bodies_follow_the_decoded_layouts() {
        assert_eq!(
            move_device("s.r", "d1"),
            json!([null, [[["s.r", [["d1"]]]]]])
        );
        assert_eq!(place_device("s", "d1"), json!([[[["s", [["d1"]]]]]]));
        assert_eq!(
            create_space("s", "Loft", "OTHER", "Other"),
            json!(["s", [null, null, "Loft", ["OTHER", "Other"]]])
        );
        assert_eq!(
            rename_space("s", "s.r", "Attic"),
            json!(["s", "s.r", [null, null, "Attic"], [["display_name"]]])
        );
        assert_eq!(delete_device("d1"), json!([null, ["d1"]]));
        assert_eq!(
            rename_device("d1", "Storage Lamp"),
            json!([["d1"], [["Storage Lamp"]], [["basic_settings.name"]]])
        );
        assert_eq!(sync_devices(), json!([]));
        assert_eq!(unlink_application("acme-agent"), json!(["acme-agent"]));
    }

    #[test]
    fn get_space_and_parsers_round_trip() {
        assert_eq!(get_space("s", "s.r"), json!(["s", "s.r"]));
        let space = json!(["s.r", null, "Office", ["OFFICE"], [[["d1", ["a", "p"]]]]]);
        let r = parse_space(&space).unwrap();
        assert_eq!(r.name, "Office");
        assert_eq!(r.device_ids, vec!["d1"]);
        assert_eq!(parse_spaces(&json!([[space]])).len(), 1);
    }
}
