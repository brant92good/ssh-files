# File-manager features

This is the working scope for the next release. The released 0.2 browser has
hidden-file controls, mouse selection and transfers; select-all, sorting and
standalone folder creation are new changes under qualification.

| Task | Current behavior |
| --- | --- |
| Choose a server and saved folder | `ssh-sessions files` in SSH Sessions 0.8 opens a group → server → saved-path chooser. Standalone Files takes an explicit `--host`. |
| Browse both computers | Separate local and remote panes, typed filters, path entry, refresh and parent-folder navigation. |
| Hidden files | Hidden initially; `.` toggles them. Local Windows hidden attributes are respected. |
| Mouse navigation | Wheel targets its pane; click selects, double-click opens folders. |
| Select a batch | Space, Ctrl-click, Shift-click or drag within a pane. Ctrl+A selects its filtered list; Ctrl+Shift+A clears marks. Maximum 1,000 entries. |
| Sort | Ctrl+O cycles name and size, ascending and descending. Folders stay first; selection follows the same entry. |
| Create a folder | Insert opens a form in the selected pane; F9 confirms. Enter does not create it. Existing entries are kept. |
| Upload/download folders | Review a recursive batch, then start its serial queue. Hidden children are included; links and special files are rejected. |
| Progress and cancellation | Browse during transfer; Esc stops work. F4 shows partials and uncertain results. Closing Files stops its own connections. |
| Existing destination | Pause and choose a different copy name or skip. No overwrite mode. |
| Reconnect | F8 reconnects the browser. Failed transfers require an explicit decision; no automatic retry of an uncertain write. |
| Copy a path | F7 requests the terminal's clipboard support. Terminal policy determines whether it succeeds. |
| Drag from Explorer/Finder into Files | Not implemented as an OS drop feature. F6 accepts pasted paths and opens a transfer review. |
| Drag from Files into another app | Not implemented. Download to the local pane first, then use the local file manager. |
| Rename or delete an existing file | Not implemented. F2 and Delete in the queue change or skip a proposed copy; they do not manage existing files. Use an SSH shell or another SFTP client for maintenance. |
| Open/edit a remote file | No editor integration or upload-on-save. Download, edit locally, then upload under a new name; replacing the original requires deliberate external maintenance. |
| Permissions and ownership | No editing interface. Use the remote shell. |
| Resume or synchronize trees | Not implemented. Use a tool such as `rsync` when both endpoints support it. |
| Server-to-server copy | No direct remote-to-remote operation. Download locally, then upload through another selected connection. |

The mouse gestures above operate **inside the file list**. They do not establish
drag-and-drop between applications. Windows Terminal can deliver pasted text as
individual keys, which is why F6 has a dedicated editor and transfers need a
separate review/start action.

Existing-file maintenance, editor integration and resumed writes need different
conflict and recovery behavior from a new-file transfer. The current queue does
not provide those operations. Their absence remains an explicit product gap.

This inventory uses the task categories in [WinSCP's file panels](https://winscp.net/eng/docs/ui_file_panel)
and [basic tasks](https://winscp.net/eng/docs/task_index). It describes SSH Files'
implementation, not feature parity with a GUI client. See [controls and recovery](usage.md)
and [qualification results](verification.md).
