# Install an explicit SSH Files beta

The ordinary installer remains pinned to 0.3.0. Beta installation uses a separate
directory and command; it changes no Workspace binaries, shortcuts, profiles,
PATH entries or user SSH configuration. The examples target 0.4.0-beta.1. Check
[the immutable release](https://github.com/brant92good/ssh-files/releases/tag/v0.4.0-beta.1)
and its `released-install` workflow results before installing. A source branch
alone does not mean these assets or HTTPS checks are available.

## Windows

Download the versioned installer into a temporary file and run it:

```powershell
$installer = Join-Path $env:TEMP ('ssh-files-beta-' + [guid]::NewGuid().ToString('N') + '.ps1')
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/brant92good/ssh-files/v0.4.0-beta.1/install-beta.ps1 -OutFile $installer
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installer -Version 0.4.0-beta.1
& "$env:LOCALAPPDATA\Programs\SSHFilesBeta\bin\ssh-files-beta.exe" --host YOUR_SSH_ALIAS
```

`-InstallDir` selects another empty or installer-owned beta directory.
`-WorkspaceRoot` additionally protects a custom Workspace source directory.
The default ordinary install directory and marked Workspace/app ancestors are
always protected. Short aliases resolve to the existing long path; reparse
points and overlap are refused. The executable reports `ssh-files 0.4.0-beta.1`
with `--version`, and the interactive header visibly identifies the beta.

## Linux and macOS beta

```sh
installer=$(mktemp /tmp/ssh-files-beta-install.XXXXXXXX)
curl -fsSL https://raw.githubusercontent.com/brant92good/ssh-files/v0.4.0-beta.1/install-beta.sh -o "$installer"
SSH_FILES_BETA_VERSION=0.4.0-beta.1 sh "$installer"
"${XDG_DATA_HOME:-$HOME/.local/share}/ssh-files-beta-install/bin/ssh-files-beta" --host YOUR_SSH_ALIAS
```

Set `SSH_FILES_BETA_INSTALL_DIR` to override the separate absolute directory.
Set `SSH_FILES_WORKSPACE_ROOT` to protect a custom Workspace directory. Symlink
traversal and normal-install overlap are refused. No shell profile is modified.

## Update and return to the baseline

Run the installer again with an exact published beta version. There is no
implicit `latest` beta. It verifies the downloaded SHA256 and native version
before replacing files, uses new inodes for hardlinked files, and attempts to
restore previous files if publication fails. An incomplete rollback is an error
that names the retained files for inspection; it is never reported as success.
Windows successful updates retain uniquely named previous files beside their
original destinations. Neither installer treats arbitrary nonempty directories
as owned. Existing unrelated files in a complete owned beta directory remain.

Return to the ordinary Workspace SFTP entry or ordinary `ssh-files` path to use
0.3.0. Closing the beta ends its own transfer/browser processes; it has no saved
machine database or persistent controller to migrate. Installer rollback does
not undo file transfers you deliberately performed in the beta.

For offline qualification only, `-Binary`/`-Sha256` on Windows or
`SSH_FILES_BETA_BINARY`/`SSH_FILES_BETA_SHA256` on Unix accepts a local binary
with an explicit expected digest. Published HTTPS checks use the official
versioned downloads without that substitution. Source, target CI, publication,
HTTPS qualification and personal installation remain separate recorded stages.
