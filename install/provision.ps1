# Take a Windows machine with nothing on it to a running ITSaNAS node.
#
#   $env:ITSANAS_PASSPHRASE = '...'
#   powershell -ExecutionPolicy Bypass -File install\provision.ps1 -Username nicolas -Pledge 100G
#
# What this is, and why it is not `windows.ps1`
# ---------------------------------------------
#
# `install/windows.ps1` compiles and installs. That is all it does: it touches
# no keys, creates no account, and writes no secret. Getting from "installed" to
# "a node that is actually a member of something" was six more commands typed by
# hand, in an order nobody had written down, half of them needing values from
# another machine. This is that order, written down.
#
# It is the Windows half of `install/provision.sh`, and it carries the same
# lessons, each of which cost a broken run on real hardware:
#
#   * **The passphrase comes from the environment, never an argument.**
#     Arguments are visible to every process on the machine and land in shell
#     history.
#
#   * **The secret file is locked down before the secret goes into it**, not
#     after. The other order leaves a window in which it is readable by
#     everyone, and the window is exactly as long as the machine is slow.
#
#   * **Idempotence is decided by looking for the keystore, not by asking the
#     program.** `itsanas status` exits non-zero when there is no node *and*
#     when there is one that a running daemon holds open. Those are the two
#     cases the check exists to tell apart, and on Linux the first version got
#     it wrong: on a machine where provisioning had already succeeded -- which
#     is a machine whose daemon is running -- it decided there was no node.
#
#   * **The store has one writer.** Every step below opens it, so a running
#     daemon is stopped first and started again at the end.
#
#   * **The summary reports what happened.** Not what was meant to happen: the
#     Linux version printed "watch it with journalctl" on a machine where the
#     service had never been created.

[CmdletBinding()]
param(
    # The account this machine belongs to. Required unless -PhraseFile restores
    # an existing one.
    [string] $Username = '',

    # Restore an existing account from its 24 words instead of creating one.
    [string] $PhraseFile = '',

    # Space offered to other members, e.g. 100G.
    [string] $Pledge = '',

    # The directory kept in step with the account.
    [string] $Folder = '',

    # Where members find each other, and the device id you pin it to.
    [string] $Coordinator = '',
    [string] $CoordinatorDevice = '',

    # An invitation, if the coordinator needs one.
    [string] $Invite = '',

    # Address this node serves on. Only needed when something already holds
    # 9797 here.
    [string] $Listen = '',

    # A peer to sync with directly. Repeatable.
    [string[]] $Peer = @(),

    # Do not register a scheduled task to start the daemon at logon.
    [switch] $NoTask,

    # The binary is already here; only configure.
    [switch] $NoInstall
)

$ErrorActionPreference = 'Stop'

$colour = -not $env:NO_COLOR -and $Host.UI.SupportsVirtualTerminal
function Write-Step { param($m) Write-Host ""; Write-Host "==> $m" }
function Write-Ok   { param($m) Write-Host "  ok   $m" }
function Write-Warn { param($m) Write-Host "  warn $m" }
function Write-Info { param($m) Write-Host "       $m" }
function Die {
    param([string] $Message, [string[]] $Detail = @())
    Write-Host ""
    Write-Host "error $Message"
    foreach ($line in $Detail) { Write-Host "      $line" }
    exit 1
}

