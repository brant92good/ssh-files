# Verification and limits

The [0.1.0 beta release](https://github.com/brant92good/ssh-files/releases/tag/v0.1.0)
contains five compiled UI binaries. The release workflow tests and packages each
one, publishes immutable assets, then exercises the tagged HTTPS installer on
Windows, both Linux architectures, and both macOS architectures.
[The tagged release run](https://github.com/brant92good/ssh-files/actions/runs/34450038417)
passed all eleven jobs at commit `e8cde5f`.

## What was exercised

| Platform | Observed checks |
| --- | --- |
| Windows x64 | Static-CRT UI; real hidden ConPTY and OpenSSH file transfers; fresh install, update, checksum rejection and polluted environment |
| Linux x64 / ARM64 | Musl UI; real PTY and OpenSSH file transfers, bracketed paste, terminal EOF and SIGTERM cleanup; actual HTTPS installs |
| macOS Apple silicon / Intel | Native build, library and ordinary PTY tests, Clippy, packaging and actual HTTPS installs; desktop use remains beta |

The published Windows binary was also replayed locally through the hidden
ConPTY fixture: both actual SFTP tests passed in 22.75 s. Its SHA-256 is
`591f7609d470ca970b59df0212cf904a74af8474fcb2ae594ef65955220a51f3`,
matching the release sidecar and the separately exercised tagged installer.
These are fixture observations, not timing promises across computers.

Actual transfer tests compare uploaded/downloaded bytes, preserve existing
files, rename collisions, and review recursive folders before starting. The
browser must fetch a newly created remote marker while a transfer is stalled,
which checks that its separate connection remains usable. Pasted newlines and
Enter must not start transfers; F9 does.

Transport tests use strict host-key refusal, an explicit ProxyCommand, a
selected hostname override, deliberate network stalls, and a successful server
finalization whose reply is withheld. The last case must preserve both paths
and report completion as unknown; it must not retry or delete them.

Both Linux architectures now close after PTY EOF and SIGTERM with exit status
0, both owned SSH processes gone, and no interrupted final file. A previous run
found a terminal-library destructor abort after successful SSH cleanup. The
regression now requires both the process exit status and cleanup assertions.

Unit and integration tests cover bounded SFTP frames and raw directory pages,
unsafe and lossy names, path collisions, recursive limits, process registry
races, stale results, and cancelled local I/O. The ordinary PTY test verifies
that text input is consumed before closing. These observations do not qualify
every SSH server, credential provider or filesystem.

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
