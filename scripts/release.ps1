<#
.SYNOPSIS
    From an open PR (or main as it is) to a signed, published release, in one
    command. Nicolas's PC, PowerShell.

.DESCRIPTION
    Run it and leave: the only questions come at the very end, from
    sign-release.ps1 (confirm, then the key's passphrase), and the PC beeps
    when they do.

      1. goes to the checkout this script lives in; refuses uncommitted
         changes rather than guess what to do with them;
      2. with -Pr: marks that PR ready, turns on GitHub's auto-merge and
         waits until GitHub has merged it (a red required check stops
         everything);
      3. brings local main up to origin/main;
      4. takes the tag from Cargo.toml's [workspace.package] version, so the
         Release workflow's version check cannot refuse it. Refuses a
         version already published: bump it in the PR first;
      5. cleans what a failed attempt left: a tag of that version on another
         commit (pushed before its PR merged) and its draft release;
      6. pushes the tag, waits for the Release workflow to finish;
      7. runs sign-release.ps1 on that tag.

.PARAMETER Pr
    The PR to merge first (usually the one bumping the version). Without it,
    main is released as it is.

.EXAMPLE
    itsanas-release -Pr 271
#>
param(
    [int] $Pr = 0,
    [string] $Repo = $(if ($env:ITSANAS_REPO) { $env:ITSANAS_REPO } else { 'SigSegGit/itsanas' })
)

# 'Continue', not 'Stop': Windows PowerShell 5 turns a native command's
# stderr into a terminating error under 'Stop', even redirected. Every
# native call's exit code is checked instead.
$ErrorActionPreference = 'Continue'

function Stop-Plain {
    param([string] $Message)
    Write-Host ''
    Write-Host "release: $Message" -ForegroundColor Red
    [console]::Beep(400, 600)
    exit 1
}

function Say {
    param([string] $Message)
    Write-Host ''
    Write-Host "== $Message" -ForegroundColor Cyan
}

# A native command's failure does not stop PowerShell 5 by itself.
function Invoke-Checked {
    param([string] $What, [scriptblock] $Command)
    & $Command
    if ($LASTEXITCODE -ne 0) { Stop-Plain $What }
}

Set-Location (Split-Path -Parent $PSScriptRoot)
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Stop-Plain 'gh is not installed: winget install GitHub.cli, then gh auth login'
}
if (git status --porcelain) {
    git status --short
    Stop-Plain "uncommitted changes in $(Get-Location): commit or stash them first"
}

if ($Pr -gt 0) {
    # GitHub merges it once `No warnings anywhere in this run` passes (main
    # requires that check, auto-merge is allowed): enable that, then wait for
    # the merge rather than judge the checks here.
    Say "PR #${Pr}: auto-merge on, waiting for its CI and the merge"
    gh pr ready $Pr -R $Repo 2>$null | Out-Null
    Invoke-Checked "PR #${Pr}: auto-merge could not be enabled" { gh pr merge $Pr -R $Repo --auto --squash }
    while ($true) {
        $state = gh pr view $Pr -R $Repo --json state -q .state
        if ($state -eq 'MERGED') { break }
        if ($state -eq 'CLOSED') { Stop-Plain "PR #$Pr was closed without merging" }
        # A failed required check leaves the PR open and blocked for good.
        if (gh pr checks $Pr -R $Repo --required 2>$null | Select-String -Pattern '\bfail' -Quiet) {
            Stop-Plain "PR #${Pr}: CI failed, nothing merged (gh pr checks $Pr -R $Repo)"
        }
        Start-Sleep -Seconds 30
    }
}

Say 'Updating main'
Invoke-Checked 'git fetch failed' { git fetch --prune --prune-tags --tags --force origin }
Invoke-Checked 'git checkout main failed' { git checkout -q main }
Invoke-Checked 'local main has commits origin/main does not: push or drop them' { git merge -q --ff-only origin/main }

$cargo = Get-Content Cargo.toml -Raw
if ($cargo -notmatch '(?ms)^\[workspace\.package\][^\[]*?^version\s*=\s*"(\d+\.\d+\.\d+)"') {
    Stop-Plain 'no version found under [workspace.package] in Cargo.toml'
}
$version = $Matches[1]
$tag = "v$version"
$head = (git rev-parse HEAD).Trim()
Write-Host "main is $(git log -1 --format='%h %s'); version $version, tag $tag"

$draft = (gh release view $tag -R $Repo --json isDraft -q .isDraft 2>$null)
if ($draft -eq 'false') {
    Stop-Plain "$tag is already published: bump the version in Cargo.toml (in the PR) first"
}
git rev-parse -q --verify "refs/tags/$tag" *> $null
$tagExists = ($LASTEXITCODE -eq 0)
$pushedAt = ''
if ($tagExists -and (git rev-list -n1 $tag).Trim() -ne $head) {
    Say "Cleaning: $tag points to $(git log -1 --format='%h %s' $tag), not main"
    if ($draft -eq 'true') { Invoke-Checked "could not delete the draft release $tag" { gh release delete $tag -R $Repo --yes } }
    git push -q origin ":refs/tags/$tag" 2>$null
    git tag -d $tag | Out-Null
    $tagExists = $false
}
if (-not $tagExists) {
    Say "Tagging $tag"
    Invoke-Checked 'git tag failed' { git tag $tag }
    $pushedAt = (Get-Date).ToUniversalTime().AddSeconds(-30).ToString('yyyy-MM-ddTHH:mm:ssZ')
    Invoke-Checked 'pushing the tag failed' { git push -q origin $tag }
}

Say "Waiting for the Release workflow on $tag (about 20 minutes)"
$run = ''
for ($i = 0; $i -lt 60 -and -not $run; $i++) {
    $run = gh run list -R $Repo --workflow release.yml --branch $tag --limit 5 --json databaseId,createdAt `
        -q "[.[] | select(.createdAt >= `"$pushedAt`")][0].databaseId // empty"
    if (-not $run) { Start-Sleep -Seconds 5 }
}
if (-not $run) { Stop-Plain "no Release run started for $tag after 5 minutes: https://github.com/$Repo/actions" }
Invoke-Checked "the Release workflow failed: gh run view $run -R $Repo --log-failed" {
    gh run watch $run -R $Repo --exit-status --interval 30
}

Say "Signing and publishing $tag"
[console]::Beep(880, 300); [console]::Beep(1320, 300)
& (Join-Path $PSScriptRoot 'sign-release.ps1') -Tag $tag -Repo $Repo