# SHA-256 without `Get-FileHash`.
#
# `Get-FileHash` lives in Microsoft.PowerShell.Utility and is normally
# autoloaded, and "normally" is doing a lot of work in that sentence: run under
# `powershell.exe -NoProfile` from a non-interactive parent, this script died
# with "Le terme Get-FileHash n'est pas reconnu". The check that proves the
# install works is not the place to depend on a module resolving. .NET is
# always there.
function Get-Sha256 {
    param([string] $Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($Path)
        try {
            return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '')
        } finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

$binDir = Join-Path $env:LOCALAPPDATA 'Programs\itsanas\bin'
$bin = Join-Path $binDir 'itsanas.exe'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$taskName = 'ITSaNAS'
$secretDir = Join-Path $env:LOCALAPPDATA 'itsanas'
$secretFile = Join-Path $secretDir 'passphrase.txt'

# ------------------------------------------------------------ what was asked

Write-Host "ITSaNAS provisioning for Windows"
Write-Step 'Checking what was asked for'

$passphrase = $env:ITSANAS_PASSPHRASE
if ([string]::IsNullOrWhiteSpace($passphrase)) {
    Die 'no passphrase' @(
        'It unlocks this machine''s keystore, and the daemon needs it at every',
        'start, so it has to be somewhere a background task can read it.',
        '',
        '  $env:ITSANAS_PASSPHRASE = ''a long one you have written down''',
        '',
        'It is taken from the environment and not from an argument because',
        'arguments are visible to every process on this machine.'
    )
}
Write-Ok 'passphrase supplied'

if ($PhraseFile) {
    if (-not (Test-Path -LiteralPath $PhraseFile)) { Die "cannot read $PhraseFile" }
    Write-Ok "restoring an existing account from $PhraseFile"
    if (-not $Username) { Die '-PhraseFile also needs -Username' }
} elseif (-not $Username) {
    Die 'no -Username' @('This machine has to belong to an account. Pick a name.')
} else {
    Write-Ok "no phrase given, so this machine creates the account $Username"
}

# ------------------------------------------------------------------- install

if (-not $NoInstall) {
    Write-Step 'The binary'
    $installer = Join-Path $here 'windows.ps1'
    if (-not (Test-Path -LiteralPath $installer)) {
        Die "windows.ps1 is not next to this script" @(
            "Run this from a checkout, or install first and re-run with -NoInstall."
        )
    }
    # -NoSmoke because this script runs its own check at the end, against the
    # account it just made rather than against a throwaway one.
    & powershell -NoProfile -ExecutionPolicy Bypass -File $installer -Yes -NoSmoke
    if ($LASTEXITCODE -ne 0) { Die 'the installer failed' @('Its output is above.') }
}

if (-not (Test-Path -LiteralPath $bin)) {
    Die "no itsanas.exe at $bin" @('Run without -NoInstall, or install it first.')
}
Write-Ok "$(& $bin --version)"

# ------------------------------------------------- a node that already runs

Write-Step 'A node that is already running'

# redb allows exactly one writer, so with the daemon up every step below --
# init, pledge, folder, coordinator, register, status -- refuses with "already
# open in another process". That makes this script fail on precisely the
# machines it has already succeeded on, because the second run of an installer
# is the run where the daemon exists.
$wasRunning = $false
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($null -ne $task -and $task.State -eq 'Running') {
    $wasRunning = $true
    Stop-ScheduledTask -TaskName $taskName
    Write-Ok "stopped the $taskName task so the store can be opened"
}
$running = Get-Process -Name 'itsanas' -ErrorAction SilentlyContinue
if ($running) {
    $wasRunning = $true
    $running | Stop-Process -Force
    Write-Ok "stopped $($running.Count) running itsanas process(es)"
}
if (-not $wasRunning) { Write-Ok 'nothing is holding the node''s state' }

$env:ITSANAS_PASSPHRASE = $passphrase

# ------------------------------------------------------------- the account

Write-Step 'The account'

# The keystore, not `itsanas status`. See the header: status cannot tell "no
# node" from "node busy", which are the two cases this has to separate.
$nodeHome = if ($env:ITSANAS_HOME) { $env:ITSANAS_HOME } else { Join-Path $env:USERPROFILE '.itsanas' }
if (Test-Path -LiteralPath (Join-Path $nodeHome 'keystore.bin')) {
    Write-Ok 'a node already exists here; leaving it alone'
} elseif ($PhraseFile) {
    & $bin login --username $Username --phrase-file $PhraseFile
    if ($LASTEXITCODE -ne 0) {
        Die 'could not restore the account' @(
            'The phrase and the username have to match the ones used when the',
            'account was created.'
        )
    }
    Write-Ok "restored $Username on this machine"
} else {
    & $bin init --username $Username
    if ($LASTEXITCODE -ne 0) { Die 'could not create the account' @('The output is above.') }
    Write-Warn 'WRITE THOSE TWENTY-FOUR WORDS DOWN, ON PAPER, NOW'
    Write-Info 'They are the only way to recover this account on a new machine.'
    Write-Info 'They are not stored anywhere else and cannot be reissued.'
}

# ------------------------------------------------------------ configuration

Write-Step 'Configuring this machine'

if ($Pledge) {
    & $bin pledge $Pledge
    if ($LASTEXITCODE -ne 0) { Die "could not set the pledge to $Pledge" }
} else {
    Write-Warn 'no -Pledge, so this node offers nothing and hosts nobody'
    Write-Info 'A node that pledges nothing is a client, not a member.'
}

# Before the coordinator step, not after: `register` publishes this address, so
# setting it afterwards leaves the directory handing other members a port this
# node does not answer on.
if ($Listen) {
    & $bin listen $Listen
    if ($LASTEXITCODE -ne 0) { Die "could not set the listen address to $Listen" }
}

if ($Folder) {
    # Say how much is there before ingesting it. Pointing this at a directory
    # that already holds a lot is a big action and the reader should see its
    # size first -- on the machine this was written against, an earlier run was
    # aimed at a folder holding an old 13 GB checkout and started storing all
    # of it.
    New-Item -ItemType Directory -Force -Path $Folder | Out-Null
    $existing = Get-ChildItem -LiteralPath $Folder -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum
    if ($existing.Count -gt 0) {
        Write-Warn ("$Folder already holds {0} files, {1:N1} GB" -f $existing.Count, ($existing.Sum / 1GB))
        Write-Info 'All of it will be stored and offered to your peers.'
    }
    & $bin folder $Folder
    if ($LASTEXITCODE -ne 0) { Die 'could not set the synced folder' }
}

foreach ($address in $Peer) {
    & $bin peer add $address
    if ($LASTEXITCODE -ne 0) { Write-Warn "could not add the peer $address" }
}

if ($Coordinator) {
    if ($CoordinatorDevice) {
        & $bin coordinator $Coordinator --device $CoordinatorDevice
    } else {
        Write-Warn 'a coordinator without -CoordinatorDevice is not pinned'
        Write-Info 'An address that resolves elsewhere would be trusted rather than refused.'
        & $bin coordinator $Coordinator
    }
    if ($LASTEXITCODE -ne 0) { Die 'could not set the coordinator' }

    if ($Invite) { & $bin register --invite $Invite } else { & $bin register }
    if ($LASTEXITCODE -ne 0) {
        Die 'the coordinator refused this account' @(
            'If it admits members by invitation, ask one of them for a code:',
            '  itsanas invite            on a machine that is already a member',
            'then re-run this with -Invite <code>.'
        )
    }
}

# ------------------------------------------------------------- the daemon

$taskOk = $false
if (-not $NoTask) {
    Write-Step 'The background task'

    New-Item -ItemType Directory -Force -Path $secretDir | Out-Null

    # Locked down before the secret goes in, not after.
    Set-Content -LiteralPath $secretFile -Value '' -NoNewline
    icacls $secretFile /inheritance:r /grant:r "$($env:USERNAME):(R,W)" | Out-Null
    Set-Content -LiteralPath $secretFile -Value $passphrase -NoNewline -Encoding ascii
    Write-Ok "$secretFile (readable only by $env:USERNAME)"
    Write-Info 'Anything running as you can read that file. That is the trade a'
    Write-Info 'background service makes; it is not a default this script hid.'

    $wrapper = Join-Path $secretDir 'run-daemon.ps1'
    @(
        '# Started by the ITSaNAS scheduled task at logon. A daemon cannot be',
        '# prompted, so the passphrase comes from a file only this account can read.',
        '$env:ITSANAS_PASSPHRASE = Get-Content "$env:LOCALAPPDATA\itsanas\passphrase.txt" -Raw',
        '& "$env:LOCALAPPDATA\Programs\itsanas\bin\itsanas.exe" daemon'
    ) | Set-Content -LiteralPath $wrapper -Encoding utf8

    $action = New-ScheduledTaskAction -Execute 'powershell.exe' `
        -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$wrapper`""
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
    try {
        Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `
            -Settings $settings -Force `
            -Description 'ITSaNAS peer-to-peer storage daemon' | Out-Null
        Start-ScheduledTask -TaskName $taskName
        Start-Sleep -Seconds 3
        $taskOk = $null -ne (Get-Process -Name 'itsanas' -ErrorAction SilentlyContinue)
        if ($taskOk) {
            Write-Ok "the $taskName task is registered and the daemon is running"
        } else {
            Write-Warn 'the task was registered but the daemon is not running'
            Write-Info "  Get-ScheduledTaskInfo -TaskName $taskName"
        }
    } catch {
        Write-Warn "could not register the task: $($_.Exception.Message)"
        Write-Info 'Run the daemon by hand with:  itsanas daemon'
    }
}

# ------------------------------------------------------------------- check

Write-Step 'Does it work here?'

# A throwaway home, so this proves the binary works without touching the
# account that was just made.
$scratch = Join-Path ([System.IO.Path]::GetTempPath()) ("itsanas-smoke-" + [guid]::NewGuid())
try {
    $env:ITSANAS_HOME = Join-Path $scratch 'home'
    $env:ITSANAS_PASSPHRASE = 'a passphrase for a throwaway smoke test account'
    New-Item -ItemType Directory -Force -Path $scratch | Out-Null

    & $bin init --username smoketest 2>&1 | Out-Null
    $payload = Join-Path $scratch 'payload.bin'
    $bytes = New-Object byte[] (350 * 1024)
    (New-Object Random 42).NextBytes($bytes)
    [System.IO.File]::WriteAllBytes($payload, $bytes)

    & $bin put 'docs/smoke.bin' $payload 2>&1 | Out-Null
    $back = Join-Path $scratch 'back.bin'
    & $bin get 'docs/smoke.bin' $back 2>&1 | Out-Null

    $before = Get-Sha256 $payload
    $after = Get-Sha256 $back
    if ($before -eq $after) {
        Write-Ok 'stored a 350 KB file across chunks and read it back byte for byte'
    } else {
        Die 'a file did not come back the way it went in' @(
            'This is the interesting kind of failure and is worth reporting.'
        )
    }
} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item Env:\ITSANAS_HOME -ErrorAction SilentlyContinue
    $env:ITSANAS_PASSPHRASE = $passphrase
}

