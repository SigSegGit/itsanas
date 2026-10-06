# A test bed in one command, on Windows. ASCII only: PowerShell 5.1 reads a
# file without a BOM as the ANSI code page.
# The Linux and macOS half is
# testbed.sh, and the reasons are written there (HANDOVER section 8, 0r).
#
#   First machine, which creates the test account:
#     powershell -ExecutionPolicy Bypass -File install\testbed.ps1 -Coordinator HOST:PORT -CoordinatorDevice ID -Invite CODE
#   Every other machine (asks for the 24 words, hidden):
#     powershell -ExecutionPolicy Bypass -File install\testbed.ps1 -Coordinator HOST:PORT -CoordinatorDevice ID
#   Unattended: -PhraseFile PATH reads the 24 words from a file you made
#   readable only by you; the script never deletes your file.
#   Any time, changes nothing:
#     powershell -ExecutionPolicy Bypass -File install\testbed.ps1 -Status
#   Remove the bed (a dry run; add -Yes):
#     powershell -ExecutionPolicy Bypass -File install\testbed.ps1 -Clean [-Yes]
#
# A thin wrapper over provision.ps1, never a fork. The bed is the account
# `essai` in the instance `essai`; real accounts on this machine are not
# touched. -Fresh archives an earlier bed, never deletes it.
param(
    [string] $Coordinator = '',
    [string] $CoordinatorDevice = '',
    [string] $Invite = '',
    [string] $PhraseFile = '',
    [switch] $Fresh,
    [switch] $Clean,
    [switch] $Yes,
    [switch] $Status
)

$ErrorActionPreference = 'Stop'
$instance = 'essai'

# First, before anything reads LOCALAPPDATA: the uninstaller must be reachable
# from any PowerShell, pwsh on Linux included (check-installers.sh runs it so).
# In-process, so -Yes stays a switch instead of crossing a command line.
if ($Clean) {
    & (Join-Path $PSScriptRoot 'clean.ps1') -Instance $instance -Yes:$Yes
    exit $LASTEXITCODE
}
$nodeHome = Join-Path $env:USERPROFILE ".itsanas-$instance"
$folder = Join-Path $env:USERPROFILE "ITSaNAS-$instance"
$bin = Join-Path $env:LOCALAPPDATA 'Programs\itsanas\bin\itsanas.exe'
$secretFile = Join-Path $env:LOCALAPPDATA "itsanas\passphrase-$instance.txt"
$taskName = "ITSaNAS-$instance"
$hostName = $env:COMPUTERNAME.ToLower()

function Die { param([string] $Message, [string[]] $Detail = @())
    Write-Host ""; Write-Host "error $Message" -ForegroundColor Red
    foreach ($line in $Detail) { Write-Host "      $line" }
    exit 1
}

function Show-Status {
    Write-Host ""; Write-Host "ITSaNAS test bed on $hostName"; Write-Host ""
    $failures = 0
    function Line { param([bool] $Good, [string] $Text)
        if ($Good) { Write-Host "  [ OK ] $Text" -ForegroundColor Green }
        else { Write-Host "  [FAIL] $Text" -ForegroundColor Red; $script:failures++ }
    }
    $script:failures = 0
    Line (Test-Path -LiteralPath $bin) $(if (Test-Path -LiteralPath $bin) { "installed      $(& $bin --version)" } else { "installed      no $bin -- run the setup command again" })
    Line (Test-Path -LiteralPath (Join-Path $nodeHome 'keystore.bin')) "account        essai, node in $nodeHome"
    $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    $running = ($null -ne $task -and $task.State -eq 'Running')
    Line $running $(if ($running) { 'daemon         running' } else { "daemon         stopped -- Start-ScheduledTask $taskName" })
    $others = @(Get-ChildItem -LiteralPath $folder -Filter 'bonjour-depuis-*.txt' -ErrorAction SilentlyContinue |
        ForEach-Object { $_.BaseName -replace '^bonjour-depuis-', '' } | Where-Object { $_ -ne $hostName })
    if ($others.Count -gt 0) { Line $true "other machines $($others.Count): $($others -join ', ')" }
    else { Line $false 'other machines none yet -- normal right after the first setup; wait a few minutes' }
    # A greeting proves a path exists; a whole 50 MB file proves it carries data.
    $big = @(Get-ChildItem -LiteralPath $folder -Filter '50Mo-depuis-*.bin' -ErrorAction SilentlyContinue |
        Where-Object { $_.BaseName -ne "50Mo-depuis-$hostName" -and $_.Length -eq 52428800 })
    if ($big.Count -gt 0) { Line $true "50 MB files    $($big.Count) complete from other machines" }
    else { Line $false '50 MB files    none complete yet -- they follow the greetings' }
    Write-Host ""
    Write-Host "  Details: `$env:ITSANAS_HOME='$nodeHome'; & '$bin' doctor"
    Write-Host "  Your test folder: $folder"; Write-Host ""
    if ($script:failures -eq 0) { Write-Host '  ==> IT WORKS' -ForegroundColor Green; Write-Host ""; exit 0 }
    Write-Host "  ==> NOT YET ($script:failures line(s) above)" -ForegroundColor Red; Write-Host ""; exit 1
}

if ($Status) { Show-Status }

