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

To report a vulnerability, open a private security advisory on the GitHub
repository rather than a public issue.
