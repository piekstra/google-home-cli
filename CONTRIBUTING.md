# Contributing

1. Read [AGENTS.md](AGENTS.md) — it is the house style and the safety rules.
2. `make verify` must be green before a PR. Tests are offline; nothing may
   touch the keychain or the network.
3. New Foyer findings (an RPC, a wire shape, a trap) go in `docs/api.md` with
   a date and a source, so nobody re-derives them.
4. Fixtures are scrubbed captures: keep the structure exact, replace every
   identifying value with the dummies described in `tests/fixtures/README.md`,
   and strip the `local_auth_token` slot.
5. A new write command must confirm (`--force`, exit 6 non-interactively),
   read its result back from Google, and take its wire layout from the app's
   descriptors — never from trial requests against a live home.
