# Portable agent images

An installed agent owns its presentation identity. Experiences may render the
same image without sharing transcripts, credentials, instructions or model
state. An image is presentation data, never an authority grant or prompt input.

In setup or **Configuration → Identity & purpose**, upload a PNG, JPEG or WebP
(up to 5 MiB). The service center-crops it to a 512 × 512 static PNG, strips
metadata, and saves it immediately. Replace or remove it in the same place.
Removing an upload restores a neutral generated image. No restart is needed.
The original upload is not retained. Images are available to authorized clients;
connecting an experience permits it to display the image to that experience's
participants. The service does not publish a public image URL.

## Management API and automation

- `GET /api/v1/identity`: identity metadata, requiring `observe`.
- `GET /api/v1/identity/avatar`: authenticated PNG, requiring `observe`.
- `PUT /api/v1/identity/avatar`: owner/admin mutation with JSON
  `{ "expected_revision": 0, "media_type": "image/png", "data": "<base64>" }`.
- `DELETE /api/v1/identity/avatar?expected_revision=1`: owner/admin removal.

Read `avatar_revision` before a mutation. A stale revision returns 409, including
current metadata. Browser mutations require the normal CSRF token; scoped API
tokens and `agent-instance api` support the same operations. Image values never
enter UI drafts or event logs. `identity.avatar.changed` records only revision,
actor and whether the image is uploaded or generated.

UI controllers can navigate to `configuration`, focus `identity.avatar.file`,
and call `agentInstanceControl.perform("identity.avatar.upload", {media_type,
data, expected_revision})` or `identity.avatar.remove`. The browser's file picker
is a local convenience; automation supplies bytes through the identical API.

## Optional ACP presentation extension: agent-identity/1

This is an **optional extension**, not a standard ACP requirement. Any agent or
wrapper can implement it; clients must work when it is absent. ACP's standard
`agentInfo` still describes the implementation. We do not repurpose it as a
portable individual identity or change the ACP protocol version.

Advertise in the `initialize` result:

```json
{
  "_meta": {
    "com.microsoft.amplifier.identity": {
      "schema": "agent-identity/1",
      "method": "_amplifier/identity/get"
    }
  }
}
```

After initialization, `_amplifier/identity/get` with `{}` returns:

```json
{
  "schema": "agent-identity/1",
  "agent_id": "stable-installation-id",
  "display_name": "Example teammate",
  "revision": "opaque-identity-revision",
  "avatar_revision": 1,
  "avatar": {
    "kind": "uploaded",
    "media_type": "image/png",
    "sha256": "sha256-of-decoded-png-bytes",
    "width": 512,
    "height": 512,
    "bytes": 12345,
    "data": "base64-png-bytes"
  }
}
```

`kind` is `uploaded` or `generated`. Consumers should leave an existing platform
icon alone for a generated image until they have managed an uploaded portrait;
a later generated image tells them to restore the neutral fallback. `revision`
changes with name or portrait; `sha256` enables image-only deduplication. Image
bytes are bounded to 1 MiB, static PNG, square 512–1024 pixels. Validate dimensions,
encoding and checksum; do not fetch arbitrary URLs from identity responses.

This operation uses the existing authenticated ACP channel and requires no
session or model call. HTTP metadata omits inline data, while ACP includes it
so clients need no HTTP management credential or special network route back to
the agent.

## Change notifications without a model process

An observer uses the same ACP command and credential. In `initialize.params`,
include:

```json
{"protocolVersion":1,"clientCapabilities":{},"_meta":{"com.microsoft.amplifier.identity":{"mode":"observe"}}}
```

A supporting server answers the ordinary ACP initialization with this additional
identity capability:

```json
{
  "schema":"agent-identity/1",
  "method":"_amplifier/identity/get",
  "observation":{"initialize_mode":"observe","subscribe":"_amplifier/identity/subscribe","changed":"_amplifier/identity/changed"},
  "observation_only":true
}
```

Only an explicitly acknowledged `observation_only: true` connection may be held
open for this purpose. Constellation handles it in the installation service,
without starting the runtime, even when the runtime is stopped or not yet set up.
It exposes no session, prompt, tool, transcript, or management mutation methods.
Ordinary ACP clients and runtime connections retain their existing capabilities;
this does not change ACP's protocol version. The ordinary initialization also
advertises the observation mode, with `observation_only: false`.

Call `_amplifier/identity/subscribe` with `{}`. The response is the current full
identity document, including image bytes. Changes then arrive as JSON-RPC
notifications named `_amplifier/identity/changed`, with the same document in
`params`. Registering precedes snapshot creation so changes cannot be lost in
between. Rename, upload, and removal change the revision. Duplicate revisions
are suppressed; unrelated instance events are never forwarded. Slow consumers
may be disconnected and reconnect for the current snapshot. Intermediate image
revisions are replaceable presentation state, not an event audit log.

Reconnecting and subscribing always returns the current snapshot. Clients may
coalesce bursts to the newest revision and deduplicate platform uploads by image
checksum. They should retry failed connections or failed/rate-limited uploads,
not poll healthy subscriptions. The authenticated connection and its token scope
remain authoritative; an expired or revoked token cannot receive further image
updates. No new credential is required.

An older or third-party peer may ignore the initialization hint. If it does not
explicitly acknowledge observation mode and these exact methods, obtain one
snapshot through its advertised get method and close the connection. Expose a
manual refresh or reconnect path; do not silently retain a costly agent process
or claim automatic change notifications for that peer.

Future surfaces can derive small thumbnails, circular crops or platform
renditions; future banner/animation roles should be separately negotiated rather
than overloading this avatar contract.
