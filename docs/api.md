# The Foyer API, the credential path, and the traps

What `ghome` talks to, how it authenticates, and the places this API misleads
you. Google publishes none of it; everything here is reverse-engineered and
dated so the next person can tell what may have rotted.

## Transport

Foyer is the private backend of the Google Home app and of home.google.com.
`ghome` speaks it the way the web app does — **gRPC-web as JSON**:

```
POST https://googlehomefoyer-pa.clients6.google.com/$rpc/google.internal.home.foyer.v1.<Service>/<Method>
Authorization: Bearer <ya29…>
Content-Type: application/json+protobuf
X-User-Agent: grpc-web-javascript/0.1
```

Request and response bodies are **positional JSON arrays**, not objects:
protobuf field number `N` sits at array index `N-1`, absent fields are `null`,
and scalars inside trait payloads are wrapped (`[null, <int>]`,
`[null, null, "<string>"]`, `[null, null, null, <0|1>]`). Some responses are
prefixed with the XSSI guard `)]}'` on its own line; strip it before parsing.

**Trap — no API key with a Bearer.** The web app pairs its public
`X-Goog-Api-Key` with cookie-derived `SAPISIDHASH` auth. A Bearer minted for
the Android Home app belongs to a different Google project, and sending the
web key alongside it gets HTTP 400 `CONSUMER_INVALID` ("The API Key and the
authentication credential are from different projects"). Send the Bearer
alone. (Verified live 2026-07 by the googlehome-mcp project; the header set
above is exactly what `ghome` sends.)

The native gRPC endpoint `googlehomefoyer-pa.googleapis.com:443` speaks the
same RPCs in protobuf; `ghome` avoids it so it needs no proto codegen.

## Credential path

Foyer accepts a Bearer for the Google Home Android app
(`com.google.android.apps.chromecast.app`, signing cert
`24bb24c05e47e0aefa68a58a766179d9b613a600`, scope
`oauth2:https://www.googleapis.com/auth/homegraph`). Such a Bearer is minted
from an Android **master token** (`aas_et/…`) at
`https://android.clients.google.com/auth` — the "gpsoauth" protocol:
form-encoded POST, `User-Agent: GoogleAuth/1.4`, newline `Key=Value` reply.

**Password login is not offered.** Since 2025 Google answers
`perform_master_login` with `BadAuthentication`, `NeedsBrowser` (passkey
accounts), or `MissingDroidguard` for most accounts, and app passwords are
blocked under Advanced Protection. The path that still works (verified by
several projects through 2026) starts in a browser:

1. Open `https://accounts.google.com/EmbeddedSetup`, sign in, click **I agree**.
   The page then appears to hang; that is expected.
2. Copy the `oauth_token` cookie for `accounts.google.com` (value starts with
   `oauth2_4/`). It is single-use and short-lived.
3. `ghome auth login` exchanges it (`service=ac2dm`, GMS signing cert
   `38918a453d07199354f8b19af05ec6562ced5788`, `droidguard_results=dummy123`)
   for the master token, which it stores in the keychain (`piekstra.ghome`,
   account = the email). Bearers are minted from it on demand and cached in
   the keychain (`<email>/bearer`) until they expire (about an hour).

The `androidId` sent in the exchange must be reused when minting Bearers.
`ghome` generates one on first login and keeps it in the config file
(`android_id`); it is not a secret.

Google may send the account a "new device signed in" security alert after
the exchange — it registered an Android device. Expected, not a compromise.

## `StructuresService/GetHomeGraph`

Request body: `[]`. The response is the whole account: homes, rooms,
devices, and label dictionaries. Indices `ghome` reads (0-based):

```
resp[1]              homes (a list; older captures show a single home array)
  home[0] id
  home[1] name
  home[2] location      [address, [lat, lng], null, null, ts, timezone]
  home[3] linked users  [[email], …]
  home[5] rooms
    room[0] id
    room[2] name
    room[3] category    ["OFFICE"] etc.
    room[4] members     [[[deviceId, [agentId, partnerDeviceId]]], …]  ← not in the public proto
  home[6] devices
    dev[0]  key         [deviceId, [agentId, partnerDeviceId]]
    dev[3]  name
    dev[5]  type        action.devices.types.LIGHT …
    dev[6]  traits      [action.devices.traits.OnOff, …]
    dev[16] hardware    [null, model]
    dev[20] assigned    [type] — the user-chosen type overriding dev[5]  ← not in the public proto
    dev[25] linked users
    dev[27] local_auth_token — a live credential for the device; never printed
resp[3]  room types     [[code, name], …]
resp[6]  device types   [[code, name], …]
resp[8]  project types  [[agentId, label], …]
```

`agentId` is the partner's Google project id — which vendor integration owns
the device. `partnerDeviceId` is that vendor's own id for it, and is the join
key `ghome audit` uses against vendor CLIs' `device-rooms/v1` output.

Verified against a live account on 2026-09-08 (one home, 91 devices): `resp[1]`
is a list of homes; rooms and membership sit exactly as above; `dev[16]` is an
empty list for vendors that report no model; `dev[17]` is `[null, name]`;
`dev[18]` is a string timestamp; `dev[27]` is populated on several devices.
`resp[8]` entries carry a third slot with a partner icon URL.

Sources: the public proto extracted from the app
(github.com/KapJI/ghome-foyer-api `api.proto`, field numbers) and the live
web capture in github.com/ericmigi/googlehome-mcp `docs/PROTOCOL.md`
(2026-07-15; membership and assigned-type slots).

## Online state — `HomeControlService/GetTraits`

Body `[[["<id1>"], ["<id2>"], …]]`; response `[[ [["<id>"], [ ["deviceStatus",
[["online", [null,null,null,<0|1>]]]], ["onOff", …], … ]], … ]]`. `online` is
what the partner last reported, so a device from a previous home that the
vendor still lists shows `0` here while still sitting in a room. Batched 60
ids per call; the web app sends all ids in one go.

## Writing rooms — decoded from the app, verified live

Google Home calls rooms **spaces** internally. The field numbers below were
decoded on 2026-09-08 from the protobuf-lite message descriptors in Google
Home for Android 4.28.27.0 (`newMessageInfo` strings; R8 had minified the
field *names*, so names are from the app's call sites). The move layout was
then verified live: the device changed room and `GetSpace` listed it.

Every id below is a string. A `DeviceId` is a one-element message
`["<hgs device id>"]` (field 1; field 2 is the partner form
`[null, [agentId, "DEVICE_<id>"]]`). A `Space` is `[id, null, displayName,
[typeCode, typeName], deviceRefs]`. Space ids are `structure.uuid`.

| RPC | Body (positional) | Returns |
|---|---|---|
| `SpacesService/BatchModifySpacesDevices` — move | `[null, [[[ "<space>", [["<dev>"]] ]]]]` — **field 1 does not exist**; instructions are field 2, each `{1: spaceId, 2: assign[DeviceId], 3: unassign[DeviceId]}`. The app sends only the assign; the server removes the device from its old room. | empty |
| `StructuresService/BatchModifyStructuresDevices` — add an unplaced device to the home | `[[[[ "<structure>", [["<dev>"]] ]]]]` (instructions **are** field 1 here) | empty |
| `SpacesService/CreateSpace` | `["<structure>", [null, null, "<name>", ["<TYPE>", "<Type name>"]]]`; a third slot `[["<dev>"], …]` moves devices in at creation | the `Space` |
| `SpacesService/UpdateSpace` — rename | `["<structure>", "<space>", [null, null, "<name>"], [["display_name"]]]` (field 4 is a FieldMask) | the `Space` |
| `SpacesService/DeleteSpace` | `["<structure>", "<space>"]` | empty |
| `HomeDevicesService/UpdateDeviceWhere` | `[["<dev>"], ["<structure>", "<space>"]]` (or `[…, [ "<structure>", null, <Space> ]]` to create the room in the same call) — the app uses this only with the partner id form for devices not yet in the graph; untested here | empty |

Device-level writes, decoded the same way and verified live on 2026-09-09:

| RPC | Body | Notes |
|---|---|---|
| `HomeDevicesService/DeleteDevice` | `[null, ["<dev>"]]` (field 1 absent) | the app's "Remove device"; a vendor that still lists the device re-adds it as unplaced on its next sync |
| `HomeDevicesService/UpdateDeviceSettings` | `[["<dev>"], [[["<name>"]]], [["basic_settings.name"]]]` | rename (Google-side name); returns `[null, device]` |
| `HomeDevicesService/SyncDevices` | `[]` | "sync my devices" |
| `SetupService/UnlinkApplication` | `["<linkable app id>"]` | decoded but **not usable yet**: the id is not the agent id (rejected 400) and `GetLinkableApplications` returns only media apps |
| `HomeControlService/GetTraits` | see above | online state |

The write responses are empty (or echo the record), so `ghome` treats a 2xx
as nothing and reads the result back before reporting success.

Rejected layouts, for the record (each a clean HTTP 400): instructions at
field 1, bare-string device ids, and the assign list ahead of the space id.

## Why fixing the vendor apps is not enough

Every vendor's Google integration may send a `roomHint` when it SYNCs a
device, and Google only honours it **when the device is new to Home Graph**:
first link, a newly added device, or after unlink → relink (which deletes and
recreates every device from that vendor, losing Google-side nicknames and
routine references). Changing the room in the Govee or Tapo app and
re-syncing moves nothing in Google Home. That is why `ghome` treats Google
Home as the thing to fix and the vendor apps as a source of expectations.

Vendor room data, for the audit's `--expect` input:

- **Govee**: the public developer API (`openapi.api.govee.com`) has no room
  concept. The app's private API (`app2.govee.com`, email + password login,
  email 2FA code since 2026-05) returns each device's `groupId` and a
  `groups[]` table — the app's rooms. No public write endpoint is known.
- **TP-Link Kasa**: the app talks to a `device-groups` module with
  `listDeviceGroups` / `updateDeviceGroup` and group type `room` (from the
  decompiled Kasa APK). Not yet exercised live.
- **TP-Link Tapo**: rooms live on the `iot.i.tplinknbu.com` cloud; endpoints
  unknown.

Those belong in the vendor CLIs (`govee`, `tplc`), emitting `device-rooms/v1`
for `ghome audit --expect -`.

## Other traps

- `Home home = 2` is singular in the public proto but arrives as a **list** on
  real accounts. `ghome` accepts both.
- A 401/403 mid-session usually means the cached Bearer expired; `ghome`
  mints a fresh one and retries the call once. If the retry also fails, the
  master token is dead (revoked device, password change): run `auth login`.
- Response sizes: a real `GetHomeGraph` is ~100 KB for ~70 devices. Every
  domain command fetches it fresh; there is no cache.
