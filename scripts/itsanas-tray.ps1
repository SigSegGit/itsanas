# A notification-area icon for one ITSaNAS node on Windows.
#
#   powershell -ExecutionPolicy Bypass -WindowStyle Hidden -File scripts\itsanas-tray.ps1 [-Instance NAME]
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
# daemon's log, restart the daemon's task, quit this icon. Pause, disconnect
# and decommission are not here yet: each needs the confirmation Nicolas
# specified, and nothing half-done belongs in a menu that stops a daemon.

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
        'stopped' { $icon.Icon = [System.Drawing.SystemIcons]::Error }
        'departed' { $icon.Icon = [System.Drawing.SystemIcons]::Error }
        default { $icon.Icon = [System.Drawing.SystemIcons]::Warning }
    }
    # A tooltip is capped at 63 characters by Windows.
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
