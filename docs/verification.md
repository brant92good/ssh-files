# Verification and limits

The first release is still in qualification. The release workflow builds all
five targets, packages only the tested UI, then runs each platform's installer
against the actual tagged HTTPS downloads. Passing a source test is not the same
as qualifying a release asset.

## Observed before publication

Windows x64 has passed real, hidden ConPTY tests against the compiled static-CRT
UI. The latest candidate's two actual SFTP tests passed in 22.73 s; its preceding
candidate was also independently replayed in 23.22 s. They covered Unicode
multiline path input, separate review/start keys, upload and download bytes,
collision pause/rename, recursive folders, and a fresh browser request while a
transfer was deliberately stalled. Cancellation and confirmed close stopped
both owned SSH connections; abrupt process termination did too. These are
fixture observations, not timing promises across computers.

The Linux x64 proof passed actual bracketed paste, the same file operations,
strict host-key refusal, explicit proxy routing and lost-finalization replies.
PTY EOF and SIGTERM stopped both owned SSH processes and left no interrupted
final file. The first EOF run exposed a terminal-library error-reporting abort
after successful transport cleanup. Its teardown fix has passed a targeted unit test and
Windows regression checks; Linux clean-exit replay remains pending.

Unit and integration tests cover bounded SFTP frames and raw directory pages,
unsafe and lossy names, path collisions, recursive limits, process registry
races, stale results, and cancelled local I/O. The ordinary PTY test verifies
that text input is actually consumed before closing.

## Reproducing the checks

- [Native UI and owned PTY tests](../tests/native_pty.rs)
- [Actual SSH route resolution](../tests/route.rs)
- [Disposable Linux OpenSSH fixture](../scripts/check_linux.py)
- [Terminal-close process ownership](../scripts/check_pty_shutdown.py)
- [Withheld successful-finalization reply](../scripts/check_lost_ack.py)
- [Compiled installer checks](../scripts/check_native_install.py)
- [Public HTTPS installer checks](../scripts/check_release_install.py)
- [Five-target release workflow](../.github/workflows/native.yml)

The SSH fixtures use generated keys, explicit configs, owned loopback servers
and temporary files. They never use normal host aliases or the user's agent.
Python and the pyte screen parser are development tools; they are not shipped
with the app. The transport probe is also excluded from release assets.

## Limits

macOS desktop use, Windows agent-only authentication, ProxyJump, native desktop
file drag/drop, servers without the hardlink extension, disk-full and
denied-close scenarios remain unqualified. OSC52 copy is a request to the
terminal and depends on its clipboard policy.

SFTP v3 and local ancestor checks are not a sandbox against another process
concurrently replacing path components. A cancelled local/network-filesystem
syscall may continue after its future is dropped. The view quarantines uncertain
local work and bounds runtime shutdown; it cannot cancel every operating-system
call. Lost finalization responses are reported as unknown, with final and partial
paths retained for inspection. They are never automatically retried or deleted.
