param(
    [string]$Version=$env:SSH_FILES_BETA_VERSION,
    [string]$InstallDir=$env:SSH_FILES_BETA_INSTALL_DIR,
    [string]$Binary=$env:SSH_FILES_BETA_BINARY,
    [string]$Sha256=$env:SSH_FILES_BETA_SHA256,
    [string]$WorkspaceRoot=$env:SSH_FILES_WORKSPACE_ROOT
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) { throw 'This beta installer requires 64-bit Windows.' }
if ($Version -cnotmatch '\A(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-beta\.[1-9][0-9]*\z') { throw 'Specify an exact x.y.z-beta.N Version; there is no implicit latest beta.' }
function Resolve-BetaPath([string]$Path) {
    if (-not $Path -or $Path -match '[\x00-\x1f<>"|?*]' -or ($Path -replace '\A[A-Za-z]:','').Contains(':')) { throw 'Use a local Windows path without wildcards or streams.' }
    foreach ($part in ($Path -split '[\\/]')) { if ($part -match '[. ]$') { throw 'Dot segments and trailing dot/space aliases are refused.' } }
    $full=[IO.Path]::GetFullPath($Path).TrimEnd('\','/')
    if ($full -notmatch '\A[A-Za-z]:\\.+') { throw 'Use a local directory, not a drive root or network path.' }
    $drive=[IO.Path]::GetPathRoot($full); $resolved=$drive; $missing=$false
    foreach ($part in ($full.Substring($drive.Length) -split '\\')) {
        $entry=$null
        if (-not $missing) {
            $iterator=[IO.Directory]::EnumerateFileSystemEntries($resolved,$part).GetEnumerator()
            try {
                if ($iterator.MoveNext()) { $entry=$iterator.Current }
                if ($iterator.MoveNext()) { throw 'Ambiguous path component.' }
            } finally { $iterator.Dispose() }
        }
        if ($null -ne $entry) {
            $resolved=$entry
            if ([IO.File]::GetAttributes($resolved) -band [IO.FileAttributes]::ReparsePoint) { throw 'Beta paths must not traverse reparse points.' }
        } else { $missing=$true; $resolved=[IO.Path]::Combine($resolved,$part) }
    }
    return $resolved
}
function Test-Overlap([string]$A,[string]$B) {
    return $A.Equals($B,[StringComparison]::OrdinalIgnoreCase) -or $A.StartsWith($B+'\',[StringComparison]::OrdinalIgnoreCase) -or $B.StartsWith($A+'\',[StringComparison]::OrdinalIgnoreCase)
}
function Get-BetaHash([string]$Path) {
    $hash=[Security.Cryptography.SHA256]::Create(); $stream=$null
    try { $stream=[IO.File]::OpenRead($Path); return [BitConverter]::ToString($hash.ComputeHash($stream)).Replace('-','').ToLowerInvariant() }
    finally { if ($stream) { $stream.Dispose() }; $hash.Dispose() }
}
function Assert-Regular([string]$Path) {
    [void](Resolve-BetaPath $Path)
    if ((Test-Path -LiteralPath $Path) -and -not [IO.File]::Exists($Path)) { throw 'An installation file is not a regular file.' }
}
function Read-Version([string]$Path) {
    $start=[Diagnostics.ProcessStartInfo]::new(); $start.FileName=$Path; $start.Arguments='--version'
    $start.UseShellExecute=$false; $start.CreateNoWindow=$true
    $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true; $start.RedirectStandardInput=$true
    $process=[Diagnostics.Process]::new(); $process.StartInfo=$start; $started=$false
    try {
        $started=$process.Start(); if (-not $started) { throw 'Could not start the verified candidate.' }
        $process.StandardInput.Close(); $output=$process.StandardOutput.ReadToEndAsync(); $errorOutput=$process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) { throw 'Candidate version check timed out.' }
        if (-not $output.Wait(1000) -or -not $errorOutput.Wait(1000) -or $process.ExitCode -ne 0) { throw 'Candidate version check failed.' }
        return $output.Result.Trim()
    } finally {
        try {
            if ($started -and -not $process.HasExited) {
                try { $process.Kill() } catch { }
                if (-not $process.WaitForExit(3000)) { throw "Owned candidate PID $($process.Id) has not confirmed exit; retain the installation for inspection." }
            }
        } finally { $process.Dispose() }
    }
}
if (-not $InstallDir) { $InstallDir=Join-Path $env:LOCALAPPDATA 'Programs\SSHFilesBeta' }
$InstallDir=Resolve-BetaPath $InstallDir
$protected=@((Resolve-BetaPath (Join-Path $env:LOCALAPPDATA 'Programs\SSHFiles')),(Resolve-BetaPath (Join-Path $env:LOCALAPPDATA 'Programs\TerminalWorkspace')))
if ($WorkspaceRoot) { $protected+=Resolve-BetaPath $WorkspaceRoot }
foreach ($path in $protected) { if (Test-Overlap $InstallDir $path) { throw 'Beta must be separate from the ordinary installation and Workspace.' } }
$inspect=$InstallDir
while ($inspect) {
    if ((Test-Path -LiteralPath (Join-Path $inspect '.ssh-files-installer')) -or (Test-Path -LiteralPath (Join-Path $inspect '.workspace-installer')) -or (Test-Path -LiteralPath (Join-Path $inspect 'bin\terminal-workspace.exe'))) { throw 'Beta must not be nested in an ordinary app or Workspace.' }
    $parent=[IO.Directory]::GetParent($inspect); $inspect=if ($parent) { $parent.FullName } else { $null }
}
$marker=Join-Path $InstallDir '.ssh-files-beta-installer'; $owner='ssh-files-beta'
$bin=Join-Path $InstallDir 'bin'; $command=Join-Path $bin 'ssh-files-beta.exe'
$versionFile=Join-Path $InstallDir 'version'; $lockPath=Join-Path $InstallDir '.install-beta.lock'
foreach ($path in @($marker,$command,$versionFile,$lockPath)) { Assert-Regular $path }
$previousOwner=if ([IO.File]::Exists($marker)) { [IO.File]::ReadAllText($marker) } else { $null }
if ($null -ne $previousOwner) {
    if ($previousOwner -cne ($owner+"`n") -or -not [IO.File]::Exists($command) -or -not [IO.File]::Exists($versionFile)) { throw 'Incomplete or unknown beta ownership; inspect this directory.' }
} elseif ((Test-Path -LiteralPath $InstallDir) -and @(Get-ChildItem -LiteralPath $InstallDir -Force).Count) { throw 'Choose an empty directory or a complete owned beta installation.' }
[IO.Directory]::CreateDirectory($InstallDir) | Out-Null
$lock=$null; $stage=$null; $retain=$false
try {
    $lock=[IO.File]::Open($lockPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    $currentOwner=if ([IO.File]::Exists($marker)) { [IO.File]::ReadAllText($marker) } else { $null }
    if ($currentOwner -cne $previousOwner) { throw 'Installation ownership changed during preparation.' }
    $stage=Join-Path $InstallDir ('.install-beta-'+[Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    $candidate=Join-Path $stage 'ssh-files-beta.exe'
    $asset='ssh-files-x86_64-pc-windows-msvc.exe'
    $url="https://github.com/brant92good/ssh-files/releases/download/v$Version/$asset"
    if ($Binary) {
        if (-not $Sha256 -or -not [IO.File]::Exists($Binary)) { throw 'Local fixture binaries require an existing file and explicit Sha256.' }
        [IO.File]::Copy([IO.Path]::GetFullPath($Binary),$candidate)
    } else {
        [Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $candidate -TimeoutSec 60
        if (-not $Sha256) {
            $sidecar=Join-Path $stage 'checksum'
            Invoke-WebRequest -UseBasicParsing -Uri ($url+'.sha256') -OutFile $sidecar -TimeoutSec 60
            $line=[IO.File]::ReadAllText($sidecar).Trim()
            if ($line -cnotmatch ('\A([a-f0-9]{64})  '+[regex]::Escape($asset)+'\z')) { throw 'Invalid exact release checksum sidecar.' }
            $Sha256=$Matches[1]
        }
    }
    if ($Sha256 -notmatch '\A[a-fA-F0-9]{64}\z' -or (Get-BetaHash $candidate) -cne $Sha256.ToLowerInvariant()) { throw 'Download checksum mismatch; existing beta files preserved.' }
    if ((Read-Version $candidate) -cne ('ssh-files '+$Version)) { throw 'The candidate has the wrong version.' }
    [IO.File]::WriteAllText((Join-Path $stage 'owner'),$owner+"`n",[Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText((Join-Path $stage 'version'),$Version+"`n",[Text.UTF8Encoding]::new($false))
    [void](Resolve-BetaPath $bin); [IO.Directory]::CreateDirectory($bin) | Out-Null
    $changes=[Collections.Generic.List[object]]::new(); $transaction=[IO.Path]::GetFileName($stage)
    try {
        foreach ($item in @(@($candidate,$command),@((Join-Path $stage 'version'),$versionFile),@((Join-Path $stage 'owner'),$marker))) {
            $destination=$item[1]; Assert-Regular $destination; $backup=$null
            if ([IO.File]::Exists($destination)) { $backup=$destination+'.previous-'+$transaction; [IO.File]::Move($destination,$backup) }
            $changes.Add(@{path=$destination;backup=$backup})
            # All files are staged on this volume. Rename new inodes rather than
            # truncating metadata that could be hardlinked to unrelated files.
            [IO.File]::Move($item[0],$destination)
        }
        if ((Get-BetaHash $command) -cne $Sha256.ToLowerInvariant()) { throw 'Published beta binary readback failed.' }
    } catch {
        $failure=$_; $rollback=[Collections.Generic.List[string]]::new()
        for ($i=$changes.Count-1; $i -ge 0; $i--) {
            try { $change=$changes[$i]; Assert-Regular $change.path; if ([IO.File]::Exists($change.path)) { [IO.File]::Delete($change.path) }; if ($change.backup) { [IO.File]::Move($change.backup,$change.path) } }
            catch { $rollback.Add($_.Exception.Message) }
        }
        if ($rollback.Count) { $retain=$true; throw "Install failed and rollback is incomplete. Retain $stage and previous files for inspection. $failure $($rollback -join '; ')" }
        throw $failure
    }
    Write-Output "Installed SSH Files BETA $Version. PATH, normal Files and Workspace settings were unchanged."
    Write-Output "Command: $command"
    Write-Output 'Use that exact path with --host YOUR_SSH_ALIAS; installation opens no connection or window.'
} finally {
    try {
        if ($stage -and -not $retain) {
            $full=[IO.Path]::GetFullPath($stage)
            if ([IO.Path]::GetDirectoryName($full) -cne $InstallDir -or [IO.Path]::GetFileName($full) -notmatch '\A\.install-beta-[a-f0-9]{32}\z') { throw 'Unsafe stage cleanup refused.' }
            Remove-Item -LiteralPath $full -Recurse -Force
        }
    } finally { if ($lock) { $lock.Dispose(); [IO.File]::Delete($lockPath) } }
}
