<p align="center"><img src="docs/assets/icon.svg" width="88" alt="SSH Files icon"></p>

<h1 align="center">SSH Files</h1>
<p align="center"><strong>SFTP that uses your SSH setup.</strong></p>

Two panes, keyboard navigation, and a transfer queue. Open an existing SSH alias,
review what will move, and browse the server while files transfer. No second
address book to maintain.

![SSH Files browsing local and remote files](docs/assets/browser.svg)

*Rendered by the native application with example files and routes.*

> **Development preview.** The first compiled release is being qualified.
> Source builds work now; public one-command installers will follow the release
> checks. macOS will remain beta until real desktop use is qualified.

## What it does

- **Use the route you already have.** Your system `ssh` reads the alias and
  configuration, including a custom config file and `ProxyCommand`.
- **Move files in either direction.** Filter a pane, mark files or folders, and
  review every destination before starting. F6 accepts pasted local paths.
- **Keep browsing during a transfer.** The browser and serial transfer queue
  have separate SSH sessions on the same selected route.
- **Keep existing files.** A collision pauses the queue. Choose another name or
  skip it; a transfer never silently replaces the destination.
- **See interrupted work.** Details shows retained partials and any destination
  whose completion could not be confirmed. Transfers stop when Files closes.

## Try the source build

Contributors need Rust 1.94.0 and system OpenSSH. The planned release installers
download a compiled binary; end users will not need Rust, Python or Git.

```sh
cargo +1.94.0 build --release --locked --bin ssh-files
./target/release/ssh-files --host devbox
```

On Windows, run `target/release/ssh-files.exe --host devbox`.
Replace `devbox` with an alias that already works with `ssh devbox`.

To use a separate SSH config or start in particular directories:

```sh
./target/release/ssh-files --host devbox --config ~/.ssh/work-config --local ~/project --remote /srv/project
```

Files requires noninteractive SSH authentication and a known, verified host key.
If SSH needs a login or trust prompt, complete that in an ordinary SSH session
first. Upload finalization currently requires the OpenSSH SFTP hardlink extension
and hardlink support on the destination filesystem. [Connection details](docs/usage.md#connecting).

## The short keyboard guide

| Key | Action |
| --- | --- |
| Tab, arrows, Enter | Switch panes, select, open a folder |
| Type, F3 | Filter files; edit or clear the filter |
| Space | Mark or unmark an entry |
| F5 | Review selected uploads or downloads |
| F9 / Ctrl+S | Start the reviewed queue |
| F6 | Paste local paths for upload |
| Esc / Ctrl+C | Stop work; Ctrl+C also works inside a dialog |
| F10 / Ctrl+Q | Close Files; confirm when a queue is active |

![Reviewing destinations before a transfer starts](docs/assets/review.svg)

In the review, **F2** changes a destination name and **Delete** skips an item.
**Enter does not start a transfer.** [All controls and recovery behavior](docs/usage.md).

## Who might find it useful

People who work in a terminal but still want to see both sides before copying
builds, datasets, logs, or project files. It works as a standalone app; the
planned SSH Sessions integration will open it on the selected device route.

Use `scp` for a quick known-path copy, or `rsync` when you need synchronization
or resumable bulk transfers. SSH Files is for browsing and reviewing a batch
without rebuilding the connection in another app.

## Platforms and current checks

| Platform | Current qualification |
| --- | --- |
| Windows x64 | Native tests and real hidden ConPTY + OpenSSH transfer flows passed |
| Linux x64 / ARM64 | Hosted UI qualification in progress; earlier transport proof passed |
| macOS Apple silicon / Intel | Planned compiled builds; beta, desktop use unqualified |

The tests include actual file bytes, collisions, cancellation, host-key refusal,
explicit proxy routes, and a server reply lost after a successful finalization.
[Evidence and limits](docs/verification.md) separate observed checks from work
still pending. There is no native drag-out or transfer-resume feature. Terminal
file-drop behavior is unqualified; **F6 paste paths** is the supported input flow.

## Contributing

Start with [the design and worker boundaries](docs/implementation.md). Run
`cargo +1.94.0 test --locked --all-targets` and
`cargo +1.94.0 clippy --locked --all-targets -- -D warnings`.
The Python scripts create disposable test servers; they are not app dependencies.
The transport probe is a developer binary and is excluded from release assets.

[MIT license](LICENSE) | [Dependency notices](docs/licenses/THIRD_PARTY_NOTICES.txt)
