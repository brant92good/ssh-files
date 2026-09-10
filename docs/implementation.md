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
Hidden entries are off by default in both panes. With an empty filter, `.` toggles
them; inside a filter/editor the period remains literal text. F3 edits a filter
that starts with a period. Local listings flag dotnames and Windows hidden
attributes in the existing worker; SFTP listings flag dotnames. Filtering changes
only the view: a selected folder's recursive transfer still includes its hidden
contents. Filtering removes now-invisible marks, and reloads preserve hidden
visibility preference. The footer shows hidden on/off.

Pointer hit tests use the renderer's exact file-list rectangles and stored
viewport offsets, excluding headers, path lines, footers and dialogs. Anchors
identify a pane, directory, full entry name and listing/filter revision. Mouse
events request a redraw before another mouse event is read. A new mapping,
keyboard event, resize or dialog invalidates gestures; wheel scrolling may keep
an active drag within its original pane. Double-click opens only the same safe
directory within 400 ms. Ctrl toggles a mark; Shift and drag build a complete
candidate set before enforcing the 1,000-entry limit, then replace the marks
atomically. Rejected entries are excluded. Mouse capture is restored before raw
mode on Windows, reversing capture's saved-console-mode order.

A bounded queue contains at most 1,000 items; only one transfer runs. Esc cancels
the current operation/queue. Pasted paths are data and reviewed before upload;
they are not native drag-out support or shell commands.

Standalone folder creation has a separate frozen request and worker result;
it cannot complete or start a transfer queue item. Insert opens the form and
F9 confirms a validated single portable child. The worker checks parent and
destination, then uses create-new mkdir. No existing entry is accepted as a
successful creation. Cancellation after mutation begins retains the destination
as uncertain. Shutdown also retains an unfinished worker's destination even
when its mutation flag is still false: lack of that flag does not prove a
running worker cannot write later. Only an observed pre-write failure can say
the folder was not created. The existing local worker slot remains occupied
until the actual blocking closure completes.

Ctrl+A builds the entire supported filtered selection before applying its cap.
Sorting keeps folders first and restores the selected name while retaining
marks. Both change the pane revision, ending stale pointer gestures. Rendered
filter footers and pointer geometry share the same variable row count. Help
lines fit the smallest supported popup, and its scroll limit follows the
actual popup height.

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
