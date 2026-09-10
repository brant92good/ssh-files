# Using SSH Files

## Connecting

`ssh-files --host devbox` opens the local working directory and the remote login
directory. The host is an existing OpenSSH alias or an explicit SSH destination.
There is no separate connection catalog and no automatic fallback route.

| Option | Purpose |
| --- | --- |
| `--host HOST` | Required SSH alias or destination |
| `--hostname ADDRESS` | Override the address while retaining the alias's options |
| `--config PATH` | Explicit OpenSSH configuration file (`ssh -F`) |
| `--user USER` | Override the SSH username |
| `--port PORT` | Override the SSH port |
| `--local PATH` | Start the local pane here |
| `--remote PATH` | Start the remote pane here |
| `--label TEXT` | A display name for this view |
| `--machine-id ID`, `--route-id ID` | Display metadata supplied by a launcher |

Arguments are passed to system OpenSSH separately. Files requires `BatchMode=yes`
and `StrictHostKeyChecking=yes`; it will not answer a password, passphrase or
first-connection trust prompt. Authentication must already work without a
prompt, through an available key or agent. Use ordinary `ssh` to verify a new
host key first; completing a password login alone does not make authentication
noninteractive. OpenSSH reads identity, include and proxy settings. Windows agent-only credentials
and ProxyJump still need their own qualification; a tested ProxyCommand fixture
does not certify every tunnel provider.

Files disables connection sharing, extra forwarding, remote commands and
backgrounding for its two owned subsystem processes. Closing Files stops those
processes. It does not close independently opened shells or Ports forwards.

## Browse and select

Tab changes panes. Arrows select an entry; Enter opens a folder. Backspace
removes a filter character, or goes up one directory when the filter is empty.
F2 opens a directory-path editor. Type to filter, or use F3 then Ctrl+A to clear
the filter. Space marks an entry; when nothing is marked, F5 uses the highlighted
entry. An exclamation mark identifies an unsupported name or file type; F4
explains it.

F8 refreshes the active pane and reconnects its browser if needed. The browser
and transfer queue are independent; a browser refresh does not restart or stop
the transfer.

## Review and transfer

F5 scans the selection and shows the destination list. Nothing is created until
F9 or Ctrl+S starts that review. Enter and a second F5 do not start transfers.
F2 changes a destination name; renaming a folder also updates its children.
Delete skips the highlighted item and any children of a skipped folder.

F6 opens the upload-path editor. Paste one path per line, or quote each path
when pasting several paths separated by spaces. The text is never a shell
command. Press F5 to prepare the review, then F9 to transfer. Windows terminal
pastes can arrive as individual keys; the dedicated editor and separate start
key keep newline input from starting an upload.

The queue runs one item at a time, with at most 1,000 reviewed entries. Recursive
planning stops at 10,000 inspected entries or depth 64. Links, reparse points,
special files, unsafe names and portable case/normalization collisions are
rejected. There is no recursive delete or overwrite command.

## Interruptions and existing files

Existing final files are preserved. A collision or transfer error pauses the
queue. Use F2 to choose another destination and review it, or Delete to skip the
failed item and continue. Esc stops the queue outside a dialog; Ctrl+C also
stops it inside a dialog. Closing with F10 or Ctrl+Q asks for a second press if
work is active.

Transfers write a uniquely named partial in the destination directory, then
create the final name with a hardlink that cannot replace an existing file.
Uploads need `hardlink@openssh.com` on the server; downloads need hardlinks on
the local filesystem. Unsupported finalization keeps and reports the partial.

If the finalization reply is lost, **completion unknown** means the final file
may already exist. Inspect it before retrying. F4 lists destination and partial
paths; retained paths are also printed when Files exits normally. A closed
terminal may no longer display that report. Partials are not automatically
resumed or deleted.

A local filesystem operation may continue after cancellation. Files waits for
an observed completion or disables further local work in that view and asks you
to reopen it. This prevents accumulating unobserved filesystem operations.

## Copy paths

F7 asks the terminal to copy the highlighted entry's path, or the open directory
if the pane is empty. It uses OSC52; clipboard permission and terminal support
determine whether copying succeeds. It does not read the clipboard. F4 shows
connection and entry details. Native drag-out is not implemented.
