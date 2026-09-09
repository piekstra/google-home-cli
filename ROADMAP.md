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
| Delete a room | planned | `SpacesService/DeleteSpace` `["<structure>","<space>"]`, decoded; needs the empty-room guard |
| Rename a device (Google-side name) | done | `devices rename` (`UpdateDeviceSettings`) |
| Remove a device from Google Home | done | `devices remove` (`DeleteDevice`); vendor re-sync may re-add it |
| Ask every vendor to re-sync | done | `devices sync` |
| Unlink a vendor integration | blocked | `SetupService/UnlinkApplication` decoded, but its "linkable app id" is not the agent id and no read exposes it yet |
| Apply an audit's expectations in one go | idea | `audit --apply` = one `devices move`/`place` per mismatch, confirmed once |
| Rename the home, home address | idea | `StructuresService/UpdateStructure(V2)`; layout not decoded |

## Control and state

| Capability | Status | Notes |
|---|---|---|
| Announce / broadcast a message to speakers and displays | blocked | Command decoded (`AssistantBroadcastTrait.BroadcastCommand{msg}`) and the `MeshInteractionService/SendCommands` envelope parses on both mesh hosts, but the JSON gateway can't resolve the command's `Any` type, `clients6` refuses binary bodies ("unsafe for trusted domain"), and the plain gRPC path 404s over HTTP/1.1. Next: native gRPC over HTTP/2 to `homeplatformmesh-pa.googleapis.com`, or the local Cast route (mDNS + Cast v2 + TTS audio), or `ProcessQuery` once its surface-context slot is decoded |
| Free-text command to the Assistant ("Ask Home") | planned | `ProcessQuery`; request shape decoded except the required surface-context slot |
| Device on/off, brightness, colour temperature | done | `devices set`, `rooms set` (`UpdateTraits`); colour (hue/saturation) still planned |
| Volume / mute, media play-pause-stop | done | `devices set --volume/--mute/--media` (`UpdateTraits`); verified on lights only so far |
| Full device state read | done | `devices state` (`GetTraits`) |
| Lock / unlock with PIN challenge | planned | `UpdateTraits` `lockUnlock`, `pin` on `pinNeeded` |
| Thermostat setpoint / mode | idea | `UpdateTraits` `temperatureSetting` |

## Routines and automations

| Capability | Status | Notes |
|---|---|---|
| List routines / automations | planned | `AutomationService/ListAutomations` `["<structure>"]` |
| Run a routine | planned | `AutomationService/ExecuteAutomation` `["<structure>","<automation>",null,2]` |
| Create / edit automations | idea | `UpsertAutomation`; script format unknown |

## Vendor side (other repos, tracked here so the audit has inputs)

| Capability | Status | Notes |
|---|---|---|
| Kasa rooms from the cloud (`tplc rooms list`) | planned | `listDeviceGroups` / `updateDeviceGroup`, group type `room`, from the Kasa APK; unverified live |
| Govee rooms (`govee rooms list`) | planned | private app API (`app2.govee.com`, email 2FA); `groupId` + `groups[]` |
| Tapo rooms | blocked | endpoints on the NBU cloud unknown |
| Emit `device-rooms/v1` from both for `ghome audit --expect -` | planned | contract defined in README |

## Platform

| Capability | Status | Notes |
|---|---|---|
| Browser-handoff login, keychain credential, cached bearer | done | `auth login` |
| Raw RPC passthrough | done | `api POST <Service>/<Method> --data '[...]'`; `--proto <base64>` sends serialized protobuf, `--grpc-web` frames it |
| Public repo, releases, self-update | done | release on version bump |
| Broadcast to one device via local Cast (no Google auth) | idea | mDNS + Cast protocol; fallback if the cloud path stays closed |
