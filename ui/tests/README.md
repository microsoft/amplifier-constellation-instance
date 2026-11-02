# Browser checks

These tests use Playwright as an optional development dependency. The production interface has no JavaScript build step or runtime package dependencies.

From the repository root:

```sh
npm install --prefix /tmp/agent-instance-browser-tests playwright@1.58.2
/tmp/agent-instance-browser-tests/node_modules/.bin/playwright install chromium
cargo build --bin agent-instance
PLAYWRIGHT_MODULE=/tmp/agent-instance-browser-tests/node_modules/playwright/index.mjs node ui/tests/fixture.mjs
PLAYWRIGHT_MODULE=/tmp/agent-instance-browser-tests/node_modules/playwright/index.mjs node ui/tests/real-service.mjs
```

Optional environment variables:

- `CHROMIUM_EXECUTABLE`: an existing compatible Chromium executable instead of Playwright's downloaded browser.
- `UI_TEST_ARTIFACTS`: a directory for reports and screenshots; otherwise a new temporary directory is created.
- `AGENT_INSTANCE_BINARY`: the Rust executable for the real-service test; defaults to `target/debug/agent-instance`.
- `PLAYWRIGHT_MODULE`: a module path; otherwise the tests import the locally resolvable `playwright` package.

`fixture.mjs` serves the actual UI under `/embedded/alpha/` against an in-memory protocol fixture. It checks the two-step default setup, account readiness/device sign-in, migration of old setup positions, local/remote Advanced disclosure, and meaningful activity despite 1,200 newer interface events. Search/category/problems/diagnostic filters and pagination use the server query contract. It also checks remote views, configuration/profile saves, secrets/tokens, credential/device-code exclusion from view requests, relative addressing, strict CSP, and mobile layout. PAM and provider sign-in responses are synthetic.

`real-service.mjs` creates a fresh temporary instance bound to loopback, starts the actual Rust service with HTTPS, and seeds a synthetic browser session in **that temporary database only**. It preconfigures `/usr/bin/cat` as a local runtime placeholder before opening the browser; that is not a model-execution test. It exercises two-step setup/start/stop/edit revision handling, optional Advanced controls, actual server Activity/diagnostic/problems filters, API/SSE/ack view control, profile event redaction, external configuration changes with local drafts, desktop/mobile layout, and CSP. Default-runtime provisioning, provider account connection, and actual model work are covered separately. It needs Python 3 with SQLite and must run as an ordinary OS user, as the service requires. Real PAM behavior is a separate test concern.

Both tests close their browsers and stop their own listeners. The real-service test removes its temporary instance after stopping it, while retaining sanitized reports and screenshots for inspection. It does not touch existing installations or running workshop services. Handled HTTP 409 revision conflicts are expected during concurrent view control and do not count as JavaScript or CSP failures.

The real-service suite additionally creates discipline/assignment records and a responsibility through the ordinary forms, puppeteers a draft through the view API, and verifies accept/start/block persistence. It confirms profile-use stays unknown before a real ACP prompt. Rust integration separately drives the actual responsibility MCP tools and profile delivery observations.

`background.mjs` starts a real temporary agent-instance service with a standard ACP subprocess fixture. It exercises explicit backend approval, pause/budget controls, a local artifact created after the browser closes, report review, responsive layout, programmatic view parity, cancellation, inspection and an explicit fresh retry. It additionally covers same-session multi-turn execution, the memory/conflict forms and semantic actions, and the native configuration editor against a versioned CLI fixture. It seeds only a temporary browser session and does not call a model or any external experience.

```sh
PLAYWRIGHT_MODULE=/tmp/agent-instance-browser-tests/node_modules/playwright/index.mjs node ui/tests/background.mjs
```


Native ChatGPT setup, without provider requests:

```sh
PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs node ui/tests/native-account.mjs
```

This starts a disposable Agent Instance and a fake local OAuth callback. Browser
navigation to the authorization page is intercepted with a fixture; no real
account is signed in or copied. It checks local/remote setup, reload, explicit
model activation, cancellation/revocation, CLI/view parity and narrow layout.
