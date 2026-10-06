# A notification-area icon for one ITSaNAS node on Windows.
#
#   powershell -ExecutionPolicy Bypass -WindowStyle Hidden -File scripts\itsanas-tray.ps1 [-Instance NAME]
#
# install/provision.ps1 copies this beside itsanas.exe and starts it at each
# logon, one Startup-folder shortcut per node; clean.ps1 removes it.
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
# Left click opens the synced folder. Right click: open the folder, open the
# daemon's log, pause or resume syncing, sync now, how often to sync, restart
# the daemon's task, quit this icon. Pause, sync now and the interval are
# `itsanas pause|resume|sync-now|interval`, which write a file the running
# daemon reads (crates/itsanas-cli/src/control.rs); this script holds no logic
# of its own. Disconnect and decommission are not here yet: each needs the
# confirmation Nicolas specified, and nothing half-done belongs in a menu that
# stops a daemon.

param([string]$Instance = '')

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$suffix = if ($Instance) { "-$Instance" } else { '' }
$taskName = "ITSaNAS$suffix"
$log = Join-Path $env:LOCALAPPDATA "itsanas\daemon$suffix.log"
$exe = Join-Path $env:LOCALAPPDATA 'Programs\itsanas\bin\itsanas.exe'
if (-not (Test-Path $exe)) { $exe = 'itsanas' }
$label = if ($Instance) { "ITSaNAS $Instance" } else { 'ITSaNAS' }

function Invoke-Itsanas([string[]]$Arguments) {
    $all = @()
    if ($Instance) { $all += @('--instance', $Instance) }
    $all += $Arguments
    try { return (& $exe @all 2>$null) } catch { return $null }
}

# The folder, from `itsanas instances`: "NAME: account A, home H, folder F reachable, ...".
function Get-SyncedFolder {
    $name = if ($Instance) { $Instance } else { '(unnamed)' }
    $line = @(Invoke-Itsanas @('instances')) | Where-Object { $_ -like "${name}:*" } | Select-Object -First 1
    if ($line -and $line -match ', folder (.+?) (reachable|UNREACHABLE),') { return $Matches[1] }
    return $null
}

function Get-State {
    $brief = @(Invoke-Itsanas @('status', '--brief')) | Select-Object -First 1
    if (-not $brief) { return @('unknown', $null) }
    $parts = "$brief".Trim() -split ' '
    $age = $null
    if ($parts.Count -gt 1) { $age = [int64]$parts[1] }
    return @($parts[0], $age)
}

function Format-Age($seconds) {
    if ($null -eq $seconds) { return '' }
    if ($seconds -lt 120) { return " ($seconds s ago)" }
    if ($seconds -lt 7200) { return " ($([int]($seconds / 60)) min ago)" }
    return " ($([int]($seconds / 3600)) h ago)"
}

$icon = New-Object System.Windows.Forms.NotifyIcon
$icon.Visible = $true

function Update-Icon {
    $state, $age = Get-State
    switch ($state) {
        'healthy' { $icon.Icon = [System.Drawing.SystemIcons]::Information }
        'paused' { $icon.Icon = [System.Drawing.SystemIcons]::Shield }
        'stopped' { $icon.Icon = [System.Drawing.SystemIcons]::Error }
        'departed' { $icon.Icon = [System.Drawing.SystemIcons]::Error }
        default { $icon.Icon = [System.Drawing.SystemIcons]::Warning }
    }
    # A tooltip is capped at 63 characters by Windows.
    if ($pauseItem) { $pauseItem.Text = if ($state -eq 'paused') { 'Resume syncing' } else { 'Pause syncing' } }
    $text = "${label}: $state$(Format-Age $age)"
    if ($text.Length -gt 63) { $text = $text.Substring(0, 63) }
    $icon.Text = $text
}

function Open-Folder {
    $folder = Get-SyncedFolder
    if ($folder -and (Test-Path $folder)) {
        Start-Process explorer.exe $folder
    } else {
        [System.Windows.Forms.MessageBox]::Show(
            "No synced folder is set for $label, or it is not reachable. Set one with: itsanas folder <path>",
            $label) | Out-Null
    }
}

$menu = New-Object System.Windows.Forms.ContextMenuStrip
$menu.Items.Add('Open the synced folder', $null, { Open-Folder }) | Out-Null
$menu.Items.Add('Open the log', $null, {
    if (Test-Path $log) { Start-Process notepad.exe $log } else {
        [System.Windows.Forms.MessageBox]::Show("No log yet at $log", $label) | Out-Null
    }
}) | Out-Null
# Pause states its consequences first, as specified (HANDOVER §8 f): it is
# the one entry here a person can forget they chose.
$pauseItem = $menu.Items.Add('Pause syncing', $null, {
    $state, $age = Get-State
    if ($state -eq 'paused') {
        Invoke-Itsanas @('resume') | Out-Null
    } else {
        $answer = [System.Windows.Forms.MessageBox]::Show(
            "Pause syncing on this machine?`n`nNothing is lost: what you change here waits until you resume, and what your other machines change waits for you. This machine keeps hosting for the others.",
            $label, [System.Windows.Forms.MessageBoxButtons]::OKCancel)
        if ($answer -eq [System.Windows.Forms.DialogResult]::OK) { Invoke-Itsanas @('pause') | Out-Null }
    }
    Update-Icon
})
$menu.Items.Add('Sync now', $null, {
    $said = @(Invoke-Itsanas @('sync-now'))
    if (-not $said) {
        [System.Windows.Forms.MessageBox]::Show(
            "Not asked: syncing is paused, or no daemon is running for $label.", $label) | Out-Null
    }
}) | Out-Null
$every = New-Object System.Windows.Forms.ToolStripMenuItem('Sync every')
foreach ($choice in @(@('1 minute', '1m'), @('5 minutes', '5m'), @('15 minutes', '15m'),
                      @('1 hour', '1h'), @('Automatic', 'auto'))) {
    # The value rides on the item: GetNewClosure() would capture it, but would
    # also hide this script's functions from the handler.
    $item = $every.DropDownItems.Add($choice[0], $null, {
        param($sender, $click)
        Invoke-Itsanas @('interval', $sender.Tag) | Out-Null
    })
    $item.Tag = $choice[1]
}
$menu.Items.Add($every) | Out-Null
$menu.Items.Add('-') | Out-Null
$menu.Items.Add('Restart the daemon', $null, {
    try {
        Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        Start-ScheduledTask -TaskName $taskName
    } catch {
        [System.Windows.Forms.MessageBox]::Show("No scheduled task $taskName to restart.", $label) | Out-Null
    }
    Update-Icon
}) | Out-Null
$menu.Items.Add('-') | Out-Null
$menu.Items.Add('Quit this icon (the daemon keeps running)', $null, {
    $icon.Visible = $false
    [System.Windows.Forms.Application]::Exit()
}) | Out-Null
$icon.ContextMenuStrip = $menu

$icon.add_MouseClick({
    param($source, $click)
    if ($click.Button -eq [System.Windows.Forms.MouseButtons]::Left) { Open-Folder }
})

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 30000
$timer.add_Tick({ Update-Icon })
$timer.Start()
Update-Icon

[System.Windows.Forms.Application]::Run()
$icon.Dispose()
