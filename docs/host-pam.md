# Optional host account sign-in

A Unix installation running under a dedicated account can use an existing private
host PAM bridge for its owner's browser login. For example, the agent can keep
running as `anchor` while its owner signs in as their normal host account. This
is optional: omitted or `null` `server.host_pam` preserves local account sign-in.
Windows does not accept this option.

An installation administrator can set the following through the existing
revision-checked management configuration API, or in its private configuration
before starting the service:

```json
{
  "server": {
    "host_pam": {
      "owner": "your-host-account",
      "socket": "/run/private-owner-login/owner-login.sock"
    }
  }
}
```

This fragment illustrates the new field, not a replacement for the complete
configuration. The administrator must already have configured and privately
exposed a compatible host bridge; Instance does not create it, change socket
permissions, copy a password, or provision a new account. The socket must be a
Unix socket owned by root or the service's current user, with no group or other
permission bits. Symlink socket entries are refused. Keep the containing path
under trusted administrative control: the pathname policy check is point-in-time,
not a filesystem lease against a privileged owner replacing the endpoint.

The protocol matches Amplifier Unified's optional host PAM client: one UTF-8 JSON
line containing only `username` and `password`, followed by exactly `OK\n` and
end-of-stream for success. An unavailable bridge, rejection, extra or malformed
reply, or failure to close within the single 20-second exchange deadline refuses
sign-in. There is no local PAM fallback. The existing bridge owns PAM account
checks and its fixed-owner policy; Instance retains its two-attempt concurrency
limit and failure cooldown. Local PAM calls still retain their concurrency slot
until the blocking worker ends, even if their caller times out.

`config.owner`, process identity, file ownership, ACP tokens, and work authority
do not change. Browser session `owner` and event actor identify the authenticated
sign-in account. The owner-facing requester-transfer control recognizes that
account as its browser principal; it does not reassign existing work or migrate
any historical principal. The login page accepts that username, and **Access &
credentials** distinguishes it from the installation owner. No password is saved
in configuration, browser view state, events, or the authentication binding.

Changing the account, socket, or PAM service rotates a private authentication
generation and invalidates browser sessions atomically. An in-flight login from
a previous generation cannot create a session, including when settings change
away and back before its reply. Restart also reconciles manual configuration
changes against the persisted binding. Unrelated changes and an upgrade using
unchanged default local sign-in retain existing cookies. Secure cookies, CSRF,
origin checks, token scopes, and the separate privileged native controls remain
unchanged. Existing active streams retain their normal bounded authentication
recheck interval; this setting does not cancel previously accepted work.

Authentication fields apply to new sign-in requests after a successful managed
configuration save; listener/TLS changes still require their normal restart.
The host bridge does not confer ACP access through a browser cookie, share an
engine's login session, or bypass either service's HTTPS authentication.
