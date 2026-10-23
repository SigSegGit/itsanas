<#
.SYNOPSIS
    The one thing the fleet CI key may run on a Windows machine of the fleet:
    scripts/fleet/agent.sh, for the laptop.

.DESCRIPTION
    administrators_authorized_keys (or the user's authorized_keys):
      restrict,command="powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\Users\<you>\.itsanas-fleet\agent.ps1" ssh-ed25519 AAAA... itsanas-fleet-ci

    Win32-OpenSSH runs this script whatever the caller asked, with the request
    in SSH_ORIGINAL_COMMAND, as sshd does on Linux. What it does is the kit's
    Windows half, scripts/acceptance.ps1, and nothing of its own: the same
    allowlist as agent.sh, the same names, the same refusals (exit 64).

    What a leaked key can do here: write files of about a mebibyte into the
    test account's folder (MaxFiles at most), delete files the kit wrote, read
    whether one is there and its digest, and read the test node's status
    snapshot. No C scan: the laptop's vault is not read from here (yet).

    Configuration: agent.json beside this script, written by hand or by a setup
    to come (docs/FLEET-CI.md, "What it does not do yet"):
      { "Folder": "C:\\Users\\you\\ITSaNAS-test", "Kit": "...\\acceptance.ps1",
        "Receipts": "...", "Homes": { "test": "C:\\Users\\you\\.itsanas-test" },
        "MaxFiles": 50 }
#>
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $PSCommandPath
$confPath = if ($env:ITSANAS_FLEET_CONF) { $env:ITSANAS_FLEET_CONF } else { Join-Path $here 'agent.json' }

function Refuse([string] $Why) {
    Write-Output "REFUSED $Why"
    exit 64
}

if (-not (Test-Path -LiteralPath $confPath)) { Refuse "no configuration at $confPath" }
$conf = Get-Content -LiteralPath $confPath -Raw | ConvertFrom-Json
$folder = [string] $conf.Folder
$kit = if ($conf.Kit) { [string] $conf.Kit } else { Join-Path $here 'acceptance.ps1' }
$maxFiles = if ($conf.MaxFiles) { [int] $conf.MaxFiles } else { 50 }
if (-not $folder -or -not (Test-Path -LiteralPath $folder -PathType Container)) { Refuse "the test folder '$folder' does not exist" }
if (-not (Test-Path -LiteralPath $kit)) { Refuse "no acceptance kit at $kit" }
if ($conf.Receipts) { $env:ITSANAS_RECEIPTS = [string] $conf.Receipts }

# The same wall as agent.sh: plain words or refused whole.
$request = [string] $env:SSH_ORIGINAL_COMMAND
if ($request.Length -gt 256) { Refuse 'request longer than 256 characters' }
if ($request -cmatch '[^A-Za-z0-9._: -]') { Refuse 'a character outside [A-Za-z0-9._: -]' }
$words = @($request -split ' ' | Where-Object { $_ -ne '' })
if ($words.Count -eq 0) { Refuse 'an empty request (try hello)' }
foreach ($word in $words) {
    if ($word.StartsWith('-')) { Refuse "a token starting with '-'" }
    if ($word.Contains('..')) { Refuse "a token containing '..'" }
    if ($word.Length -gt 64) { Refuse 'a token longer than 64 characters' }
}

function Is-Sha([string] $Text) { $Text -cmatch '^[0-9a-f]{64}$' }
function Is-Code([string] $Text) { $Text -cmatch '^[0-9a-f]{4}-?[0-9a-f]{4}$' }
function Is-KitName([string] $Text) {
    ($Text -cmatch '^acceptance-[A-Z]-[0-9]{8}T[0-9]{6}Z-[0-9]+\.bin$') -or ($Text -cmatch '^ITSANAS-CANARY-[0-9a-f]{24}\.txt$')
}
function Room-For-One-More {
    $made = @(Get-ChildItem -LiteralPath $folder -File | Where-Object { Is-KitName $_.Name }).Count
    if ($made -ge $maxFiles) { Refuse "already $maxFiles files from the kit in the test folder; F delete some" }
}
function Kit {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $kit @args
    exit $LASTEXITCODE
}
# Where pwsh runs this (CI, a test), powershell.exe may not exist.
if (-not (Get-Command powershell.exe -ErrorAction SilentlyContinue)) {
    function Kit { & pwsh -NoProfile -File $kit @args; exit $LASTEXITCODE }
}

$w = $words
$n = $w.Count
switch ("$($w[0]) $(if ($n -gt 1) { $w[1] })") {
    'hello ' {
        if ($n -ne 1) { Refuse 'usage: hello' }
        $sha = (Get-FileHash -LiteralPath $kit -Algorithm SHA256).Hash.ToLowerInvariant().Substring(0, 16)
        $accounts = @($conf.Homes.PSObject.Properties.Name) -join ' '
        Write-Output "hello $([System.Net.Dns]::GetHostName()) agent 1 accounts $accounts kit $sha"
        exit 0
    }
    { $_ -in 'B write', 'E write' } {
        if ($n -ne 2) { Refuse "usage: $($w[0]) write" }
        Room-For-One-More
        Kit $w[0] write $folder
    }
    'C plant' {
        if ($n -ne 2) { Refuse 'usage: C plant' }
        Room-For-One-More
        Kit C plant $folder
    }
    { $_ -in 'B check', 'E check' } {
        if ($n -eq 4 -and (Is-KitName $w[2]) -and (Is-Sha $w[3])) { Kit $w[0] check $folder $w[2] $w[3] }
        if ($n -eq 3 -and (Is-Code $w[2])) { Kit $w[0] check $folder $w[2] }
        Refuse "usage: $($w[0]) check <kit file> <sha256> | <code>"
    }
    { $_ -in 'F delete', 'F check' } {
        if ($n -ne 3 -or -not (Is-KitName $w[2])) { Refuse "usage: F $($w[1]) <kit file>" }
        Kit F $w[1] $folder $w[2]
    }
    'C scan' { Refuse 'C scan is not offered on Windows: the vault is not read from here' }
    default {
        if ($w[0] -eq 'status' -and $n -eq 2) {
            $entry = $conf.Homes.PSObject.Properties | Where-Object { $_.Name -ceq $w[1] } | Select-Object -First 1
            if (-not $entry) { Refuse "no account '$($w[1])' on this machine" }
            $snapshot = Join-Path ([string] $entry.Value) 'status.snapshot'
            if (-not (Test-Path -LiteralPath $snapshot)) {
                Write-Output "NOSNAPSHOT $($w[1]) has no status.snapshot: its daemon has not run here"
                exit 1
            }
            $lines = Get-Content -LiteralPath $snapshot
            if ($lines[0] -match '^snapshot (\d+) every (\d+)') {
                Write-Output "snapshot $($Matches[1]) $($Matches[2]) now $([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"
            }
            $version = if ($lines[0] -match ' version ([0-9A-Za-z.+-]+)') { $Matches[1] } else { 'unknown' }
            Write-Output "version $version"
            $lines | Where-Object { $_ -match '^  (username|device|files|live chunks)' }
            exit 0
        }
        Refuse "unknown request '$request' (hello, B|E write|check, C plant, F delete|check, status)"
    }
}
