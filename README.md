# ghome — Google Home rooms and devices from the terminal

`ghome` is Google Home from the terminal, for people and for agents. It
reads the whole home graph (homes, rooms, devices, who owns what, what's
online), audits and fixes where every device is filed, controls devices and
whole rooms, and exposes the private API behind the Google Home app as a raw
passthrough. It started because "turn off the office lights" only works
when the office lights are actually in the Office, and nothing in the app
audits that across a hundred devices.

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
cargo install --git https://github.com/piekstra/google-home-cli --locked
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

### Controlling devices

```console
ghome devices state "Office Hex"                      # online, on, brightness, colour temp, raw traits
ghome devices set "Office Hex" --off
ghome devices set "Office Hex" --on --brightness 40 --temp 2700
ghome devices set "Office Hex" --color teal                 # names, #rrggbb, rgb(), hsv()
ghome devices set "Kitchen display" --volume 20 --media pause
ghome rooms set Office --off                          # every light in the room
ghome rooms set Office --on --all                     # plugs, switches and speakers too
```

Control is reversible, so it doesn't prompt; the resulting state is read
back from Google and reported. `rooms set` targets lights unless `--all`,
so "turn off the bedroom" never cuts a plug you'd rather keep on.

### Routines

```console
ghome routines list                                   # which can be run on demand
ghome routines run "Good night"                       # prompts; --force to skip
ghome routines validate --file morning.yaml           # Google's own validator, nothing saved
ghome routines create --file morning.yaml             # validated, then saved; prompts
ghome routines update "Morning" --file morning.yaml   # replace its script
ghome routines delete "Morning"
```

Scripts are the Home script editor's YAML (`metadata` + `automations` with
`starters` and `actions`; devices as `"Name - Room"`), so anything the
editor at home.google.com/automations accepts works here, `--file -` reads
stdin, and a created automation shows up in the app and can be run with
`routines run`. A rejected script lists Google's problems as `line:col
message` (rows under `errors` in `--json`) and exits 2.

### Announcements

```console
ghome announce "Dinner is ready"                                  # every speaker and display on the LAN
ghome announce "Dinner is ready" --room Kitchen                   # that room's devices (uses the home graph)
ghome announce "Dinner is ready" --device "Kitchen display" --volume 40
ghome announce "Doorbell" --url https://example.com/chime.mp3     # any audio the device can fetch
```

Announcements go over the LAN with the Cast protocol, so `--device` and
the default need no Google account at all; speech is generated by Google
Translate's text-to-speech (unofficial, under 200 characters, `--lang` for
other languages). Whatever was playing is interrupted, as with a broadcast;
`--volume` is put back afterwards. `--via cloud` is the Home app's own
broadcast path, which Google currently rejects (see docs/api.md).

### Fixing rooms

```console
ghome devices move "Office Lamp" --room Office        # prompts; --force to skip
ghome devices place "Office Hex" --room Office        # from `devices list --unplaced`
ghome rooms create Storage --kind OTHER               # kinds: `rooms types`
ghome rooms rename "Guest Bedroom" Gym
ghome rooms delete Loft                               # only when empty
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
`expected_room`), `unassigned`, `unfiled` (the vendor row that matched names
no room, so the vendor app is what needs fixing), `unplaced`, `unmatched` (an
expectation with no Google device), or `local_only` (an expectation flagged
`"cloud": false`, a Bluetooth-only device Google Home can never see;
informational). `source` says what decided a row (`expect` or `name`);
rows that came from a vendor row also carry `vendor` when it named one.

The vendor CLIs emit this shape directly:

```console
govee --json rooms devices | ghome audit --expect - --problems
tplc --json rooms devices | ghome audit --expect - --problems    # Tapo rooms
tplc --json groups devices | ghome audit --expect - --problems   # Kasa groups
```

`--apply` turns the audit into the fixes it implies: one `devices move` or
`devices place` per `mismatch`, `unassigned` or `unplaced` row whose expected
room came from `--expect` (the name heuristic only ever suggests). It lists
the changes, asks once (`--force` to skip; required non-interactively), reads
every move back, and exits 5 if any failed:

```console
govee --json rooms devices | ghome audit --expect - --apply
```

### Raw API

```console
ghome api POST StructuresService/GetHomeGraph --data '[]'
```

Bodies are positional protobuf arrays; see [docs/api.md](docs/api.md).

## Use with Claude

`ghome mcp` serves four tools over MCP, so Claude can see your Google Home and, when you ask, change one
light, plug or wall switch, or the lights of one room. There is no hosted service: the tools run on your
computer, in your copy of `ghome`, and call Google with your credential.

