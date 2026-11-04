# Changelog

## v0.10.0 — 2026-11-04

- Windows: releases attach `ghome-x86_64-pc-windows-msvc.tar.gz`, and the `.mcpb` runs there. A credential too long for one Windows credential item is split and read back as one.
- `auth logout --forget` says what it does not end, naming `$GHOME_MCP_TOKEN`, and claims a removal only when there was an item.
- cli-common v0.29.0, up from v0.25.0.

## 0.9.1 — 2026-11-03

- **Security fix:** `ghome api` refuses a plain `http` URL off this machine with exit 2, before it reads
  a credential. A full `http://` URL sent the bearer in the clear.
- **Security fix:** no client that carries a credential follows a redirect off its host. A 307 or 308
  re-sends a body, and a form to Google's auth endpoint holds a master token or an oauth token. A redirect
  to another host is now an error that says so, and a write refused this way is reported as a failure, not
  as an unknown outcome.
- A write's deadlines are now cli-common's `SendDeadline`, with the same 60 s to send and 85 s to start a
  read-back, on both clocks. Its refusal is an `other` error in place of `upstream`, since Google did not
  fail, and it tells the caller to check the current state before calling again.
- cli-common v0.25.0, up from v0.23.0.

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
- A minted bearer, and the move of a legacy keychain item to the one-item layout, are written under a lock every `ghome`
  process shares, and only over the master token they came from, so a server running during `auth logout --forget` or
  `auth login --overwrite` cannot put a removed credential back. A login waits for a mint in flight.
- `auth logout --forget` also removes the connector token from the keychain, and says a running `ghome mcp --http` keeps
  its own until it stops. A token it cannot remove still lets it clear the config, and it then says so and exits nonzero.
- `auth logout --forget` with no account configured now clears the connector token and the config and exits 0, where it
  was a usage error (exit 2), because a connector token can outlive an account. Plain `auth logout` with no account is
  still exit 2.
- A failure that got no answer from Google no longer prints its URL.
- cli-common v0.23.0, up from v0.15.1. The minimum Rust version is 1.89.
