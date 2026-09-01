<#
.SYNOPSIS
    ITSaNAS installer for Windows.

.DESCRIPTION
    A fresh Windows box has no Rust, no C toolchain, and — the part that catches
    everyone — no linker. `cargo build` on such a machine fails with
    "linker `link.exe` not found", which sends people to install Rust again
    rather than the Visual Studio Build Tools that are actually missing.

    So this checks first and says what to do, rather than starting a forty-minute
    build that fails at the last step.

    Deliberate choices:

    - **No `Set-ExecutionPolicy`.** An installer that loosens a security setting
      to run itself is teaching a bad habit. If the policy blocks this, the
      message says how to run it once without changing anything permanently.
    - **Nothing is parsed that does not have to be.** The Rust version is split
      on dots and compared as integers, not matched with a regular expression
      that a future `rustc --version` format quietly breaks.
    - **Idempotent.** Run it twice and the second run changes nothing.

.PARAMETER Prefix
    Where to put the binaries. Default: %LOCALAPPDATA%\Programs\itsanas

.PARAMETER Source
    Build from this checkout instead of looking for one.

.PARAMETER NoService
    Do not offer to register the scheduled task.

.PARAMETER NoBuild
    Check the machine and stop, changing nothing.

.PARAMETER NoSmoke
    Skip storing a test file once it is installed.

.PARAMETER Yes
    Do not ask before installing anything.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File install\windows.ps1

.EXAMPLE
    .\install\windows.ps1 -NoBuild
#>

[CmdletBinding()]
param(
    [string] $Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\itsanas'),
    [string] $Source = '',
    [switch] $NoService,
    [switch] $NoBuild,
    [switch] $NoSmoke,
    [switch] $Yes
)

$ErrorActionPreference = 'Stop'
$MinRustMajor = 1
$MinRustMinor = 88

# ---------------------------------------------------------------- appearance