| Tool | Does | Changes a device |
| --- | --- | --- |
| `ghome_devices` | What do I have? Name, kind, room, online and what the tools can change on it, narrowed by `name` and `room` | no |
| `ghome_device_state` | Is it on? Online, power, brightness, color or color temperature, and how a speaker or a lock reports | no |
| `ghome_device_set` | Set one light, plug or wall switch: `power`, `brightness`, `color` or `color_temp_k` | **yes** |
| `ghome_room_lights` | Set the lights of one room, and nothing else in it | **yes** |

The two that change a device are not read-only and carry `anthropic/requiresUserInteraction`, so a client
that honors them asks you before every call. `ghome_device_set` takes one device by its full name or its id,
and `ghome_room_lights` one room the same way. Part of a name is refused however unique, so "all" cannot
reach the hallway light, a name two devices or rooms share is refused, and there is no list, wildcard or
"everything". A write reaches only a device that Google calls a light, an outlet or a switch **and** that you
have not typed as something else, so a heater or a garage door on a smart plug is refused, and so is a lock
you typed as a light. A room's plugs and switches stay as they are unless you typed one as a light, and its other devices always do.

Each returns the state read back from Google, about 1.5 and 4.5 seconds later, with `confirmed`: `true`
only when that state shows the change. If it does not, or the read fails, a `note` says so. That is not a
failure, and Claude is told not to send the command again. If the command's own answer is lost, the state read
back settles it the same way, and the `note` says the outcome is unknown until it does. A write that reaches its
command 60 seconds or more after the call began sends nothing and says so.

No tool can remove, rename or move a device, edit a room, run or edit a routine, make an announcement, set a
speaker's volume or media, unlock a door, change your account or share access, and none reaches `ghome api`.

Results are concise by default (`response_format: "detailed"` adds a device's id, model and home), 25 records a
page with a cursor for more. A field a device left out is absent and never a default, and a list that could not
read who is online says so in `note`. Each page reads Google again, so a device added between pages can repeat
or be skipped: for a count, read one call with `limit` up to 200. Device and room names are yours; the server
tells Claude to treat them as data, not instructions. Your home's address, the people linked to it, a device's
`local_auth_token` and its partner ids never reach a result. Tools wait 15 seconds for each request.

**Sign in first.** Run `ghome auth login` in a terminal ([Setup](#setup)). Every path reads that credential from
the OS keychain (service `piekstra.ghome`), writes a short-lived token back beside it under a lock every `ghome`
process shares, and asks for nothing itself. When the credential ends the tools say so and ask you to run it
again. On macOS a program the keychain has not seen asks once per item, for the read and for the write of a new
token; choose Always Allow for each.

**Claude Code.** Add the `google-home-cli` plugin from the
[piekstra plugin marketplace](https://github.com/piekstra/claude-plugins):

```console
$ claude plugin marketplace add piekstra/claude-plugins
$ claude plugin install google-home-cli@piekstra-plugins
```

or register the server yourself:

```console
$ claude mcp add ghome -- ghome mcp
```

**Claude Desktop.** Download `ghome-<version>.mcpb` from the
[latest release](https://github.com/piekstra/google-home-cli/releases/latest) and open it. It has no settings:
install the CLI and sign in first, and the bundled `ghome` reads the same keychain credential. A new version of
the extension is a new program to the keychain and can ask again. macOS and Linux only;
[`SECURITY.md`](SECURITY.md) says why there is no Windows build.

Any other MCP client that launches a local server can run `ghome mcp`.

**Claude on the web and mobile.** claude.ai reaches tools through a remote connector, so `ghome` serves the
tools from your computer to a connector you deploy into your own Cloudflare account:

```console
$ ghome mcp --new-token            # once: a bearer token, saved in the keychain and printed for the Worker
$ ghome mcp --http --port 8762     # serve on 127.0.0.1 only
```

The connector, [cli-common's `connector/`](https://github.com/piekstra/cli-common/tree/main/connector#tools-from-your-computer-home-modules),
reaches that port through a Cloudflare Tunnel in your account; its README walks through the tunnel, the VPC
service, a launchd agent that keeps the server running, and the `HOME_GHOME_TOKEN` secret, with the threat model.
`--http` binds 127.0.0.1 only, refuses to start without the token, and refuses any request whose `Host` is not
loopback or that carries an `Origin`.

It serves the two reads only. `--allow-writes` adds `ghome_device_set` and `ghome_room_lights`, and then anyone
holding the token can change your lights and plugs through your Worker, with no prompt unless the client asks for
one; leave it off unless you want that. Your Google credential stays on your computer, but tool results do not:
your device and room names and their states pass through Cloudflare and reach Claude
([`SECURITY.md`](SECURITY.md)). While the computer sleeps, the tools answer that it is offline.

After `ghome self-update`, restart the server: a running one keeps the old code, and its launchd agent restarts it
only when it exits. Its start line names the version.

```console
$ launchctl kickstart -k gui/$(id -u)/com.piekstra.ghome-mcp-http
```

`self-update` does not reach a Claude Desktop extension, which runs its own copy. Install the new release's
`.mcpb` too.

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