# -------------------------------------------------------------------- done

Write-Step 'Done'

# If this script stopped a daemon and did not start one, it owes the machine
# the state it found it in. -NoTask is the path that gets here.
if ($wasRunning -and -not $taskOk) {
    $env:ITSANAS_PASSPHRASE = $passphrase
    Start-Process -FilePath $bin -ArgumentList 'daemon' -WindowStyle Hidden `
        -RedirectStandardOutput "$env:TEMP\itsanas-daemon.log" `
        -RedirectStandardError "$env:TEMP\itsanas-daemon.err"
    Start-Sleep -Seconds 3
    if (Get-Process -Name 'itsanas' -ErrorAction SilentlyContinue) {
        $taskOk = $true
        Write-Ok 'the daemon was running when this started and is running again'
    } else {
        Write-Warn 'the daemon was running when this started and is now stopped'
        Write-Info '  itsanas daemon'
    }
}

# What this prints is what the machine is, not what the script meant to do.
if ($taskOk) {
    Write-Host ""
    Write-Host "       Watch it:  Get-Content `$env:TEMP\itsanas-daemon.log -Wait"
    Write-Host "       Stop it:   Stop-ScheduledTask -TaskName $taskName"
    Write-Host "       Ask it:    itsanas status   (stop it first: one writer)"
} else {
    Write-Host ""
    Write-Host "       There is no background task running on this machine. Start the"
    Write-Host "       daemon by hand when you want the node up:"
    Write-Host ""
    Write-Host "           itsanas daemon"
}
Write-Host ""
Write-Host "       To rebuild this machine, keep the command you just ran. That is"
Write-Host "       the whole point of this script: it is the artefact, not the machine."
Write-Host ""
