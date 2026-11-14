# Validation

Run the repository's automated checks from a clean checkout. The source fixtures
use fictional identities and temporary state. They do not contain installed
agents, personal profiles, account credentials, or exported conversations.

```sh
cargo fmt --check
cargo test --locked
cargo build --locked
python3 scripts/test-installation.py
python3 scripts/test-native-installation.py
python3 scripts/test-provisioning.py
python3 tests/test_checkpoints.py
cargo clippy --locked --all-targets -- -D warnings
```

See the test scripts and CI workflow for prerequisites and platform conditions.
Browser tests use disposable local services and require Playwright/Chromium.
Linux service tests may require PAM development headers; isolated command tests
also need the documented OS isolation prerequisites. Opt-in real account, model,
or messaging tests remain separate from deterministic checks.

The [Actions page](https://github.com/microsoft/amplifier-constellation-instance/actions) records checks
against the published source. Earlier private deployment reports and their
transcripts are excluded. A passing fixture verifies its tested boundary, not
general agent quality, human approval, or live tenant compatibility.

## Consumer bundle validation

All six native CI targets package and run `tests/consumer_bundle_live.py` with
compiler, Git, Node and Python tools absent from PATH. The package's own Python
performs setup; a Unicode home/name, the published default engine, repeated
same-home starts and removal of the original download are checked. These runs
use no model or production state and do not register OS startup jobs.

Windows and macOS additionally run `tests/consumer_startup_live.py` against
actual current-user Scheduled Tasks or LaunchAgents. A clearly labelled build
with only its version changed supplies the baseline for update and rollback;
configuration, identity, credential, CA and retained database content must survive.
Windows ARM runs on Windows 11 ARM; Windows x64 CI runs on Server 2025 and does not
establish every consumer desktop account policy.

Linux CI uses the real offline systemd parser in
`scripts/test-native-installation.py`; that parser case is explicitly skipped
on other OSes. It does not claim a working Linux user-manager startup cycle.
A separate disposable host integration must establish that behavior. Native
model/provider authorization and existing used-session compatibility remain
separate qualifications from these new-installation fixtures.
