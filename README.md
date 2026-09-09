# ghome — Google Home rooms and devices from the terminal

`ghome` answers the question "which room does Google Home think this device
is in?" for every device on the account, flags the ones that are misfiled,
and gives agents a raw handle on the Home API. It exists because "turn off
the office lights" only works when the office lights are actually in the
Office — and nothing in the Google Home app audits that across sixty devices.

Conforms to [piekstra-cli spec v1](https://github.com/piekstra/cli-common)
(`--json` everywhere, standard exit codes, keychain-only secrets).

**Unofficial.** Not affiliated with or endorsed by Google. It speaks the
private API behind the Google Home app, reverse-engineered from the app and
documented in [docs/api.md](docs/api.md); Google can change or close it at
any time. Use it on your own account, at your own risk.

What's built and what's next: [ROADMAP.md](ROADMAP.md).

It also **fixes** them: move a device into a room, add a device that is
linked to your account but sitting in no home, create and rename rooms. Every
write prompts for confirmation unless `--force` and is read back from Google
before it is reported as done.

## Install

```console
cargo install --git https://github.com/piekstra/google-home-cli
# or a release binary: https://github.com/piekstra/google-home-cli/releases
```

## Setup

Google no longer lets non-browser clients sign in with a password, so the
credential comes out of a browser once:

```console
ghome config set username you@example.com
ghome auth login
```

`auth login` prints the steps: open `https://accounts.google.com/EmbeddedSetup`,
sign in, click **I agree** (the page will look stuck — expected), copy the
`oauth_token` cookie (starts with `oauth2_4/`) and paste it at the prompt.
The token is exchanged for a long-lived credential stored in the OS keychain
under `piekstra.ghome`; nothing is written to disk. Headless:

```console
pbpaste | ghome auth login --stdin
ghome auth status --json
```

## Usage

```console
ghome homes list
ghome rooms list
ghome rooms get Office                 # the room and its devices
ghome devices list                     # every device with its room
ghome devices list --room Office
ghome devices list --unassigned        # devices in no room at all
ghome devices list --agent <AGENT_ID>  # one vendor integration's devices
ghome devices list --status            # adds an online column (vendor-reported)
ghome devices list --offline           # stale hardware from a previous home
ghome devices get "Office Lamp"        # id, exact name, or unique partial
ghome devices agents                   # which integrations own devices
ghome rooms types                      # Google's room categories
```

Every command takes `--json` and emits one schema-tagged DTO
(`home-list/v1`, `room/v1`, `device-list/v1`, …). Multi-home accounts
narrow with `--home <ID|NAME>` or `ghome config set home <NAME>`.

### Fixing rooms

```console
ghome devices move "Office Lamp" --room Office        # prompts; --force to skip
ghome devices place "Office Hex" --room Office        # from `devices list --unplaced`
ghome rooms create Storage --kind OTHER               # kinds: `rooms types`
ghome rooms rename "Guest Bedroom" Gym
ghome devices rename "Old Lamp" "Storage Old Lamp"   # Google-side name only
ghome devices remove "Old Lamp"                       # vendor may re-add it on sync
ghome devices sync                                    # ask every vendor to re-sync
```

Non-interactive runs (`--json`, a pipe, an agent) must pass `--force` or they
stop with exit 6 before touching anything. Google only ever moves what you
name: no bulk fix exists on purpose, so an audit row becomes one explicit
command.

### Auditing rooms

```console
ghome audit                     # name-based: "Office Lamp" outside Office is flagged
ghome audit --problems          # hide the ok rows
ghome audit --expect rooms.json # compare against where the vendor app says they are
```

`--expect` takes a `device-rooms/v1` document (or a bare array), the shape
the vendor CLIs emit so the audit can be piped:

```json
{ "schema": "device-rooms/v1",
  "items": [
    { "id": "<vendor device id>", "name": "Office Lamp", "room": "Office", "source": "govee" }
  ] }
```

Devices are joined on the vendor id against Google's `partner_device_id`
(punctuation and case ignored), then on name. The result (`room-audit/v1`)
has a `summary` and one row per device: `ok`, `mismatch` (with
`expected_room`), `unassigned`, or `unmatched` (an expectation with no
Google device).

### Raw API

```console
ghome api POST StructuresService/GetHomeGraph --data '[]'
```

Bodies are positional protobuf arrays; see [docs/api.md](docs/api.md).

## Exit codes

0 ok · 2 usage · 3 auth (run `auth login`) · 4 not found · 5 upstream ·
6 confirmation required (a write without `--force` in a non-interactive run).

## Related

- [govee-cli](https://github.com/piekstra/govee-cli) and
  [tplink-cloud-cli](https://github.com/piekstra/tplink-cloud-cli) control
  the devices at the vendor; `ghome` audits where Google filed them.
- [cli-common](https://github.com/piekstra/cli-common) — the shared spec and
  crates.

## License

MIT
