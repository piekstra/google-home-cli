# CLAUDE.md

The canonical agent guide for this repo is **[AGENTS.md](AGENTS.md)** — read it
first. It covers build/test/lint, layout, conventions, and the safety rules.

Claude Code specifics:

- **Gate on `make verify`.** Don't report a change as done until it's green
  (fmt + clippy `-D warnings` + tests + smoke). Tests are fully offline.
- **Never run `auth login` to "test" it.** It registers a device on the real
  Google account. The token exchange is unit-tested offline; live login is
  the owner's call.
- **Writes act on a real home.** `devices move|place` and `rooms
  create|rename` change the owner's Google Home. Don't run them to "test";
  the encoders are unit-tested and the layouts documented in `docs/api.md`.
  Never add a write RPC by guessing its positional layout.
- **Secrets:** the master token and cached Bearer live in the OS keychain
  (`piekstra.ghome`). Never print them, put them on argv, or write them to a
  file.
- **"Deployed" means released + installed.** A change isn't live until the
  release workflow ships it and the binary is installed or `self-update`d.
- **Public repo, private home.** No real emails, UUIDs, addresses, or tokens in
  any diff — fixtures included (dummies only).
