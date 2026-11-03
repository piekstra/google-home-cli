# Changelog

## 0.9.0 — 2026-11-03

- `ghome mcp`: tools for Claude. `ghome_devices` and `ghome_device_state` read; `ghome_device_set` changes one light,
  plug or wall switch, named in full, and `ghome_room_lights` the lights of one room, and a client that honors their
  annotations asks you before each call. Each returns the state read back from Google and says whether it shows the
  change. A write that reaches its command 60 s or more after the call began sends nothing and says so, so a keychain
  prompt answered late cannot switch a device. They run over stdio for Claude Code and Claude Desktop, and with
  `--http` on 127.0.0.1 behind a bearer token for Claude on the web and mobile through your own Cloudflare Worker and
  tunnel, reads only unless `--allow-writes`. No tool can reach a lock, a garage door, a heater, a camera or a speaker,
  remove, rename or move a device, edit a room or a routine, announce, change an account, or reach `ghome api`.
- Releases are cut by pushing a tag, no longer by merging a version bump. They attach `ghome-<version>.mcpb`, a Claude
  Desktop extension, and a release is published only once every asset is there.
- `self-update` says to restart a running `ghome mcp --http` and to install the new `.mcpb`.
- A minted bearer is cached under a lock every `ghome` process shares, and only over the master token it came from, so a
  server minting during `auth logout --forget` or `auth login --overwrite` cannot put a removed credential back. A login
  waits for a mint in flight.
- `auth logout --forget` also removes the connector token, and plain `auth logout` says it keeps it. A token it cannot
  remove still lets it clear the config, and it then says so and exits nonzero.
- A failure that got no answer from Google no longer prints its URL.
- cli-common v0.23.0, up from v0.15.1. The minimum Rust version is 1.89.
