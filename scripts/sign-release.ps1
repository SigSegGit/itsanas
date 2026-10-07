<#
.SYNOPSIS
    Sign and publish the ITSaNAS release CI has just built. Double-click
    sign-release.cmd beside this file, or run it from PowerShell.

.DESCRIPTION
    The release workflow (.github/workflows/release.yml) builds the binaries on
    a v* tag and leaves a DRAFT release holding them and an unsigned
    manifest.txt. This script, on Nicolas's PC:

      1. makes the release key the first time (asks before doing it, and says
         where the offline copy goes);
      2. finds the newest draft release with `gh`;
      3. downloads its manifest.txt and binaries, refuses unless each binary
         matches the manifest and the manifest's version is the tag's
         (`itsanas-release check`, which prints each SHA-256), and shows what
         it is about to sign;
      4. signs it with `itsanas-release sign`, which asks the passphrase itself,
         hidden -- this script never sees, prints or stores it;
      5. uploads manifest.txt.sig and publishes the release;
      6. says what was published.

    Nothing secret leaves this PC: CI never holds the key (decided 2026-10-06,
    docs/RELEASING.md).

.PARAMETER Key
    The sealed key file. Default: $env:ITSANAS_RELEASE_KEY, else
    %USERPROFILE%\itsanas-release-key\release-signing.key. Deliberately not
    under %USERPROFILE%\.itsanas-*: install\clean.ps1 deletes those.

.PARAMETER Tag
    Sign this draft instead of the newest one.
#>
param(
    [string] $Key = $(if ($env:ITSANAS_RELEASE_KEY) { $env:ITSANAS_RELEASE_KEY } else { "$env:USERPROFILE\itsanas-release-key\release-signing.key" }),
    [string] $Tag = '',
    [string] $Repo = $(if ($env:ITSANAS_REPO) { $env:ITSANAS_REPO } else { 'SigSegGit/itsanas' })
)

$ErrorActionPreference = 'Stop'
$checkout = Split-Path -Parent $PSScriptRoot

# One plain line and a way out, never a stack trace: this is double-clicked.
function Stop-Plain {
    param([string] $Message)
    Write-Host ''
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Confirm-Yes {
    param([string] $Question)
    $answer = Read-Host "$Question [y/N]"
    return $answer -match '^(y|yes|o|oui)$'
}

# The signing tool is built from this checkout, so the code that touches the
# key is the code in the repository, not a binary downloaded from anywhere.
# Called as a statement, never inside `( ... )`: captured, its output (the
# public key and the offline-copy advice) would vanish into a variable instead
# of reaching the screen. The caller reads $LASTEXITCODE afterwards.
function Invoke-ReleaseTool {
    param([string[]] $Arguments)
    & cargo run -q --release --manifest-path (Join-Path $checkout 'Cargo.toml') -p itsanas-release -- @Arguments
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Stop-Plain 'the GitHub command line (gh) is not installed: winget install GitHub.cli, then gh auth login'
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Stop-Plain 'cargo is not installed here: install Rust from https://rustup.rs and run this again'
}
# 'Continue' around it: gh writes its status to stderr, and Windows PowerShell
# 5.1 turns redirected native stderr into a terminating error under 'Stop' --
# a stack trace on a double-click, even when gh is logged in.
$ErrorActionPreference = 'Continue'
& gh auth status *> $null
$ghAuth = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($ghAuth -ne 0) {
    Stop-Plain 'gh is not logged in to GitHub: run  gh auth login  then this again'
}

# ------------------------------------------------------------ the key, once
if (-not (Test-Path -LiteralPath $Key)) {
    Write-Host "There is no release key at $Key."
    Write-Host 'The release key signs every ITSaNAS version; nodes install only what it signed.'
    Write-Host 'It is made once, here, sealed under a passphrase you choose.'
    if (-not (Confirm-Yes 'Create it now?')) {
        Stop-Plain 'nothing was done. If your key file is elsewhere, set ITSANAS_RELEASE_KEY to its path.'
    }
    Invoke-ReleaseTool @('keygen', '--out', $Key)
    if ($LASTEXITCODE -ne 0) {
        Stop-Plain 'the key was not created (the reason is just above): run this again'
    }
    Write-Host ''
    Write-Host 'Do the offline copy now, before publishing anything: copy the file above to a'
    Write-Host 'USB stick, put the stick in a drawer, and keep the passphrase on paper elsewhere.'
    [void] (Read-Host 'Press Enter once the copy is made')
}

# ------------------------------------------------------- the draft to sign
if (-not $Tag) {
    $Tag = (& gh release list -R $Repo --limit 30 --json tagName,isDraft --jq '[.[] | select(.isDraft)][0].tagName // empty') | Select-Object -First 1
    if ($LASTEXITCODE -ne 0) { Stop-Plain "could not list the releases of ${Repo}: check your network and gh auth status" }
    if (-not $Tag) {
        Stop-Plain "no draft release in ${Repo}: push a v* tag and wait for the release workflow to finish"
    }
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("itsanas-sign-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    # Every binary too, not only the manifest: a draft is writable by anyone
    # with write access to the repository, so the manifest is signed only once
    # each binary it lists has been measured here and found identical.
    & gh release download $Tag -R $Repo -p manifest.txt -p 'itsanas-*' -D $work
    if ($LASTEXITCODE -ne 0) { Stop-Plain "could not download release ${Tag}: did the release workflow finish? Look at its run on GitHub" }
    $manifest = Join-Path $work 'manifest.txt'
    Invoke-ReleaseTool @('check', '--dir', $work, '--tag', $Tag)
    if ($LASTEXITCODE -ne 0) {
        Stop-Plain "the draft $Tag does not match its own manifest (the reason is just above): do not sign it, nothing was signed"
    }
    $lines = Get-Content -LiteralPath $manifest
    $version = ($lines | Where-Object { $_ -like 'version *' } | Select-Object -First 1) -replace '^version ', ''
    $targets = @($lines | Where-Object { $_ -like 'file *' } | ForEach-Object { ($_ -split ' ')[1] })

    Write-Host ''
    Write-Host "Draft ${Tag}: ITSaNAS $version, $($targets.Count) binaries:"
    $targets | ForEach-Object { Write-Host "  $_" }
    if (-not (Confirm-Yes 'Sign it and publish it?')) {
        Stop-Plain 'nothing was signed or published'
    }

    Invoke-ReleaseTool @('sign', $manifest, '--key', $Key)
    if ($LASTEXITCODE -ne 0) {
        Stop-Plain 'not signed (the reason is just above): nothing was published'
    }
    & gh release upload $Tag -R $Repo "$manifest.sig" --clobber
    if ($LASTEXITCODE -ne 0) { Stop-Plain "the signature could not be uploaded to ${Tag}: run this again, it is safe" }
    & gh release edit $Tag -R $Repo --draft=false
    if ($LASTEXITCODE -ne 0) { Stop-Plain "signed and uploaded, but $Tag is still a draft: run this again, or publish it on GitHub" }

    Write-Host ''
    Write-Host "Published ITSaNAS $version ($Tag), signed, for: $($targets -join ', ')." -ForegroundColor Green
    Write-Host "Testers can now install it: see docs/RELEASING.md."
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
