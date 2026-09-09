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
| Room audit: name heuristic + vendor expectations | done | `audit`, `--expect` takes `device-rooms/v1` |
| Move a device between rooms | done | `devices move` (`BatchModifySpacesDevices`) |
| Add an unplaced device to the home / a room | done | `devices place` (`BatchModifyStructuresDevices`) |
| Create / rename a room | done | `rooms create`, `rooms rename` |
| Delete a room | done | `rooms delete`, refuses non-empty rooms |
| Rename a device (Google-side name) | done | `devices rename` (`UpdateDeviceSettings`) |
| Remove a device from Google Home | done | `devices remove` (`DeleteDevice`); confirmed: a vendor re-sync (`devices sync`) brings a still-listed device back as unplaced, so delete it at the vendor too |
| Ask every vendor to re-sync | done | `devices sync` |
| Unlink a vendor integration | blocked | `SetupService/UnlinkApplication` decoded, but its "linkable app id" is not the agent id and no read exposes it yet. Unlinking in the Home app works and is what finally removes a vendor's dead devices (verified: Tuya and Yale gone after unlink + `devices sync`) |
| Apply an audit's expectations in one go | idea | `audit --apply` = one `devices move`/`place` per mismatch, confirmed once |
| Rename the home, home address | idea | `StructuresService/UpdateStructure(V2)`; layout not decoded |

## Control and state

| Capability | Status | Notes |
|---|---|---|
| Announce / broadcast a message to speakers and displays | blocked | `ghome announce` implements the Play-services recipe (mesh scope + `OAuthSessionTrait.UpdateToken` handshake + `BroadcastCommand` over native gRPC) and Google answers status 13/3; the mesh client is a runtime-delivered native module. Remaining routes: local Cast with generated speech, or `ProcessQuery` text queries |
| Free-text command to the Assistant ("Ask Home") | planned | `ProcessQuery`; request shape decoded except the required surface-context slot |
| Device on/off, brightness, colour temperature | done | `devices set`, `rooms set` (`UpdateTraits`); colour (hue/saturation) still planned |
| Volume / mute, media play-pause-stop | done | `devices set --volume/--mute/--media` (`UpdateTraits`); verified on lights only so far |
| Full device state read | done | `devices state` (`GetTraits`) |
| Lock / unlock with PIN challenge | planned | `UpdateTraits` `lockUnlock`, `pin` on `pinNeeded` |
| Thermostat setpoint / mode | idea | `UpdateTraits` `temperatureSetting` |

## Routines and automations

| Capability | Status | Notes |
|---|---|---|
| List routines / automations | done | `routines list` |
| Run a routine | done (unverified live) | `routines run`, confirms unless `--force`; response `["1"]` = started |
| Create / edit automations | idea | `UpsertAutomation`; script format unknown |

## Vendor side (other repos, tracked here so the audit has inputs)

| Capability | Status | Notes |
|---|---|---|
| Kasa rooms from the cloud (`tplc groups list`) | done | `api.tplinkra.com/v1/device-groups` with the SDK envelope, verified live; this account has no Kasa groups (its rooms are in the Tapo app) |
| Govee rooms (`govee rooms list`) | done | private app API, email + emailed code (read from Gmail by `gro`); `rooms devices` feeds `ghome audit --expect -`, verified end to end |
| Tapo rooms | blocked | endpoints on the NBU cloud unknown |
| Emit `device-rooms/v1` from both for `ghome audit --expect -` | done | `tplc groups devices`, `govee rooms devices` |

## Platform

| Capability | Status | Notes |
|---|---|---|
| Browser-handoff login, keychain credential, cached bearer | done | `auth login` |
| Raw RPC passthrough | done | `api POST <Service>/<Method> --data '[...]'`; `--proto <base64>` sends serialized protobuf, `--grpc-web` frames it |
| Public repo, releases, self-update | done | release on version bump |
| Broadcast to one device via local Cast (no Google auth) | idea | mDNS + Cast protocol; fallback if the cloud path stays closed |