function Write-Step { param([string] $Text) Write-Host ''; Write-Host "==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param([string] $Text) Write-Host "  ok   $Text" -ForegroundColor Green }
function Write-Warn { param([string] $Text) Write-Host "  warn $Text" -ForegroundColor Yellow }
function Write-Info { param([string] $Text) Write-Host "       $Text" }

# Every failure says what was attempted and what to do instead. A one-line
# error on somebody else's machine is a support conversation.
function Stop-WithAdvice {
    param([string] $Problem, [string[]] $Advice = @())
    Write-Host ''
    Write-Host "error $Problem" -ForegroundColor Red
    foreach ($line in $Advice) { Write-Host "       $line" }
    Write-Host ''
    exit 1
}

function Test-Command {
    param([string] $Name)
    $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

Write-Host 'ITSaNAS installer 1.0' -ForegroundColor DarkGray

# ------------------------------------------------------------- what is this

Write-Step 'Looking at this machine'

if (-not [Environment]::Is64BitOperatingSystem) {
    Stop-WithAdvice '32-bit Windows is not supported' @(
        'ITSaNAS needs a 64-bit target: it maps large files and keeps 64-bit',
        'counters that a 32-bit address space cannot hold.'
    )
}

$arch = $env:PROCESSOR_ARCHITECTURE
Write-Ok "Windows $([Environment]::OSVersion.Version) on $arch"

if ($PSVersionTable.PSVersion.Major -lt 5) {
    Stop-WithAdvice "PowerShell $($PSVersionTable.PSVersion) is too old" @(
        'This needs PowerShell 5.1 or newer, which ships with Windows 10 and 11.'
    )
}

# --------------------------------------------------------------- resources

Write-Step 'Checking there is enough to build with'

try {
    $memGb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
    if ($memGb -lt 2) {
        Write-Warn "$memGb GB of memory; the build wants about 1.5 GB free"
    } else {
        Write-Ok "$memGb GB of memory"
    }
} catch {
    Write-Warn 'could not read the memory size; skipping that check'
}

try {
    $driveLetter = (Split-Path -Qualifier $Prefix).TrimEnd(':')
    $free = [math]::Round((Get-PSDrive -Name $driveLetter).Free / 1GB, 1)
    if ($free -lt 4) {
        Write-Warn "$free GB free on ${driveLetter}: and the build wants about 4 GB"
    } else {
        Write-Ok "$free GB free on ${driveLetter}:"
    }
} catch {
    Write-Warn 'could not measure free space; skipping that check'
}

# --------------------------------------------------------------- toolchain

Write-Step 'Checking the build tools'

# The linker. This is the one that catches everybody: Rust installs cleanly,
# `cargo build` runs for a while, and then fails with "linker `link.exe` not
# found" — which reads like a Rust problem and is not.
$haveLinker = $false
if (Test-Command 'link.exe') {
    $haveLinker = $true
} else {
    # A Visual Studio or Build Tools installation that is present but not on
    # this shell's PATH is the normal state: the linker lives inside a
    # developer command prompt. Look for it rather than concluding it is absent.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $found = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>$null
        if ($found) {
            $haveLinker = $true
            Write-Ok "MSVC build tools at $found"
        }
    }
}

if ($haveLinker) {
    if (-not (Test-Command 'link.exe')) {
        Write-Info 'The linker is installed but not on this shell''s PATH.'
        Write-Info 'cargo finds it anyway; if it does not, use a "Developer PowerShell".'
    } else {
        Write-Ok 'linker (link.exe)'
    }
} else {
    Stop-WithAdvice 'the MSVC linker is missing' @(
        'Rust on Windows links with Microsoft''s linker, which does not ship with',
        'Windows and is not part of Rust. Without it the build runs for a long',
        'time and then fails with "linker `link.exe` not found", which looks like',
        'a Rust problem and is not.',
        '',
        'Install the Build Tools (about 2 GB, no Visual Studio needed):',
        '',
        '  winget install --id Microsoft.VisualStudio.2022.BuildTools ^',
        '    --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools ^',
        '    --includeRecommended"',
        '',
        'or download them from:',
        '  https://visualstudio.microsoft.com/visual-cpp-build-tools/',
        '',
        'Then run this installer again.'
    )
}

# Rust. Compared as integers after an explicit split: `rustc --version` prints
# things like "rustc 1.88.0-nightly (abc 2026-01-01)" and every regex written
# for that string eventually meets a form it did not expect.
function Test-RustIsNewEnough {
    if (-not (Test-Command 'rustc')) { return $false }
    $line = (& rustc --version 2>$null)
    if (-not $line) { return $false }
    $version = ($line -split '\s+')[1]
    if (-not $version) { return $false }
    $parts = ($version -split '[.\-+]')
    if ($parts.Count -lt 2) { return $false }
    $major = 0; $minor = 0
    if (-not [int]::TryParse($parts[0], [ref] $major)) { return $false }
    if (-not [int]::TryParse($parts[1], [ref] $minor)) { return $false }
    if ($major -gt $MinRustMajor) { return $true }
    return ($major -eq $MinRustMajor -and $minor -ge $MinRustMinor)
}

if (Test-RustIsNewEnough) {
    Write-Ok "rust $(((& rustc --version) -split '\s+')[1])"
} else {
    if (Test-Command 'rustc') {
        Write-Warn "rust $(((& rustc --version) -split '\s+')[1]) is older than $MinRustMajor.$MinRustMinor"
    } else {
        Write-Warn 'rust is not installed'
    }

    if (Test-Command 'rustup') {
        Write-Info 'updating the toolchain with rustup'
        & rustup update stable
        if ($LASTEXITCODE -ne 0) { Stop-WithAdvice 'rustup update failed' }
    } else {
        if (-not $Yes) {
            $reply = Read-Host '  Install the Rust toolchain with rustup? [y/N]'
            if ($reply -notmatch '^(y|yes)$') {
                Stop-WithAdvice "stopping: Rust $MinRustMajor.$MinRustMinor or newer is needed"
            }
        }
        Write-Info 'downloading rustup'
        $installer = Join-Path $env:TEMP 'rustup-init.exe'
        try {
            Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $installer -UseBasicParsing
        } catch {
            Stop-WithAdvice 'could not download rustup' @(
                "The error was: $($_.Exception.Message)",
                'Check the network, or install Rust from https://rustup.rs by hand',
                'and run this again.'
            )
        }
        & $installer -y --no-modify-path --profile minimal
        if ($LASTEXITCODE -ne 0) { Stop-WithAdvice 'rustup failed to install the toolchain' }
        Remove-Item $installer -ErrorAction SilentlyContinue
        $env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH
    }

    if (-not (Test-RustIsNewEnough)) {
        Stop-WithAdvice "Rust is still older than $MinRustMajor.$MinRustMinor" @(
            'If rustup just installed it, this shell may still be finding an older',
            'rustc first. Open a new PowerShell and run this again.'
        )
    }
    Write-Ok "rust $(((& rustc --version) -split '\s+')[1])"
}

if ($NoBuild) {
    Write-Step 'Stopping here (-NoBuild)'
    Write-Ok 'this machine can build ITSaNAS'
    exit 0
}

# ----------------------------------------------------------------- sources

Write-Step 'Getting the source'

if ($Source) {
    if (-not (Test-Path (Join-Path $Source 'Cargo.toml'))) {
        Stop-WithAdvice "$Source is not an ITSaNAS checkout" @('Expected to find Cargo.toml there.')
    }
    $buildDir = (Resolve-Path $Source).Path
} else {
    $here = Split-Path -Parent $PSScriptRoot
    if (Test-Path (Join-Path $here 'Cargo.toml')) {
        $buildDir = $here
    } else {
        Stop-WithAdvice 'nothing to build' @(
            'Run this from inside a checkout, or pass -Source <dir>.'
        )
    }
}
Write-Ok "building from $buildDir"

# ------------------------------------------------------------------- build

Write-Step 'Building (10-20 minutes the first time)'

Push-Location $buildDir
try {
    & cargo build --release --locked
    $buildFailed = ($LASTEXITCODE -ne 0)
} finally {
    Pop-Location
}

if ($buildFailed) {
    Stop-WithAdvice 'the build failed' @(
        'If the last line mentioned "link.exe", the MSVC build tools are missing',
        'or this shell cannot see them: open a "Developer PowerShell for VS" and',
        'run this again.',
        '',
        'If it mentioned a checksum or a download, the network dropped: run this',
        'again, cargo resumes where it stopped.',
        '',
        'If Windows Defender is scanning the target directory the build can take',
        'many times longer and occasionally fail on a locked file. Excluding',
        (Join-Path $buildDir 'target') + ' from real-time scanning fixes both.'
    )
}
Write-Ok 'built'

# ----------------------------------------------------------------- install

Write-Step 'Installing'

$binDir = Join-Path $Prefix 'bin'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

foreach ($prog in @('itsanas.exe', 'itsanas-coordinator.exe')) {
    $src = Join-Path $buildDir "target\release\$prog"
    if (-not (Test-Path $src)) {
        Stop-WithAdvice "$prog was not produced by the build" @(
            "Expected $src.",
            'This usually means the build stopped early; scroll up.'
        )
    }
    # A running daemon holds its own binary open, so a plain copy fails with a
    # sharing violation. Say which process, rather than "access denied".
    try {
        Copy-Item $src (Join-Path $binDir $prog) -Force
    } catch {
        Stop-WithAdvice "could not replace $prog" @(
            "$($_.Exception.Message)",
            '',
            'If ITSaNAS is running, stop it first:',
            '  Get-Process itsanas -ErrorAction SilentlyContinue | Stop-Process',
            '  schtasks /End /TN ITSaNAS'
        )
    }
    Write-Ok (Join-Path $binDir $prog)
}

# PATH, for this user only. A machine-wide change needs administrator rights
# that a storage tool has no business asking for.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -split ';' -notcontains $binDir) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$binDir", 'User')
    Write-Ok "added $binDir to your PATH"
    Write-Info 'Open a new terminal for it to take effect.'
} else {
    Write-Ok "$binDir is already on your PATH"
}

