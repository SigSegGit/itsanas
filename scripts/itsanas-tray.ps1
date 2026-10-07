# A notification-area icon for one ITSaNAS node on Windows.
#
#   powershell -ExecutionPolicy Bypass -WindowStyle Hidden -File scripts\itsanas-tray.ps1 [-Instance NAME]
#
# install/provision.ps1 copies this beside itsanas.exe and starts it at each
# logon, one Startup-folder shortcut per node; clean.ps1 removes it. Its
# siblings draw the same menu elsewhere: scripts/itsanas-menubar.js (macOS)
# and scripts/itsanas-tray.py (Linux desktops).
#
# It shows what `itsanas status --brief` says and nothing else: healthy only
# while a daemon holds the store AND wrote a snapshot within two intervals.
# That rule lives in Rust, tested (red_team_a_silent_daemon_is_stale_never_healthy);
# this script only draws it. Why PowerShell and not a Rust crate: the tray
# crates (tray-icon, winit) pull GTK and Wayland dependencies that cargo deny
# rejects -- it judges every dependency edge on every target, so building
# them for Windows only does not help -- and calling Shell_NotifyIcon
# directly needs unsafe, which this workspace allows in one file. Windows
# ships NotifyIcon; nothing is added. (HANDOVER §8 f.)
#
# The menu is built from Get-Menu, a plain function of what the CLI said, so
# that `-Describe 'paused 60'` prints it without opening anything:
# scripts/check-installers.sh compares that text with the two other trays',
# state by state, which is how "the same menu everywhere" is held. Every entry
# runs `itsanas [--instance NAME] ...`; no logic of its own beyond drawing.
#
# Never wait on a long command here: this is the UI thread, and a frozen icon
# reads as a dead one. `itsanas settings` serves a page for as long as it is
# open, so it starts hidden and is left alone; `signout` starts hidden and is
# watched by a timer; `signin` asks for the passphrase, so it gets a window.

param(
    [string]$Instance = '',
    # Print the menu for this `status --brief` line and exit, opening nothing.
    [string]$Describe = '',
    # With -Describe: what `itsanas interval` would have said.
    [string]$Interval = ''
)

$ErrorActionPreference = 'Stop'

$suffix = if ($Instance) { "-$Instance" } else { '' }
$taskName = "ITSaNAS$suffix"
$label = if ($Instance) { "ITSaNAS $Instance" } else { 'ITSaNAS' }

# The words of the two dialogs, the same on every platform (the other trays
# carry the same text): what a person loses, and what they keep.
$pauseWords = 'Nothing is lost: what you change here waits until syncing resumes, and what your other machines change waits for you. This machine keeps hosting for the others.'
$signoutWords = "This stops ITSaNAS on this machine and forgets the passphrase here, so it no longer starts by itself.`n`nYour files, and the data this machine keeps for other people, stay on this disk. While signed out it does not sync, and the others cannot check what it keeps for them.`n`nTo come back, choose Sign in... and type your passphrase."

# ---------------------------------------------------------------- the model

function Format-Age($seconds) {
    if ($null -eq $seconds) { return '' }
    if ($seconds -lt 120) { return " ($seconds s ago)" }
    if ($seconds -lt 7200) { return " ($([math]::Floor($seconds / 60)) min ago)" }
    return " ($([math]::Floor($seconds / 3600)) h ago)"
}

# `status --brief` is "WORD [AGE]"; anything else is unknown.
function Split-Brief([string]$Brief) {
    $parts = "$Brief".Trim() -split '\s+'
    if (-not $parts[0]) { return @('unknown', $null) }
    $age = $null
    if ($parts.Count -gt 1 -and $parts[1] -match '^\d+$') { $age = [int64]$parts[1] }
    return @($parts[0], $age)
}

function Get-Colour([string]$State) {
    switch ($State) {
        'healthy' { return 'green' }
        'paused' { return 'blue' }
        'stopped' { return 'red' }
        'departed' { return 'red' }
        'signed-out' { return 'grey' }
        default { return 'orange' }
    }
}

