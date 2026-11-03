# AGENTS.md

Guidance for AI coding agents (and humans) working in this repo. Tool-agnostic;
`CLAUDE.md` points here.

## What this is

`ghome` — a Rust CLI over Google Home's private "Foyer" API (the backend of
the Home app and home.google.com), for auditing which room each device is
in. A thin, Google-specific layer over the shared
[`cli-common`](https://github.com/piekstra/cli-common) `pk-cli-*` crates
(auth shapes, http, config, secrets, self-update). This repo owns only the
Foyer client, the codec, and the room commands.

## Build, test, lint

```console
make verify     # fmt-check + clippy -D warnings + tests + smoke — the CI gate
make test
make install    # cargo install + re-sign so keychain grants survive
```

Run `make verify` before considering a change done — it's exactly what CI runs.

## Layout

- `src/main.rs` — clap tree, arg validation, exit-code mapping, `auth`/`config`.
- `src/gpsoauth.rs` — the Android token exchanges (oauth_token → master token
  → Bearer). Pure form-building + a thin POST; unit-tested offline.
- `src/session.rs` — keychain layout: master token and cached Bearer together
  in one item at `<email>` (a `<email>/bearer` item is legacy, read and removed);
  mint/refresh logic under `RefreshLock`.
- `src/mcp.rs` — `ghome mcp`; see "The MCP tools".
- `src/foyer.rs` — the RPC transport (positional JSON over HTTPS), 401/403
  → re-mint once.
- `src/homegraph.rs` — codec for `GetHomeGraph` → `Home`/`Room`/`Device`,
  plus device/room resolution ladders.
- `src/spaces.rs` — encoders for the write RPCs (decoded field layouts) and
  the space parsers.
- `src/audit.rs` — pure audit logic (expectations, name heuristic, statuses).
- `src/commands/*.rs` — one module per command group; `Ctx::graph()` is the
  single network entry point for reads.
- `tests/` — offline surface tests + fixture contract tests; see
  `tests/fixtures/README.md`.
- `docs/api.md` — the wire format, the credential path, the write RPCs we
  know of but haven't captured, and every trap found so far.

## Conventions (do not break these)

- **`--json` on every command**, one DTO tagged `"schema": "<name>/v1"`.
  Human output → stdout; diagnostics → stderr. Keep both paths in sync.
- **Exit codes:** 0 ok · 2 usage · 3 auth · 4 not found · 5 upstream · 6
  confirmation required. Validate args **before** touching the keychain or
  network, so `--help` and bad input never prompt or hang.
- **Secrets** come from the keychain (`piekstra.ghome`), `--stdin`, or
  `--from-env` — never argv, never logs, never a file. `--verbose` prints
  URLs and status codes, never tokens.
- **Never print `local_auth_token`** (device slot 27 in `GetHomeGraph`). It is
  a live credential for the device. The codec doesn't read it; keep it so.
- **Writes are confirmed and read back.** `devices move|place` and `rooms
  create|rename` prompt unless `--force`, exit 6 non-interactively *before*
  any network call, and verify with `GetSpace`/`GetHomeGraph` before reporting
  success — Foyer's write responses are empty, so a 2xx proves nothing.
- **Control doesn't prompt; structure does.** `devices set` / `rooms set`
  are reversible state changes and run without confirmation (they still
  read back). Moves, renames, removes and room creation confirm or need
  `--force`.
- **No guessing field numbers against a real home.** Layouts live in
  `src/spaces.rs` with the decoded field numbers in doc comments; a new write
  RPC gets its layout from the app's descriptors (docs/api.md), not from
  trial requests. `spaces::LAYOUT_CONFIRMED` gates every write.

## The MCP tools

`src/mcp.rs` is `ghome mcp` (cli-common `DESIGN.md` §1.9). Two tools read; `ghome_device_set` and
`ghome_room_lights` change something, follow my-q-cli's pattern, and are not read-only. A write names one device or
room by its full name or id (`named_device`, `named_room`), reaches a device only when `kind` and `assigned_kind` are
both on `CONTROLLABLE_KINDS`, can send only a `Wanted`, and returns the state read back with `confirmed`, also when the
command's own answer was lost (`Sent::NoAnswer`). A room is the one group a tool reaches, and only its lights
(`is_a_light`). A new argument or tool goes on the lists in `src/mcp/tests.rs`, and a new tool in
`packaging/mcpb/manifest.json` and README's table. The definitions stay under 8,000 characters together. No tool
removes, renames or moves a device, edits a room or a routine, announces, sets volume or media, locks or unlocks,
or reaches `ghome api`, whatever the CLI can do. `GetHomeGraph` carries the address, linked users, partner ids and
`local_auth_token`: `read_state` and `device_item` copy named fields only, and must go on doing so. Each request waits
15 s, no command goes out 60 s after the call began (`SEND_DEADLINE`, asked right before each send through `Foyer::write`,
because a mint can wait on a keychain prompt), and no read-back starts after 85 s, so a write answers inside `--http`'s 120 s.

This crate pins cli-common **v0.25.0**, the spec it conforms to.

## Releasing

Bump `version` in `Cargo.toml` and `packaging/mcpb/manifest.json`, add to `CHANGELOG.md`, merge, then push the tag
`v<version>`. The workflow checks the tag, crate and manifest agree, builds the tarballs `self-update` finds and the
`.mcpb`, and publishes once all of them are there. A pull request that touches the workflow, packaging or dependencies
runs everything but the publish as a dry run.

## Tests

Offline, always. No test may read the OS keychain: `cargo test` produces an
ad-hoc-signed binary that macOS treats as a new identity, so a credentialed
command would prompt once per keychain item on every run. The surface tests
and `tests/mcp_stdio.rs` use a nonexistent `GHOME_CONFIG` so credentialed commands
stop at exit 3 before the keychain. Live checks go through the installed binary by hand.

## Safety & privacy (this will be a public repo)

- Nothing tracked in git may carry a real email, home/room/device UUID,
  address, or token. `tests/cli_surface.rs` scans `git ls-files` for those
  shapes. Fixtures use `home-…`/`room-…`/`dev-…` ids and `@example.com`.
- Runtime output legitimately carries all of that; that's the tool's job.
- `auth login` registers an "Android device" on the Google account; Google
  may email a security alert. Don't run it in tests or CI.

## Roadmap

`ROADMAP.md` is the capability ledger. A new command lands with its row
flipped to **done** in the same commit; a blocked one says what it's
waiting on. It exists so the plan survives a lost session.

## Definition of done

`make verify` green, CI green, the change dogfooded through the installed
binary, `--json` and human output in sync, `docs/api.md` matching reality,
and no secrets or personal data anywhere in the diff.
