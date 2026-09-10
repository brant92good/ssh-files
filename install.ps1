param(
    [string]$InstallDir = $env:SSH_FILES_INSTALL_DIR,
    [string]$Version = $(if ($env:SSH_FILES_VERSION) { $env:SSH_FILES_VERSION } else { '0.2.0' }),
    [string]$Binary = $env:SSH_FILES_BINARY,
    [string]$Sha256 = $env:SSH_FILES_SHA256,
    [switch]$NoPath = ($env:SSH_FILES_NO_PATH -eq '1')
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+([-.][A-Za-z0-9.-]+)?$') { throw 'Invalid release version.' }
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\SSHFiles' }
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$marker = Join-Path $InstallDir '.ssh-files-installer'
if ((Test-Path -LiteralPath $InstallDir) -and -not (Test-Path -LiteralPath $marker) -and
    @(Get-ChildItem -LiteralPath $InstallDir -Force).Count) {
    throw "Choose an empty install directory: $InstallDir already contains other files."
}
if ((Test-Path -LiteralPath $marker) -and [IO.File]::ReadAllText($marker).Trim() -ne 'ssh-files') {
    throw 'This install directory belongs to another application.'
}
if (-not [Environment]::Is64BitOperatingSystem) { throw 'This release requires 64-bit Windows.' }
$asset = 'ssh-files-x86_64-pc-windows-msvc.exe'
$base = "https://github.com/brant92good/ssh-files/releases/download/v$Version"
if (-not $Binary) { $Binary = "$base/$asset" }
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
$stage = Join-Path $InstallDir ('.install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
$candidate = Join-Path $stage 'ssh-files.exe'
$bin = Join-Path $InstallDir 'bin'
$command = Join-Path $bin 'ssh-files.exe'
$backup = Join-Path $bin ('ssh-files.previous-' + [Guid]::NewGuid().ToString('N') + '.exe')
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    if ($Binary -match '^https://') {
        Invoke-WebRequest -UseBasicParsing -Uri $Binary -OutFile $candidate
        if (-not $Sha256) {
            # Release sidecars may have an application/octet-stream content type.
            # Read bytes from a file rather than assuming IWR.Content is a string.
            $checksumFile = Join-Path $stage 'checksum.txt'
            Invoke-WebRequest -UseBasicParsing -Uri ($Binary + '.sha256') -OutFile $checksumFile
            $Sha256 = ([IO.File]::ReadAllText($checksumFile).Trim() -split '\s+')[0]
        }
    } elseif (Test-Path -LiteralPath $Binary -PathType Leaf) {
        Copy-Item -LiteralPath $Binary -Destination $candidate
        if (-not $Sha256) { throw 'Local test binaries require -Sha256.' }
    } else { throw 'Binary must be an HTTPS release URL or an existing local file.' }
    if ($Sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'The release checksum is invalid.' }
    # Use the built-in .NET implementation. An inherited PowerShell 7 module path
    # can prevent Windows PowerShell 5 from loading Get-FileHash's script module.
    $hasher = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::OpenRead($candidate)
    try { $actualHash = [BitConverter]::ToString($hasher.ComputeHash($stream)).Replace('-', '') }
    finally { $stream.Dispose(); $hasher.Dispose() }
    if ($actualHash -ine $Sha256) { throw 'Download checksum mismatch. The existing installation was preserved.' }
    $reported = & $candidate --version
    if ($LASTEXITCODE -ne 0 -or $reported -ne "ssh-files $Version") { throw 'The downloaded app could not run or has the wrong version.' }
    New-Item -ItemType Directory -Path $bin -Force | Out-Null
    if (Test-Path -LiteralPath $command) {
        try { Move-Item -LiteralPath $command -Destination $backup }
        catch { throw 'Close SSH Files and run the install command again. The current app was preserved.' }
    }
    try { Move-Item -LiteralPath $candidate -Destination $command }
    catch {
        if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $command }
        throw
    }
    [IO.File]::WriteAllText($marker, 'ssh-files')
    [IO.File]::WriteAllText((Join-Path $InstallDir 'version'), $Version)
    if (-not $NoPath) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $entries = @($userPath -split ';' | Where-Object { $_ })
        if (-not ($entries | Where-Object { $_.TrimEnd('\') -ieq $bin.TrimEnd('\') })) {
            [Environment]::SetEnvironmentVariable('Path', (($entries + $bin) -join ';'), 'User')
        }
        if (-not ($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $bin.TrimEnd('\') })) { $env:Path += ';' + $bin }
    }
    Write-Output "Installed SSH Files $Version. Run ssh-files --host YOUR_SSH_ALIAS."
    Write-Output "Command: $command"
    if (-not $NoPath) { Write-Output 'Open a new terminal if the command is not found yet.' }
    Write-Output 'Open an existing SSH alias: ssh-files --host devbox'
} finally {
    # Only the random staging directory created by this invocation may be removed.
    $resolvedStage = [IO.Path]::GetFullPath($stage)
    if ([IO.Path]::GetDirectoryName($resolvedStage) -eq $InstallDir -and
        [IO.Path]::GetFileName($resolvedStage) -match '^\.install-[a-f0-9]{32}$' -and
        (Test-Path -LiteralPath $resolvedStage)) {
        Remove-Item -LiteralPath $resolvedStage -Recurse -Force
    }
}