# `itsanas interval` says "every 5 min (set with ...)" or "auto: ...".
function Get-CurrentEvery([string]$Line) {
    if ($Line -match '^every (.+?) \(') { return $Matches[1] }
    if ($Line -like 'auto*') { return 'auto' }
    return ''
}

function New-Entry([string]$Kind, [string]$Text, $Action, [switch]$Confirm, [switch]$Checked) {
    return @{ Kind = $Kind; Text = $Text; Action = $Action; Confirm = [bool]$Confirm; Checked = [bool]$Checked; Children = @() }
}

function Get-Menu([string]$Brief, [string]$IntervalLine) {
    $state, $age = Split-Brief $Brief
    $menu = @()
    $menu += New-Entry 'item' 'Open the synced folder' 'open-folder'
    $menu += New-Entry 'status' "${label}: $state$(Format-Age $age)" $null
    $menu += New-Entry 'sep' '' $null
    if ($state -eq 'signed-out') {
        $menu += New-Entry 'item' 'Sign in...' @('signin')
    } else {
        if ($state -eq 'paused') {
            $menu += New-Entry 'item' 'Resume syncing' @('resume')
        } else {
            $pause = New-Entry 'menu' 'Pause syncing' $null
            $pause.Children = @(
                (New-Entry 'item' 'For 1 hour' @('pause', '--for', '1h') -Confirm),
                (New-Entry 'item' 'For 8 hours' @('pause', '--for', '8h') -Confirm),
                (New-Entry 'item' 'Until I resume' @('pause') -Confirm))
            $menu += $pause
        }
        $menu += New-Entry 'item' 'Sync now' @('sync-now')
        $current = Get-CurrentEvery $IntervalLine
        $every = New-Entry 'menu' 'Sync every' $null
        foreach ($choice in @(@('1 min', '1m', '1 min'), @('5 min', '5m', '5 min'), @('15 min', '15m', '15 min'),
                              @('1 hour', '1h', '1 h'), @('Automatic', 'auto', 'auto'))) {
            $every.Children += New-Entry 'item' $choice[0] @('interval', $choice[1]) -Checked:($current -eq $choice[2])
        }
        $menu += $every
        $menu += New-Entry 'sep' '' $null
        $menu += New-Entry 'item' 'Settings...' @('settings')
        $menu += New-Entry 'item' 'Sign out...' @('signout') -Confirm
    }
    $menu += New-Entry 'sep' '' $null
    $menu += New-Entry 'item' 'Open the log' 'open-log'
    $menu += New-Entry 'item' 'Restart' 'restart'
    $menu += New-Entry 'item' 'Quit the icon' 'quit'
    return ,$menu
}

function Get-CliArguments([string[]]$Arguments) {
    $all = @()
    if ($Instance) { $all += @('--instance', $Instance) }
    return $all + $Arguments
}

# One line per entry, in the format the other trays print (check-installers.sh).
function Write-Menu($Entries, [string]$Indent) {
    foreach ($entry in $Entries) {
        switch ($entry.Kind) {
            'sep' { "$Indent---" }
            'status' { "${Indent}status $($entry.Text)" }
            'menu' { "${Indent}menu $($entry.Text)"; Write-Menu $entry.Children "$Indent  " }
            default {
                $action = if ($entry.Action -is [array]) { 'itsanas ' + ((Get-CliArguments $entry.Action) -join ' ') } else { $entry.Action }
                $flags = ''
                if ($entry.Checked) { $flags += ' [checked]' }
                if ($entry.Confirm) { $flags += ' [confirm]' }
                "${Indent}item $($entry.Text) -> $action$flags"
            }
        }
    }
}

if ($PSBoundParameters.ContainsKey('Describe')) {
    $state, $age = Split-Brief $Describe
    "icon $(Get-Colour $state)"
    Write-Menu (Get-Menu $Describe $Interval) ''
    exit 0
}

# ---------------------------------------------------------------- the icon

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$log = Join-Path $env:LOCALAPPDATA "itsanas\daemon$suffix.log"
$exe = Join-Path $PSScriptRoot 'itsanas.exe'
if (-not (Test-Path -LiteralPath $exe)) { $exe = Join-Path $env:LOCALAPPDATA 'Programs\itsanas\bin\itsanas.exe' }
if (-not (Test-Path -LiteralPath $exe)) { $exe = 'itsanas' }

