# SSH Files contributor guide

Read [docs/implementation.md](docs/implementation.md) before changing transport,
workers or UI state, and [docs/verification.md](docs/verification.md) for the
observed platform coverage. System OpenSSH owns routing, authentication and
host-key checks. Never add automatic fallback routes or a second host catalog.

Use disposable keys, explicit fixture configs, loopback servers and temporary
files for integration checks. Tests must not connect to personal hosts, query a
user's agent, edit regular SSH settings, install services or focus desktop apps.
Keep the runtime free of Python and shell-script dependencies. Development-only
Python fixtures and the transport probe are excluded from release assets.

Preserve bounded packet/listing/queue sizes, process ownership, out-of-band
cancellation and destination no-clobber behavior. Keep retained partials and
uncertain finalization visible. A cancelled filesystem future does not prove
its blocking operation stopped. Read the worker invariants before changing it.

Run `cargo +1.94.0 fmt --check`, `cargo +1.94.0 test --locked --all-targets`
and `cargo +1.94.0 clippy --locked --all-targets -- -D warnings`.
Changes at the process/input/transfer boundary also require actual owned PTY and
OpenSSH execution, not just unit tests. macOS remains beta until its desktop
behavior is independently qualified. Do not replace or retag published assets.
Require independent code and README review before each initial platform release;
keep source, CI-tested, published and installed states distinct.
