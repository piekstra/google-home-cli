# Test fixtures — Foyer wire shapes

Foyer answers in **positional JSON arrays** (protobuf field number → array
index, 1-based, `null` holes), so these files look nothing like the DTOs the
CLI emits. Tests load them as files; nothing is embedded as a string literal.

- `get-home-graph.json` — a `StructuresService/GetHomeGraph` response.
  **Synthetic**, assembled on 2026-09-08 from the documented shape (the public
  `api.proto` field numbers plus the two live-captured slots the proto omits:
  room membership at `Room[4]` and the user-assigned type at `Device[20]`).
  Replace it with a scrubbed live capture once one exists; keep the file name.

## Scrubbing policy (enforced by `tests/fixture_shapes.rs`)

Every identity-bearing value must be an obvious dummy:

- ids: `home-…`, `room-…`, `dev-…` (never real UUIDs);
- emails: `@example.com` only;
- addresses: `Example St`; coordinates `0.0`;
- partner device ids: `P<n>`; agent ids: `agent-<letter>`;
- the `local_auth_token` slot (`Device[27]`) is absent — a real capture must
  have it removed, it is a live credential for the device.