function Invoke-Itsanas([string[]]$Arguments) {
    $all = Get-CliArguments $Arguments
    try { return (& $exe @all 2>$null) } catch { return $null }
}

# A filled circle, drawn once per colour: no image file to ship or lose.
$icons = @{}
foreach ($pair in @(@('green', 46, 160, 67), @('blue', 47, 111, 235), @('orange', 230, 140, 20),
                    @('red', 210, 45, 45), @('grey', 140, 140, 140))) {
    $bitmap = New-Object System.Drawing.Bitmap 16, 16
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, $pair[1], $pair[2], $pair[3]))
    $graphics.FillEllipse($brush, 1, 1, 14, 14)
    $graphics.Dispose(); $brush.Dispose()
    $icons[$pair[0]] = [System.Drawing.Icon]::FromHandle($bitmap.GetHicon())
}

# The folder, from `itsanas instances`: "NAME: account A, home H, folder F reachable, ...".
function Get-SyncedFolder {
    $name = if ($Instance) { $Instance } else { '(unnamed)' }
    $line = @(Invoke-Itsanas @('instances')) | Where-Object { $_ -like "${name}:*" } | Select-Object -First 1
    if ($line -and $line -match ', folder (.+?) (reachable|UNREACHABLE),') { return $Matches[1] }
    return $null
}

function Get-Brief { return "$(@(Invoke-Itsanas @('status', '--brief')) | Select-Object -First 1)" }

$icon = New-Object System.Windows.Forms.NotifyIcon
$icon.Visible = $true

function Update-Icon {
    $state, $age = Split-Brief (Get-Brief)
    $icon.Icon = $icons[(Get-Colour $state)]
    # A tooltip is capped at 63 characters by Windows.
    $text = "${label}: $state$(Format-Age $age)"
    if ($text.Length -gt 63) { $text = $text.Substring(0, 63) }
    $icon.Text = $text
}

function Show-Message([string]$Text) {
    [System.Windows.Forms.MessageBox]::Show($Text, $label) | Out-Null
}

function Open-Folder {
    $folder = Get-SyncedFolder
    if ($folder -and (Test-Path -LiteralPath $folder)) {
        Start-Process explorer.exe $folder
    } else {
        Show-Message "No synced folder is set for $label, or it is not reachable. Set one in Settings..."
    }
}

# Started hidden, its output kept, and watched by a timer: the menu stays
# responsive, and the person still learns what happened.
$script:watched = $null
function Start-Watched([string[]]$Arguments, [string]$What) {
    $out = [System.IO.Path]::GetTempFileName()
    $none = [System.IO.Path]::GetTempFileName()
    $process = Start-Process -FilePath $exe -ArgumentList (Get-CliArguments $Arguments) -WindowStyle Hidden `
        -RedirectStandardOutput $out -RedirectStandardError "$out.err" -RedirectStandardInput $none -PassThru
    $script:watched = @{ Process = $process; Out = $out; None = $none; What = $What }
}

function Test-Watched {
    $job = $script:watched
    if (-not $job -or -not $job.Process.HasExited) { return }
    $said = ((Get-Content -LiteralPath $job.Out, "$($job.Out).err" -ErrorAction SilentlyContinue) -join "`n").Trim()
    Remove-Item -LiteralPath $job.Out, "$($job.Out).err", $job.None -ErrorAction SilentlyContinue
    $script:watched = $null
    if ($job.Process.ExitCode -eq 0) { Show-Message "$($job.What) done.`n`n$said" } else { Show-Message "$($job.What) did not work:`n`n$said" }
    Update-Icon
}

# Sign in asks for the passphrase, so it runs where the person can type it.
function Start-InWindow([string[]]$Arguments) {
    $quoted = (@($exe) + (Get-CliArguments $Arguments) | ForEach-Object { "'" + ($_ -replace "'", "''") + "'" })
    $command = "& $($quoted -join ' '); Read-Host 'Press Enter to close this window'"
    # Encoded, because Windows PowerShell 5 joins -ArgumentList with spaces and
    # quotes nothing: a path with a space would split the command.
    $encoded = [Convert]::ToBase64String([System.Text.Encoding]::Unicode.GetBytes($command))
    Start-Process powershell.exe -ArgumentList @('-NoProfile', '-EncodedCommand', $encoded)
}

