# Roadmap — capabilities and their status

The single list of what `ghome` can do, what it should do next, and what is
blocked and why. Update it in the same commit as the capability it describes;
a capability without an entry here does not exist. Statuses: **done**
(shipped, verified live), **next** (being built), **planned** (layout known,
not built), **blocked** (needs something we don't have yet), **idea**.

## Rooms and placement

| Capability | Status | Notes |
|---|---|---|
| List homes, rooms, devices, room membership | done | `homes`, `rooms`, `devices` over `GetHomeGraph` |
| Devices linked to the account but in no home | done | `devices list --unplaced` (`ListUnassignedDevices`) |
| Vendor-reported online state | done | `devices list --status` / `--offline` (`GetTraits`) |
| Room audit: name heuristic + vendor expectations | done | `audit`, `--expect` takes `device-rooms/v1`; a vendor row without `room` is reported `unfiled` (the vendor app files the device nowhere; found five such Govee devices on 2026-09-10 that the audit had been blind to) |
| Move a device between rooms | done | `devices move` (`BatchModifySpacesDevices`) |
| Add an unplaced device to the home / a room | done | `devices place` (`BatchModifyStructuresDevices`) |
| Create / rename a room | done | `rooms create`, `rooms rename` |
| Delete a room | done | `rooms delete`, refuses non-empty rooms |
| Rename a device (Google-side name) | done | `devices rename` (`UpdateDeviceSettings`) |
| Remove a device from Google Home | done | `devices remove` (`DeleteDevice`); confirmed: a vendor re-sync (`devices sync`) brings a still-listed device back as unplaced, so delete it at the vendor too |
| Ask every vendor to re-sync | done | `devices sync` |
| Unlink a vendor integration | blocked | `SetupService/UnlinkApplication` decoded, but its "linkable app id" is not the agent id and no read exposes it yet. Unlinking in the Home app works and is what finally removes a vendor's dead devices (verified: Tuya and Yale gone after unlink + `devices sync`) |
| Apply an audit's expectations in one go | done | `audit --apply`: one `devices move`/`place` per mismatch, unassigned or unplaced row an explicit expectation decided, confirmed once, each read back; rows whose room does not exist are skipped, never created |
| Rename the home, home address | idea | `StructuresService/UpdateStructure(V2)`; layout not decoded |

## Control and state

| Capability | Status | Notes |
|---|---|---|
| Announce / broadcast a message to speakers and displays | done (local) | `ghome announce` speaks over the LAN with the Cast protocol and generated speech (`--device`, `--room`, or everywhere; `--url` for any audio; `--volume` restored afterwards), verified live 2026-09-14. The cloud path (`--via cloud`: mesh scope + `OAuthSessionTrait.UpdateToken` + `BroadcastCommand` over native gRPC) stays blocked — Google answers status 13/3 and the mesh client is a runtime-delivered Play-services module |
| Free-text command to the Assistant ("Ask Home") | planned | `ProcessQuery`; request shape decoded except the required surface-context slot |
| Device on/off, brightness, colour temperature, colour | done | `devices set`, `rooms set` (`UpdateTraits`); `--color` takes a name, `#rrggbb`, `rgb()` or `hsv()` and writes `color.colorRGB` |
| Volume / mute, media play-pause-stop | done | `devices set --volume/--mute/--media` (`UpdateTraits`); verified on lights only so far |
| Full device state read | done | `devices state` (`GetTraits`) |
| Lock / unlock with PIN challenge | planned | `UpdateTraits` `lockUnlock`, `pin` on `pinNeeded` |
| Thermostat setpoint / mode | idea | `UpdateTraits` `temperatureSetting` |

## Routines and automations

| Capability | Status | Notes |
|---|---|---|
| List routines / automations | done | `routines list` |
| Run a routine | done | `routines run`, confirms unless `--force`; response `["1"]` = started. Verified live 2026-09-14 on an automation ghome created (a light changed colour) |
| Create / edit / delete automations | done | `routines validate|create|update|delete --file <yaml>`: the script editor's YAML through `ValidateAutomation`, `UpsertAutomation` (field mask `script_details.content`, `status.is_enabled`) and `DeleteAutomation`, captured from the web editor; every write read back through the list |

## Vendor side (other repos, tracked here so the audit has inputs)

| Capability | Status | Notes |
|---|---|---|
| Kasa rooms from the cloud (`tplc groups list`) | done | `api.tplinkra.com/v1/device-groups` with the SDK envelope, verified live; this account has no Kasa groups (its rooms are in the Tapo app) |
| Govee rooms (`govee rooms list`) | done | private app API, email + emailed code (read from Gmail by `gro`); `rooms devices` feeds `ghome audit --expect -`, verified end to end |
| Govee room writes (`govee rooms move|create|rename|delete`) | done | govee-cli #19; endpoints decoded from Govee Home for Android 7.6.21; verified live 2026-09-10 (room created, seven devices moved, all read back) |
| Tapo rooms (`tplc rooms list|devices|move|create|rename|delete`) | done | tplink-cloud-cli #3 + #4; NBU app-server (`/v1/families`, `/v2/things`, `POST /v1/families/thing-settings`), decoded from Tapo for Android 3.20.753. Verified live 2026-09-10: `tplc --json rooms devices \| ghome audit --expect -` joins the Tapo lock on Google's partner id (the MAC without separators), audit 95 ok |
| Emit `device-rooms/v1` from both for `ghome audit --expect -` | done | `tplc groups devices`, `tplc rooms devices`, `govee rooms devices`; since govee 0.2.1 / tplc 0.2.1 a device the vendor app files in no room keeps its row with `room` omitted and the audit reports it `unfiled` (verified with a synthetic row, 2026-09-10) |

## Platform

| Capability | Status | Notes |
|---|---|---|
| Browser-handoff login, keychain credential, cached bearer | done | `auth login` |
| Raw RPC passthrough | done | `api POST <Service>/<Method> --data '[...]'`; `--proto <base64>` sends serialized protobuf, `--grpc-web` frames it |
| Public repo, releases, self-update | done | release on version bump |
| Vendor CLIs on the family spec | done | govee-cli #20 and tplink-cloud-cli #3 (2026-09-10): text default + `--json`, family exit codes, `auth`/`config`/`self-update`/`info`, `piekstra.<bin>` keychain with migration; cli-common v0.8.0 carries the shared confirm gate, `pick` resolver, `emit_list`, keychain JSON items and the `device-rooms/v1` contract |
| Broadcast via local Cast (no Google auth) | done | `src/cast.rs`: mDNS `_googlecast._tcp` + CastMessage v2 over TLS 8009, Default Media Receiver; `announce` uses it by default |
