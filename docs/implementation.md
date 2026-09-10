# Architecture and development boundaries

SSH Files is a standalone SFTP browser and transfer queue. Workspace integration
passes an explicit route snapshot; it does not change the leaf's transport or
create another machine catalog.

## Interfaces

- `ssh-files --host HOST [--hostname ADDRESS] [--config PATH] [--user USER] [--port PORT]`
  opens the UI. Optional machine ID, route ID and label are display/snapshot
  metadata, never keys for another catalog lookup. Local and remote starting
  directories are explicit arguments. Existing proof commands remain available
  through `ssh-files-proof` while qualification continues.
  `--host` retains the selected alias and its local SSH options; optional
  `--hostname` supplies `-o HostName=ADDRESS` so a catalog's explicit edited
  route address is honored even when it differs from the alias's config file.
  Standalone callers that omit it keep ordinary alias resolution.
- SSH Sessions keeps F for Favorites. Its later reviewed Files action uses X
  if unused and passes the exact selected route/config snapshot. No fallback
  route is inferred; no second catalog is created.
- One separately owned raw SFTP browser session supplies bounded
  opendir/readdir requests. The pinned high-level session does not expose its
  raw handle and its convenience read_dir builds an unbounded vector. Keep the
  verified high-level transfer path instead of forking the dependency to add a
  getter. At most one browser and one serial-transfer SSH session use the same
  frozen route, trust flags and capped process/pipe implementation.

## UI and worker boundaries

Tab switches panes; arrows/Enter browse; Backspace goes up; Space marks entries;
printable text filters and F2 opens a path editor. F5 builds an Upload/Download
review list; a distinct F9 or Ctrl+S starts it, never Enter or the same F5.
F6 opens a local upload-path editor; pasted key text/newlines stay data there,
with F5 building the review. This matters because crossterm on Windows does not
provide an atomic Event::Paste. Main printable keys must not execute mutations
or quit. F1 opens help and F10/Ctrl+Q closes after confirming active transfers.
Show file names, types, sizes, completed/remaining items and active byte progress.
A bounded queue contains at most 1,000 items; only one transfer runs. Esc cancels
the current operation/queue. Pasted paths are data and reviewed before upload;
they are not native drag-out support or shell commands.

Use bounded request/result channels or one retained job handle per worker, and
a latest-value watch for progress. Cancel/quit use a separate watch/atomic path;
the event loop never waits to enqueue an ordinary request. Results carry an
operation generation and requested path so an old/cancelled result cannot
replace a newer view. Workers perform SSH/filesystem work separately from the event loop. Listing
and recursive planning stop at 10,000 inspected entries/depth 64, during raw
pagination, before accumulation. Local listing also stops while iterating.
Reject unsafe/lossy names visibly; do not silently collapse entries. No recursive
symlink/reparse/special-file following. Detect case/normalization collisions
before enqueuing. Empty non-EOF remote pages fail as no progress; duplicates and
rejected entries count toward the inspection limit, with an overall listing
deadline too. New destination directories and files use private modes where
supported. Existing final files are preserved, with skip/alternate-name choices;
there is no unqualified overwrite or delete action.

Local listing/planning runs in one tracked blocking worker whose actual closure
completion releases its slot. Cancelling its awaiting UI operation does not
permit another blocking worker while the old one remains alive. Synchronous
ancestor metadata checks never run in the draw/event handler. State is per
worker/operation, not a shared boolean that another operation could clear.

Cancellation closes the relevant owned connection; browser navigation and
transfer cancellation stay independent. Closing the view stops both. A local
I/O-in-flight flag is set before a Tokio filesystem await and cleared only when
that operation demonstrably completes. Dropping the awaiting future must not
clear it. After cancellation during local work, quarantine all new local
filesystem actions in that view instead of accumulating unknown blocking work.
Create-partial and final-link paths/state are recorded before their awaits;
cancelled local operations may already have created either path. Network-only cancellation
can reconnect on a later explicit transfer. Unknown finalization remains visible
with its final and partial paths; no automatic retry or deletion. Runtime
shutdown is bounded, while documentation preserves the local-filesystem limit.

## Release checks

Changes to these boundaries require tests for hostile directory pages, unsafe
and lossy names, destination collisions, recursive caps, bounded progress,
local-versus-network cancellation, actual PTY input and owned SFTP transfers.
Independent code and README review precedes publication; tagged assets must
also pass the real HTTPS installers. [The verification record](verification.md)
tracks observed coverage. macOS desktop use, agent-only Windows credentials,
ProxyJump and native drag-out remain outside verified claims until their own
tests pass.

The process registry shares each existing Group through Arc/Mutex, keeping the
sole Windows job handle and the unreaped Unix leader identity. UI close seals
new registration, snapshots group handles, releases the registry lock, and stops
groups directly. A delayed registration is stopped/reaped before Connection is
returned. Group kill/clear happens before leader reap; waits, filesystem calls
and worker joins never run while holding those locks. This lets app Drop/quit
stop SSH independently of a worker blocked in local filesystem code.