# ----------------------------------------------------------------- service

if (-not $NoService) {
    Write-Step 'Setting up the background task'

    # A scheduled task rather than a Windows service. A service runs as
    # LocalSystem or needs a stored password; this daemon holds the user's keys
    # and writes into their profile, so it belongs to the user session. The
    # trade is that it starts at logon rather than at boot, which is stated
    # rather than hidden.
    $taskName = 'ITSaNAS'
    $existing = schtasks /Query /TN $taskName 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Ok "the '$taskName' task already exists; left alone"
        Write-Info "Delete it with:  schtasks /Delete /TN $taskName /F"
    } else {
        Write-Info 'Not registering it automatically: the daemon needs your'
        Write-Info 'passphrase, and a task that stores it is a decision you should'
        Write-Info 'make deliberately rather than inherit from an installer.'
        Write-Info ''
        Write-Info 'To run it at logon once you have decided:'
        Write-Info ''
        Write-Info "  schtasks /Create /TN $taskName /SC ONLOGON ^"
        Write-Info "    /TR `"$(Join-Path $binDir 'itsanas.exe') daemon`" /RL LIMITED"
        Write-Info ''
        Write-Info 'and set the passphrase for your account with:'
        Write-Info ''
        Write-Info '  [Environment]::SetEnvironmentVariable('
        Write-Info "    'ITSANAS_PASSPHRASE', '<your passphrase>', 'User')"
        Write-Info ''
        Write-Info 'That stores it in your registry hive, readable by anything running'
        Write-Info 'as you. If that is not acceptable, run `itsanas daemon` by hand'
        Write-Info 'instead and type it.'
    }
}

# ------------------------------------------------------------------- check

Write-Step 'Checking what was installed'

$version = & (Join-Path $binDir 'itsanas.exe') --version 2>$null
if (-not $version) {
    Stop-WithAdvice 'the installed binary does not run' @(
        "Tried: $(Join-Path $binDir 'itsanas.exe') --version"
    )
}
Write-Ok $version

# ------------------------------------------------------------------ smoke

# `--version` proves Windows can execute the file and nothing about whether the
# data path works. The Unix installers run scripts/smoke.sh for this; there is
# no sh here, so the same steps are written out. Same claim, same evidence.
if (-not $NoSmoke) {
    Write-Step 'Storing a file and reading it back, on this machine'
    $work = Join-Path ([IO.Path]::GetTempPath()) ("itsanas-smoke-" + [IO.Path]::GetRandomFileName())
    try {
        New-Item -ItemType Directory -Force -Path $work | Out-Null
        $exe = Join-Path $binDir 'itsanas.exe'
        $env:ITSANAS_PASSPHRASE = 'itsanas-smoke-passphrase-9931'

        $initOutput = & $exe --home (Join-Path $work 'home') init --username smoke 2>&1
        # The recovery phrase is printed in two numbered columns. A count other
        # than 24 would mean the key schedule produced something different here,
        # so an account made on this machine would not open on another.
        $words = ([regex]::Matches(($initOutput -join "`n"), '\b\d{1,2}\.\s+([a-z]+)')).Count
        if ($words -ne 24) {
            Stop-WithAdvice 'the recovery phrase is not 24 words' @(
                "Got $words. The output was:", ($initOutput -join "`n")
            )
        }
        Write-Ok "an account, and a $words-word recovery phrase"

        # Larger than one chunk, so this exercises the chunker and the manifest
        # rather than a single sealed blob.
        $payload = Join-Path $work 'payload.bin'
        # `RandomNumberGenerator::Fill` is .NET Core and this script supports
        # PowerShell 5.1, which is .NET Framework. System.Random is not a
        # cryptographic source and does not need to be: this is a payload to
        # hash, not a key.
        $bytes = New-Object byte[] 350000
        (New-Object Random).NextBytes($bytes)
        [IO.File]::WriteAllBytes($payload, $bytes)

        & $exe --home (Join-Path $work 'home') put 'docs/smoke.bin' $payload | Out-Null
        & $exe --home (Join-Path $work 'home') get 'docs/smoke.bin' (Join-Path $work 'back.bin') | Out-Null

        $before = (Get-FileHash -Algorithm SHA256 $payload).Hash
        $after = (Get-FileHash -Algorithm SHA256 (Join-Path $work 'back.bin')).Hash
        if ($before -ne $after) {
            Stop-WithAdvice 'the bytes changed between storing and reading' @(
                "wrote $before", "read  $after",
                '',
                'It installed and it does not work, which is the interesting',
                'kind of failure. Please report it.'
            )
        }
        Write-Ok 'a file went in and came back byte for byte'
    } finally {
        Remove-Item env:ITSANAS_PASSPHRASE -ErrorAction SilentlyContinue
        Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# --------------------------------------------------------------- firewall

# Windows blocks inbound connections to a program that has not been allowed, and
# it does so silently: the node serves happily on 0.0.0.0, `netstat` shows it
# listening, and every peer that tries to reach it times out. Tested between this
# laptop and an aarch64 VM on the same LAN -- the laptop could pull, and nothing
# could ever pull from it.
#
# That matters more here than it looks. A node nobody can dial can push its own
# work and never host anybody else's, which is the half of the bargain that pays
# for the other half.
#
# This does not add the rule. Creating a firewall exception needs administrator
# rights, and an installer that asks for them in order to open a port is an
# installer that has to be trusted about which port. It prints the command
# instead, the same way the linker check prints the winget line.
Write-Step 'Can peers reach this machine?'

$listenPort = 9797
$configPath = Join-Path $env:USERPROFILE '.itsanas\config'
if (Test-Path $configPath) {
    $configured = Select-String -Path $configPath -Pattern '^listen\s*=\s*\S+:(\d+)' -ErrorAction SilentlyContinue
    if ($configured) { $listenPort = [int]$configured.Matches[0].Groups[1].Value }
}

$allowed = $false
try {
    $allowed = [bool](Get-NetFirewallRule -Direction Inbound -Enabled True -Action Allow -ErrorAction Stop |
        Get-NetFirewallPortFilter -ErrorAction Stop |
        Where-Object { $_.Protocol -eq 'TCP' -and $_.LocalPort -eq $listenPort })
} catch {
    Write-Warn 'could not read the firewall rules; skipping this check'
    $allowed = $true
}

if ($allowed) {
    Write-Ok "inbound TCP $listenPort is allowed"
} else {
    Write-Warn "nothing lets peers connect to this machine on TCP $listenPort"
    Write-Info 'Windows will accept nothing on that port until a rule exists, and'
    Write-Info 'the failure is silent: this node will listen, and every peer that'
    Write-Info 'tries to reach it will time out. It can still push its own work.'
    Write-Info ''
    Write-Info 'In an administrator PowerShell, for the local network only:'
    Write-Info ''
    Write-Info '  New-NetFirewallRule -DisplayName "ITSaNAS peer" -Direction Inbound ^'
    Write-Info "    -Action Allow -Protocol TCP -LocalPort $listenPort -Profile Private"
    Write-Info ''
    Write-Info 'Leave it out if this machine only ever syncs outwards.'
}

# ------------------------------------------------------------------- next

Write-Host ''
Write-Host 'Installed.' -ForegroundColor Green
Write-Host @"

Next, in a new terminal:

  itsanas init --username <your-name>     create an account, print the 24 words
  itsanas pledge 100G                     offer space to other members
  itsanas folder `$HOME\Sync               the directory to keep in step

Then point it at a coordinator, or add a peer directly:

  itsanas coordinator <host:port> --device <its-id>
  itsanas register
  itsanas peer add <host:port>

And run it:

  itsanas daemon

"@
