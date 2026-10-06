<#
.SYNOPSIS
    Install ITSaNAS on Windows from the latest release, without compiling.

.DESCRIPTION
    In a PowerShell window:

        irm https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.ps1 | iex

    or, from a checkout:

        powershell -ExecutionPolicy Bypass -File install\get.ps1

    For a tester who cannot, or should not have to, build Rust; install\windows.ps1
    builds from source and stays the way to install what is on `main` before
    it is released.

    It finds the latest *published* release through the GitHub API, downloads
    its manifest.txt and itsanas-x86_64-pc-windows-msvc.exe, checks the size and
    SHA-256 against the manifest, puts the binary where windows.ps1 puts it
    (%LOCALAPPDATA%\Programs\itsanas\bin), adds that to your PATH the way
    windows.ps1 does, and runs `itsanas setup`.

    What it trusts, honestly: the manifest is signed by Nicolas's release key,
    but this script cannot check an Ed25519 signature with what Windows ships.
    So for this first download the trust root is HTTPS to github.com: the size
    and hash check catches a truncated or corrupted download, not a forged
    release. From then on, the installed binary verifies every update's
    signature against the key compiled into it (docs/RELEASING.md).

.PARAMETER Prefix
    Where to install. Default: %LOCALAPPDATA%\Programs\itsanas

.PARAMETER NoSetup
    Install only; do not run `itsanas setup` afterwards.

.PARAMETER NoPath
    Do not add the install directory to your user PATH. Also what
    scripts/check-installers.sh passes: a check of this script must never be
    able to edit the PATH of whoever runs it, even when the script is broken.

.PARAMETER Clean
    Remove what a previous install put here, then stop. Delegates to
    clean.ps1 (a dry run unless -Yes is given).
#>
param(
    [string] $Prefix = "$env:LOCALAPPDATA\Programs\itsanas",
    [switch] $NoSetup,
    [switch] $NoPath,
    [switch] $Clean,
    [switch] $Yes,
    [string] $Repo = $(if ($env:ITSANAS_REPO) { $env:ITSANAS_REPO } else { 'SigSegGit/itsanas' })
)

# Delegation, not a second uninstaller: install\clean.ps1 is the only one.
if ($Clean) {
    $script = if ($PSScriptRoot) { Join-Path $PSScriptRoot 'clean.ps1' } else { '' }
    if (-not $script -or -not (Test-Path -LiteralPath $script)) {
        Write-Host 'error: -Clean needs the checkout: clone the repository and run install\clean.ps1'
        if ($PSCommandPath) { exit 1 } else { return }
    }
    $LASTEXITCODE = 0
    & $script -Yes:$Yes
    exit $LASTEXITCODE
}

# Read at the top level: inside a function, $PSBoundParameters is the function's.
$prefixGiven = $PSBoundParameters.ContainsKey('Prefix')

# Every failure is one line saying what to do, then a stop. Not `exit`: run as
# `irm ... | iex` this is the person's own PowerShell window, and `exit` would
# close it with the message in it. A marked exception unwinds to the bottom of
# the file instead, which exits only when this was run as a file.
$stopMark = 'itsanas-get-stop'
function Stop-Plain {
    param([string] $Message)
    Write-Host ''
    Write-Host "error: $Message" -ForegroundColor Red
    throw $stopMark
}

# SHA-256 through .NET, as windows.ps1 does: Get-FileHash is not always
# loadable under -NoProfile.
function Get-Sha256 {
    param([string] $Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($Path)
        try {
            return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
        } finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

function Get-Release {
    param([string] $Work)
    # Windows PowerShell 5.1 may still default to TLS 1.0, which GitHub refuses.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    Write-Host 'Looking for the latest ITSaNAS release...'
    try {
        $release = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/$Repo/releases/latest"
    } catch {
        Stop-Plain "no published release found for $Repo (or GitHub is unreachable): try again later, or build from source with install\windows.ps1"
    }
    $tag = [string] $release.tag_name
    # The tag goes into URLs below; anything but a plain version tag is refused.
    if ($tag -notmatch '^v[0-9][A-Za-z0-9._-]*$') {
        Stop-Plain 'GitHub answered without a release version: try again in a minute'
    }
    $base = "https://github.com/$Repo/releases/download/$tag"
    $manifest = Join-Path $Work 'manifest.txt'
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/manifest.txt" -OutFile $manifest
    } catch {
        Stop-Plain "release $tag has no manifest.txt yet: it may still be being published, try again in a few minutes"
    }
    return @{ Tag = $tag; Base = $base; Manifest = $manifest }
}

function Get-VerifiedBinary {
    param([hashtable] $Release, [string] $Target, [string] $Work)
    # file <target> <name> <size> <blake3> <sha256>
    $line = Get-Content -LiteralPath $Release.Manifest |
        Where-Object { ($_ -split ' ')[0] -eq 'file' -and ($_ -split ' ')[1] -eq $Target } |
        Select-Object -First 1
    if (-not $line) {
        Stop-Plain "release $($Release.Tag) has no binary for ${Target}: build from source, or wait for the next release"
    }
    $fields = $line -split ' '
    $name = $fields[2]
    $size = [int64] $fields[3]
    $sha = $fields[5]
    if ($name -ne "itsanas-$Target.exe") { Stop-Plain "the manifest of $($Release.Tag) is damaged: try again later" }

    Write-Host "Downloading ITSaNAS $($Release.Tag.TrimStart('v')) for $Target..."
    $file = Join-Path $Work $name
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$($Release.Base)/$name" -OutFile $file
    } catch {
        Stop-Plain "the download of $name stopped: check the network and run this again"
    }
    $got = (Get-Item -LiteralPath $file).Length
    if ($got -ne $size) { Stop-Plain "the download is incomplete ($got of $size bytes): run this again" }
    if ((Get-Sha256 $file) -ne $sha) {
        Stop-Plain 'the download does not match the release (SHA-256 differs): run this again; if it persists, tell Nicolas'
    }
    Write-Host '  ok   size and SHA-256 match the release manifest' -ForegroundColor Green
    return $file
}