if (-not $Coordinator -or -not $CoordinatorDevice) {
    Die '-Coordinator and -CoordinatorDevice are both needed' @(
        'They are in your fleet notes. The coordinator prints its id with',
        '  itsanas-coordinator --identity')
}

$provision = Join-Path $PSScriptRoot 'provision.ps1'
if (-not (Test-Path -LiteralPath $provision)) {
    Die 'provision.ps1 is not next to this script' @('Run it from a checkout: git clone https://github.com/SigSegGit/itsanas')
}

if ($Fresh -and (Test-Path -LiteralPath $nodeHome)) {
    Write-Host ""; Write-Host '==> Archiving the earlier test bed'
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    $archive = Join-Path $env:USERPROFILE ("itsanas-archive-" + (Get-Date -Format 'yyyy-MM-dd-HHmmss'))
    New-Item -ItemType Directory -Path $archive | Out-Null
    foreach ($path in @($nodeHome, $folder, $secretFile)) {
        if (Test-Path -LiteralPath $path) { Move-Item -LiteralPath $path -Destination $archive }
    }
    Write-Host "  moved to $archive (delete it yourself once the new bed works)"
}

# The passphrase: reused if this machine has one, so a re-run does not lock
# the node out of its own keystore; otherwise drawn at random, once.
if (Test-Path -LiteralPath $secretFile) {
    $env:ITSANAS_PASSPHRASE = (Get-Content -LiteralPath $secretFile -Raw).Trim()
} elseif (Test-Path -LiteralPath (Join-Path $nodeHome 'keystore.bin')) {
    Die 'a test node exists but its passphrase file is gone' @('Start over with -Fresh (the old node is archived, not deleted).')
} else {
    $bytes = New-Object byte[] 24
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    $env:ITSANAS_PASSPHRASE = -join ($bytes | ForEach-Object { $_.ToString('x2') })
}

$tempPhraseFile = $null
try {
    $arguments = @{
        Instance = $instance; Username = 'essai'; Pledge = '10G'; Keep = '3G'; Folder = $folder
        Coordinator = $Coordinator; CoordinatorDevice = $CoordinatorDevice
    }
    if ($Invite) { $arguments.Invite = $Invite }
    elseif (Test-Path -LiteralPath (Join-Path $nodeHome 'keystore.bin')) { }
    elseif ($PhraseFile) {
        if (-not (Test-Path -LiteralPath $PhraseFile)) { Die "cannot read $PhraseFile" }
        if (@((Get-Content -LiteralPath $PhraseFile -Raw).Trim() -split '\s+').Count -ne 24) { Die "$PhraseFile does not hold 24 words" }
        # Not $tempPhraseFile: `finally` removes only the file this script made.
        # PowerShell names are case-insensitive: the temporary file must not be
        # called $phraseFile, or it erases the -PhraseFile parameter.
        $arguments.PhraseFile = (Resolve-Path -LiteralPath $PhraseFile).Path
    } else {
        Write-Host ""; Write-Host '==> Joining the test account'
        $secure = Read-Host '  Paste the 24 words the first machine printed (hidden)' -AsSecureString
        $words = [System.Net.NetworkCredential]::new('', $secure).Password
        if (@($words.Trim() -split '\s+').Count -ne 24) { Die 'that is not 24 words' @('Copy all of them, in order.') }
        # A file only this account can read, removed in `finally`.
        $tempPhraseFile = [System.IO.Path]::GetTempFileName()
        $acl = New-Object System.Security.AccessControl.FileSecurity
        $acl.SetAccessRuleProtection($true, $false)
        $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule(
            [System.Security.Principal.WindowsIdentity]::GetCurrent().Name, 'FullControl', 'Allow')))
        Set-Acl -LiteralPath $tempPhraseFile -AclObject $acl
        Set-Content -LiteralPath $tempPhraseFile -Value $words -NoNewline
        $words = $null
        $arguments.PhraseFile = $tempPhraseFile
    }

    Write-Host ""; Write-Host '==> Installing and setting up (provision.ps1)'
    & powershell -NoProfile -ExecutionPolicy Bypass -File $provision @arguments
    if ($LASTEXITCODE -ne 0) { Die 'provisioning failed' @('Its output is above.') }
} finally {
    if ($tempPhraseFile) { Remove-Item -LiteralPath $tempPhraseFile -Force -ErrorAction SilentlyContinue }
}

Write-Host ""; Write-Host "==> Dropping this machine's files into the test folder"
New-Item -ItemType Directory -Force -Path $folder | Out-Null
Set-Content -LiteralPath (Join-Path $folder "bonjour-depuis-$hostName.txt") -Value "Bonjour depuis $hostName (Windows), $(Get-Date)"
$big = Join-Path $folder "50Mo-depuis-$hostName.bin"
if (-not (Test-Path -LiteralPath $big)) {
    $buffer = New-Object byte[] (1MB)
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $stream = [System.IO.File]::Create($big)
    try { for ($i = 0; $i -lt 50; $i++) { $rng.GetBytes($buffer); $stream.Write($buffer, 0, $buffer.Length) } }
    finally { $stream.Dispose() }
}
Write-Host "  bonjour-depuis-$hostName.txt and 50Mo-depuis-$hostName.bin"
Start-Sleep -Seconds 5
Show-Status
