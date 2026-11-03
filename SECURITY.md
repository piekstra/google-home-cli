# Security

`ghome` holds a long-lived Google credential (an Android master token) and
short-lived Bearers minted from it. Both live only in the OS keychain under
`piekstra.ghome`; nothing is written to disk and nothing is logged, at any
verbosity.

The credential is as powerful as the Google Home app: it can read every home,
room, and device on the account and, through the raw `api` passthrough,
control devices. Treat the keychain entry accordingly. `ghome auth logout
--forget` removes it; revoking the "Android" device from the Google account's
security page invalidates it server-side.

`ghome` fetches `GetHomeGraph`, which includes a per-device `local_auth_token`.
The CLI never reads, prints, or stores that slot.

`ghome` speaks Google's private API with an Android master token, neither of which Google offers for this; see the
README's note.

## The MCP server

`ghome mcp` serves four tools to an MCP client (Claude Code, Claude Desktop) over stdio; the server opens no port
unless run with `--http` (below). **What the tools return goes to Claude on every path**, as part of your
conversation: your device and room names, each device's kind, whether it is online, what it reports about its
power, brightness, color, volume, playback or lock, and with `response_format: "detailed"` its id, model and home's
name. `GetHomeGraph` also carries your home's address and coordinates, the email addresses of the people linked to
it, each device's partner ids and its `local_auth_token`, and no tool returns them.

Two tools read. `ghome_device_set` changes one device, whose full name or id the call must carry, and
`ghome_room_lights` the lights of one room, whose full name or id the call must carry. Each reaches a device only if
Google's kind for it and the kind you assigned are both a light, an outlet or a switch, and a room also needs you or
Google to call it a light. They are not read-only and carry `anthropic/requiresUserInteraction`, so a client that
honors them asks before every call. They can send only a power, brightness, color or color-temperature command, and
return the state read back. A tool cannot reach `ghome api`, routines, announcements, moving or renaming anything, a
speaker's volume or media, or any credential write except the cached bearer. `src/mcp/tests.rs` holds the definitions
to that, and the requests the live path makes against a stub. An approval is the client's: `--http` with
`--allow-writes` serves them to whoever holds the token, and a client that ignores the annotation does not ask.

A name in a result can come from anyone who has named a device or a room in the home, so the server tells Claude to
treat names as data and to change a device only on the user's direct request.

The server reads the credential on each call and holds it no longer than the call, so a new login applies on the
next. A tool never prompts for a credential or creates a keychain item: no credential is an error asking the user to
run `ghome auth login` in a terminal. It does write a newly minted bearer back, under a lock in the config directory
shared with every other `ghome` process, and only over the master token it came from, so a logout or a new login is
not undone. On macOS a binary the keychain has not granted raises a prompt on a call, and a server left unattended
waits on it. A write that reaches its command 60 s or more after the call began sends nothing and says so, so a prompt
answered late cannot switch a device. The Claude Desktop extension is such a binary, with a grant of its own.

The extension and the release have no Windows build, because the credential and its cached bearer are one item,
unmeasured against Windows's 2,560-byte credential limit, and an item too long for it would fail when it is written.

`ghome mcp --http --port <PORT>` serves over HTTP for the user's own Cloudflare Worker, which reaches it through the
user's own Cloudflare Tunnel (cli-common's `connector/`). It binds 127.0.0.1 only, with no flag for another address, and
refuses to start without a bearer token of 32 or more characters, from `GHOME_MCP_TOKEN` or the keychain item
`mcp-http-token` (`piekstra.ghome`), which `ghome mcp --new-token` makes from 256 random bits and replaces only with
`--overwrite`. Every request must carry the token (compared in constant time) and a `Host` of `127.0.0.1` or
`localhost`, and must not carry an `Origin`; there are no CORS headers. It serves the two reads only, and a write is
neither listed nor callable, unless started with `--allow-writes`. The Google credential stays on your computer: the
Worker sends tool calls and receives results, and never sees it. Results do pass through Cloudflare and reach Claude,
and anyone holding the token can read them, or with `--allow-writes` change your lights and plugs with no prompt. `ghome
auth logout --forget` removes the connector token. A server keeps its token until it exits, so restart it after
that, and after `self-update`.

## Reporting

To report a vulnerability, open a private security advisory on the GitHub
repository rather than a public issue.