function Install-Binary {
    param([string] $File, [string] $BinDir)
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $dest = Join-Path $BinDir 'itsanas.exe'
    $new = Join-Path $BinDir 'itsanas.exe.new'
    # Leftovers from earlier runs; one still in use simply stays.
    Get-ChildItem -LiteralPath $BinDir -Filter 'itsanas.exe.old-*' -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    try {
        Copy-Item -LiteralPath $File -Destination $new -Force
        # Never written over: a running daemon holds itsanas.exe open, and
        # Windows lets a running executable be renamed but not rewritten. The
        # old one is moved aside and the new one renamed into place; the daemon
        # picks it up at its next start (the #240 rule, as in windows.ps1).
        if (Test-Path -LiteralPath $dest) {
            Rename-Item -LiteralPath $dest -NewName ('itsanas.exe.old-' + (Get-Date -Format 'yyyyMMddHHmmss'))
        }
        Rename-Item -LiteralPath $new -NewName 'itsanas.exe'
    } catch {
        Stop-Plain "could not install into ${BinDir}: close any ITSaNAS window and run this again"
    }
    Write-Host "  ok   $dest" -ForegroundColor Green
    return $dest
}

function Add-ToUserPath {
    param([string] $BinDir)
    # This user only, as windows.ps1 does: a machine-wide change needs
    # administrator rights a storage tool has no business asking for.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $BinDir) {
        $joined = if ($userPath) { "$userPath;$BinDir" } else { $BinDir }
        [Environment]::SetEnvironmentVariable('Path', $joined, 'User')
        Write-Host "  ok   added $BinDir to your PATH (new windows will see it)" -ForegroundColor Green
    }
    if (($env:Path -split ';') -notcontains $BinDir) { $env:Path = "$env:Path;$BinDir" }
}

function Install-ITSaNAS {
    # Set here, not at the top of the file: under `irm | iex` the top of the
    # file is the person's own session, and these would outlive the install.
    # Functions called from here see them. The progress bar is off because
    # Windows PowerShell 5.1 downloads several times slower while drawing it.
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    if (-not $env:LOCALAPPDATA -and -not $prefixGiven) {
        Stop-Plain 'LOCALAPPDATA is not set, so there is no default place to install: pass -Prefix C:\Tools\itsanas'
    }
    # x64 runs natively, and under emulation on Windows on ARM; nothing else
    # is built.
    if ($env:PROCESSOR_ARCHITECTURE -notin @('AMD64', 'ARM64')) {
        Stop-Plain "no release is built for a $env:PROCESSOR_ARCHITECTURE Windows: ITSaNAS needs 64-bit Windows"
    }
    $target = 'x86_64-pc-windows-msvc'
    $work = Join-Path ([System.IO.Path]::GetTempPath()) ('itsanas-get-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $release = Get-Release -Work $work
        $file = Get-VerifiedBinary -Release $release -Target $target -Work $work
        $binDir = Join-Path $Prefix 'bin'
        $exe = Install-Binary -File $file -BinDir $binDir
        if ($NoPath) {
            Write-Host "  To type itsanas anywhere, add $binDir to your PATH"
        } else {
            Add-ToUserPath -BinDir $binDir
        }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($NoSetup) {
        Write-Host "Installed. Next: itsanas setup"
        return
    }
    Write-Host ''
    Write-Host 'Installed. Starting the setup...'
    & $exe setup
}

try {
    Install-ITSaNAS
} catch {
    if ($_.Exception.Message -ne $stopMark) {
        Write-Host ''
        Write-Host "error: $($_.Exception.Message)" -ForegroundColor Red
    }
    if ($PSCommandPath) { exit 1 }
}
