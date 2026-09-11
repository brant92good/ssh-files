#!/bin/sh
# Explicit native beta only. Never updates PATH, shell profiles or normal Files.
set -eu
fail() { printf '%s\n' "$*" >&2; exit 1; }
download() {
    if command -v curl >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -fsSL --connect-timeout 15 --max-time 120 "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then wget --timeout=120 -q "$1" -O "$2"
    else fail 'Install curl or wget first.'; fi
}
checksum() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d ' ' -f 1
    else fail 'A SHA-256 tool is required.'; fi
}
safe_path() {
    case "$1" in /*) ;; *) fail 'Beta paths must be absolute.';; esac
    case "$1" in /|*/../*|*/./*|*/..|*/.|*//*) fail 'Use a canonical separate beta directory.';; esac
    inspect=$1
    while :; do
        [ ! -L "$inspect" ] || fail 'Beta paths must not traverse symbolic links.'
        [ "$inspect" != / ] || break
        inspect=${inspect%/*}; [ -n "$inspect" ] || inspect=/
    done
}
system=$(uname -s)
overlap() {
    first=$1; second=$2
    # Default macOS volumes ignore case. Refuse ASCII case variants even on
    # case-sensitive volumes; existing inode comparisons also cover non-ASCII
    # case/normalization aliases without inventing a filesystem Unicode fold.
    if [ "$system" = Darwin ]; then
        first=$(printf '%s' "$first" | LC_ALL=C tr '[:upper:]' '[:lower:]')
        second=$(printf '%s' "$second" | LC_ALL=C tr '[:upper:]' '[:lower:]')
    fi
    case "$first/" in "$second/"*) return 0;; esac
    case "$second/" in "$first/"*) return 0;; esac
    ancestor=$1
    while [ "$ancestor" != / ]; do
        if [ -d "$ancestor" ] && [ -d "$2" ] && [ "$ancestor" -ef "$2" ]; then return 0; fi
        ancestor=${ancestor%/*}; [ -n "$ancestor" ] || ancestor=/
    done
    ancestor=$2
    while [ "$ancestor" != / ]; do
        if [ -d "$ancestor" ] && [ -d "$1" ] && [ "$ancestor" -ef "$1" ]; then return 0; fi
        ancestor=${ancestor%/*}; [ -n "$ancestor" ] || ancestor=/
    done
    return 1
}
regular() { [ ! -L "$1" ] && { [ ! -e "$1" ] || [ -f "$1" ]; } || fail 'An installation file is not a regular file.'; }
version=${SSH_FILES_BETA_VERSION:-}
[ "$(printf '%s' "$version" | tr -d '\r\n')" = "$version" ] && printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-beta\.[1-9][0-9]*$' || fail 'Specify an exact x.y.z-beta.N version; there is no implicit latest beta.'
install_root=${SSH_FILES_BETA_INSTALL_DIR:-${XDG_DATA_HOME:-"$HOME/.local/share"}/ssh-files-beta-install}
install_root=${install_root%/}; safe_path "$install_root"
stable_root=${XDG_DATA_HOME:-"$HOME/.local/share"}/ssh-files-install
safe_path "$stable_root"
overlap "$install_root" "$stable_root" && fail 'Beta must be separate from the ordinary installation.'
if [ -n "${SSH_FILES_WORKSPACE_ROOT:-}" ]; then
    safe_path "$SSH_FILES_WORKSPACE_ROOT"
    overlap "$install_root" "${SSH_FILES_WORKSPACE_ROOT%/}" && fail 'Beta must be separate from Workspace.'
fi
inspect=$install_root
while :; do
    [ ! -e "$inspect/.ssh-files-installer" ] && [ ! -e "$inspect/.workspace-installer" ] && [ ! -e "$inspect/bin/terminal-workspace.exe" ] || fail 'Beta must not be nested in an ordinary app or Workspace.'
    [ "$inspect" != / ] || break
    inspect=${inspect%/*}; [ -n "$inspect" ] || inspect=/
done
owner=ssh-files-beta
marker="$install_root/.ssh-files-beta-installer"
command_path="$install_root/bin/ssh-files-beta"
for path in "$marker" "$install_root/version" "$command_path"; do safe_path "$path"; regular "$path"; done
if [ -e "$marker" ]; then
    [ "$(cat "$marker")" = "$owner" ] && [ -f "$install_root/version" ] && [ -f "$command_path" ] || fail 'Incomplete or unknown beta ownership; inspect this directory.'
    previous_owner=$(cat "$marker")
elif [ -d "$install_root" ] && [ -n "$(ls -A "$install_root")" ]; then
    fail 'Choose an empty directory or a complete owned beta installation.'
else previous_owner=''; fi
case "$system/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin;;
    Darwin/x86_64) target=x86_64-apple-darwin;;
    Linux/x86_64) target=x86_64-unknown-linux-musl;;
    Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-musl;;
    *) fail 'No compiled beta for this operating system/CPU.';;
esac
stage=''; owned_lock=0; changed=''; backed=''; committed=0; preserve=0
destination() {
    case "$1" in binary) printf '%s' "$command_path";; owner) printf '%s' "$marker";; version) printf '%s' "$install_root/version";; esac
}
finish() {
    status=$?
    trap - 0 INT TERM HUP
    if [ "$committed" = 0 ]; then
        for item in $changed; do
            path=$(destination "$item")
            case " $backed " in
                *" $item "*)
                    # If the old move failed (or the signal preceded it), the
                    # original destination still exists and must be left alone.
                    if [ -f "$stage/previous-$item" ]; then
                        if [ -e "$path" ] || [ -L "$path" ]; then rm -f "$path" || preserve=1; fi
                        mv "$stage/previous-$item" "$path" || preserve=1
                    fi;;
                *) if [ -e "$path" ] || [ -L "$path" ]; then rm -f "$path" || preserve=1; fi;;
            esac
        done
    fi
    if [ "$preserve" = 1 ]; then
        printf '%s\n' "Rollback incomplete; retain $stage and inspect before retrying." >&2
        status=1
    elif [ -n "$stage" ]; then
        case "$stage" in "$install_root"/.install-beta.*) rm -rf -- "$stage";; *) printf '%s\n' 'Unsafe stage cleanup refused.' >&2; status=1;; esac
    fi
    if [ "$owned_lock" = 1 ]; then rmdir "$install_root/.install-beta.lock" || status=1; fi
    exit "$status"
}
mkdir -p "$install_root"
mkdir "$install_root/.install-beta.lock" || fail 'Another beta install or unfinished lock exists; inspect it first.'
owned_lock=1
trap finish 0
trap 'exit 130' INT
trap 'exit 143' TERM HUP
current_owner=''; if [ -f "$marker" ]; then current_owner=$(cat "$marker"); fi
[ "$current_owner" = "$previous_owner" ] || fail 'Ownership changed during preparation.'
stage=$(mktemp -d "$install_root/.install-beta.XXXXXXXX")
asset=ssh-files-$target
url=https://github.com/brant92good/ssh-files/releases/download/v$version/$asset
expected=${SSH_FILES_BETA_SHA256:-}
if [ -n "${SSH_FILES_BETA_BINARY:-}" ]; then
    [ -f "$SSH_FILES_BETA_BINARY" ] && [ -n "$expected" ] || fail 'Local fixture binaries require an existing file and explicit SHA256.'
    cp "$SSH_FILES_BETA_BINARY" "$stage/binary"
else
    download "$url" "$stage/binary"
    if [ -z "$expected" ]; then
        download "$url.sha256" "$stage/checksum"
        expected=$(cut -d ' ' -f 1 "$stage/checksum" | tr -d '\r\n')
        [ "$(cat "$stage/checksum" | tr -d '\r\n')" = "$expected  $asset" ] || fail 'Invalid exact release checksum sidecar.'
    fi
fi
case "$expected" in ''|*[!a-fA-F0-9]*) fail 'Invalid checksum.';; esac
[ "${#expected}" = 64 ] || fail 'Invalid checksum length.'
expected=$(printf '%s' "$expected" | tr A-F a-f)
[ "$(checksum "$stage/binary")" = "$expected" ] || fail 'Download checksum mismatch; existing beta files preserved.'
chmod 755 "$stage/binary"
# The verified native metadata command opens no UI or connection.
actual_version=$("$stage/binary" --version) || fail 'The candidate version command failed.'
[ "$actual_version" = "ssh-files $version" ] || fail 'The candidate has the wrong version.'
printf '%s\n' "$owner" > "$stage/owner"
printf '%s\n' "$version" > "$stage/version"
safe_path "$install_root/bin"; mkdir -p "$install_root/bin"
for item in binary version owner; do
    path=$(destination "$item"); safe_path "$path"; regular "$path"
    # Register recovery before moving anything, including signal interruption
    # between an old-file rename and its replacement.
    changed="$item $changed"
    if [ -f "$path" ]; then backed="$item $backed"; mv "$path" "$stage/previous-$item"; fi
    # Rename new inodes; never truncate hardlinked files or an old backup.
    mv "$stage/$item" "$path"
done
[ "$(checksum "$command_path")" = "$expected" ] || fail 'Published beta binary readback failed.'
committed=1
printf '\n%s\n' "Installed SSH Files BETA $version. PATH, shell profiles and normal Files were unchanged."
printf 'Command: %s\n' "$command_path"
printf '%s\n' 'Use that exact path with --host YOUR_SSH_ALIAS; installation opens no connection or window.'
