<#
.SYNOPSIS
    Remove everything an ITSaNAS install put on this machine.

.DESCRIPTION
    Two scripts install things here — `windows.ps1` puts the programs in place,
    `provision.ps1` creates the account, the scheduled task and the file holding
    the passphrase — and until now nothing took them away. Uninstalling meant
    remembering six paths, two of them written by a script the person may never
    have read, and one of them a file holding a passphrase.

    An install that cannot be undone is not an install anybody should trust with
    a disk. It is also, in practice, how a machine ends up with two versions of
    the same daemon and a task pointing at a binary that is no longer there.

    **The account is not removed unless asked twice.** `keystore.bin` holds this
    machine's sealed copy of the master secret, and the store holds the only
    copy of anything not yet replicated elsewhere. That is losing data, not
    uninstalling a program, so it takes its own switch.

    The dry run is the default, because the first thing anybody does with an
    unfamiliar clean-up script is run it to see what it says.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File install\clean.ps1
    powershell -ExecutionPolicy Bypass -File install\clean.ps1 -Yes
    powershell -ExecutionPolicy Bypass -File install\clean.ps1 -Yes -PurgeAccount
#>
[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$PurgeAccount,
    [string]$NodeHome = $(if ($env:ITSANAS_HOME) { $env:ITSANAS_HOME } else { "$env:USERPROFILE\.itsanas" })
)

# Not `Stop`: several of the probes below are expected to fail on a machine that
# never had this installed, and a missing scheduled task is an answer rather
# than an error. The same reasoning as `provision.ps1`, which was bitten by it.
$ErrorActionPreference = 'Continue'

$programs = "$env:LOCALAPPDATA\Programs\itsanas"
$state = "$env:LOCALAPPDATA\itsanas"
$taskName = 'ITSaNAS'

function Plan([string]$line) { Write-Host "  $line" }

Write-Host "ITSaNAS clean-up"
Write-Host ""

Write-Host "the scheduled task"
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($task) {
    Plan "stop and remove the task '$taskName' (state: $($task.State))"
} else {
    Plan "no task named '$taskName'"
}
$running = @(Get-Process itsanas, itsanas-drive -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) { Plan "stop $($running.Count) running process(es)" }

Write-Host ""
Write-Host "the programs"
if (Test-Path $programs) { Plan "remove $programs" } else { Plan "nothing at $programs" }

Write-Host ""
Write-Host "the passphrase and the wrapper"
foreach ($file in @("$state\passphrase.txt", "$state\run-daemon.ps1", "$state\sampler.ps1")) {
    if (Test-Path $file) { Plan "remove $file" }
}
if (Test-Path $state) { Plan "remove the logs in $state" }

Write-Host ""
Write-Host "the account"
if (Test-Path $NodeHome) {
    if ($PurgeAccount) {
        Plan "REMOVE $NodeHome - the sealed master secret and every chunk on this machine"
        Plan "anything here and nowhere else is gone for good"
    } else {
        Plan "keep $NodeHome (pass -PurgeAccount to remove it)"
    }
} else {
    Plan "no node at $NodeHome"
}

if (-not $Yes) {
    Write-Host ""
    Write-Host "Nothing was changed. Add -Yes to do it."
    exit 0
}

Write-Host ""

# The task first. Removing a binary out from under a running daemon leaves a
# process with a deleted executable, which restarts into nothing.
if ($task) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
}
Get-Process itsanas, itsanas-drive -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Write-Host "task and processes stopped"

if (Test-Path $programs) {
    Remove-Item $programs -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "programs removed"
}

if (Test-Path $state) {
    Remove-Item $state -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "passphrase, wrapper and logs removed"
}

if ($PurgeAccount -and (Test-Path $NodeHome)) {
    Remove-Item $NodeHome -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "account removed from $NodeHome"
} elseif (Test-Path $NodeHome) {
    Write-Host "account left at $NodeHome"
}

# A daemon that is gone is still a holder in somebody else's ledger until they
# notice. Saying so is the difference between "I uninstalled it" and "my
# friend's replica count silently dropped".
Write-Host ""
Write-Host "Note: other members still count this machine as holding their data until"
Write-Host "their next audit withdraws it. If this was a host for somebody, tell them."