function Confirm-Entry($entry) {
    if (-not $entry.Confirm) { return $true }
    if ($entry.Action[0] -eq 'signout') {
        $question = "Sign out of $label on this machine?`n`n$signoutWords"
    } else {
        $lasts = switch ($entry.Text) { 'For 1 hour' { 'for 1 hour' } 'For 8 hours' { 'for 8 hours' } default { 'until you resume it' } }
        $question = "Pause syncing on this machine $lasts?`n`n$pauseWords"
    }
    $answer = [System.Windows.Forms.MessageBox]::Show($question, $label, [System.Windows.Forms.MessageBoxButtons]::OKCancel)
    return $answer -eq [System.Windows.Forms.DialogResult]::OK
}

function Invoke-Entry($entry) {
    if (-not (Confirm-Entry $entry)) { return }
    if ($entry.Action -is [array]) {
        switch ($entry.Action[0]) {
            'settings' { Start-Process -FilePath $exe -ArgumentList (Get-CliArguments @('settings')) -WindowStyle Hidden }
            'signin' { Start-InWindow @('signin') }
            'signout' { Start-Watched @('signout') 'Sign out' }
            'sync-now' {
                if (-not @(Invoke-Itsanas @('sync-now'))) { Show-Message "Not asked: syncing is paused, or no daemon is running for $label." }
            }
            default { Invoke-Itsanas $entry.Action | Out-Null }
        }
        Update-Icon
        return
    }
    switch ($entry.Action) {
        'open-folder' { Open-Folder }
        'open-log' { if (Test-Path -LiteralPath $log) { Start-Process notepad.exe $log } else { Show-Message "No log yet at $log" } }
        'restart' {
            try {
                Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
                Start-ScheduledTask -TaskName $taskName
            } catch { Show-Message "No scheduled task $taskName to restart." }
            Update-Icon
        }
        'quit' { $icon.Visible = $false; [System.Windows.Forms.Application]::Exit() }
    }
}

# The entry rides on the item's Tag: GetNewClosure() would capture it, but
# would also hide this script's functions from the handler.
function Add-Entries($Items, $Entries) {
    foreach ($entry in $Entries) {
        switch ($entry.Kind) {
            'sep' { $Items.Add('-') | Out-Null }
            'status' { $item = $Items.Add($entry.Text); $item.Enabled = $false }
            'menu' {
                $sub = New-Object System.Windows.Forms.ToolStripMenuItem($entry.Text)
                Add-Entries $sub.DropDownItems $entry.Children
                $Items.Add($sub) | Out-Null
            }
            default {
                $item = $Items.Add($entry.Text, $null, { param($sender, $click) Invoke-Entry $sender.Tag })
                $item.Tag = $entry
                $item.Checked = $entry.Checked
            }
        }
    }
}

# Built afresh each time it opens, so it says what is true now.
$menu = New-Object System.Windows.Forms.ContextMenuStrip
$menu.add_Opening({
    param($sender, $opening)
    $menu.Items.Clear()
    $intervalLine = "$(@(Invoke-Itsanas @('interval')) | Select-Object -First 1)"
    Add-Entries $menu.Items (Get-Menu (Get-Brief) $intervalLine)
    # An empty strip is cancelled before it opens; give it the entries first.
    $opening.Cancel = $false
})
$menu.Items.Add('...') | Out-Null
$icon.ContextMenuStrip = $menu

$icon.add_MouseClick({
    param($source, $click)
    if ($click.Button -eq [System.Windows.Forms.MouseButtons]::Left) { Open-Folder }
})

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 30000
$timer.add_Tick({ Update-Icon })
$timer.Start()
$watcher = New-Object System.Windows.Forms.Timer
$watcher.Interval = 1000
$watcher.add_Tick({ Test-Watched })
$watcher.Start()
Update-Icon

[System.Windows.Forms.Application]::Run()
$icon.Dispose()
