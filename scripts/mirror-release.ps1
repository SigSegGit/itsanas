<#
.SYNOPSIS
    Copy a published, signed release to the mirror on the coordinator's host,
    for networks that cannot reach GitHub. Run by sign-release.ps1 after it
    publishes; safe to run again by hand.

.DESCRIPTION
    Every program asks GitHub first, then https://<coordinator host>/releases/
    (update.rs `mirror`, Updater.kt). That second place serves a directory as
    static files over HTTPS, laid out as GitHub's:

        <dir>/download/v<version>/<every file of the release>
        <dir>/latest/download  ->  ../download/v<version>   (a link)

    This script downloads the release from GitHub, copies it into
    download/v<version>/ with scp, and only then moves the `latest` link, in
    one rename: a program never sees a manifest whose files are not there yet.
    Nothing here is trusted by a program: each checks the signature.

    The target comes from ITSANAS_MIRROR on this PC, as `<ssh host>:<dir>`;
    unset, the release stays on GitHub alone and this says so.

.PARAMETER Tag
    The release to copy. Default: the latest published one.
#>
param(
    [string] $Tag = '',
    [string] $Repo = $(if ($env:ITSANAS_REPO) { $env:ITSANAS_REPO } else { 'SigSegGit/itsanas' }),
    [string] $Target = $env:ITSANAS_MIRROR
)

$ErrorActionPreference = 'Continue'

function Stop-Plain {
    param([string] $Message)
    Write-Host "mirror-release: $Message" -ForegroundColor Red
    exit 1
}

if (-not $Target) {
    Write-Host 'No mirror (ITSANAS_MIRROR is not set on this PC): the release is on GitHub only.'
    exit 0
}
$colon = $Target.IndexOf(':')
if ($colon -lt 1 -or $colon -eq $Target.Length - 1) {
    Stop-Plain "ITSANAS_MIRROR must be <ssh host>:<directory>, not '$Target'"
}
$sshHost = $Target.Substring(0, $colon)
$dir = $Target.Substring($colon + 1).TrimEnd('/')
# Both go into commands the remote shell reads: only characters no shell
# treats specially, as the tag below is only v1.2.3.
if ($sshHost -notmatch '^[A-Za-z0-9._@-]+$' -or $dir -notmatch '^[A-Za-z0-9._/-]+$') {
    Stop-Plain "ITSANAS_MIRROR may hold only letters, digits and . _ - / @, not '$Target'"
}

if (-not $Tag) {
    $Tag = (& gh release view -R $Repo --json tagName -q .tagName) | Select-Object -First 1
    if ($LASTEXITCODE -ne 0 -or -not $Tag) { Stop-Plain "could not find the latest release of $Repo" }
}
if ($Tag -notmatch '^v\d+\.\d+\.\d+$') { Stop-Plain "'$Tag' is not a release tag (v1.2.3)" }

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("itsanas-mirror-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    & gh release download $Tag -R $Repo -p manifest.txt -p manifest.txt.sig -p 'itsanas-*' -p 'ITSaNAS-*' -D $work
    if ($LASTEXITCODE -ne 0) { Stop-Plain "could not download $Tag from GitHub" }
    if (-not (Test-Path -LiteralPath (Join-Path $work 'manifest.txt.sig'))) {
        Stop-Plain "$Tag has no signature: sign and publish it first (sign-release.ps1)"
    }

    $release = "$dir/download/$Tag"
    & ssh $sshHost "mkdir -p '$release' '$dir/latest'"
    if ($LASTEXITCODE -ne 0) { Stop-Plain "could not reach $sshHost over ssh" }
    $files = @(Get-ChildItem -LiteralPath $work -File | ForEach-Object { $_.FullName })
    & scp -q @files "${sshHost}:$release/"
    if ($LASTEXITCODE -ne 0) { Stop-Plain "the files could not be copied to ${sshHost}: run this again, it is safe" }
    # The link moves in one rename (mv -T), after every file is in place.
    & ssh $sshHost "ln -sfn '../download/$Tag' '$dir/latest/.next' && mv -Tf '$dir/latest/.next' '$dir/latest/download'"
    if ($LASTEXITCODE -ne 0) { Stop-Plain "the files are on $sshHost but latest still points to the previous release: run this again" }
    Write-Host "Mirrored $Tag ($($files.Count) files) to the coordinator's host." -ForegroundColor Green
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
