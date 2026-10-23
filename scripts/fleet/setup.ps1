<#
.SYNOPSIS
    Put the fleet agent on this Windows machine (the laptop), once, as the
    user whose ITSaNAS nodes run here. The Windows half of setup.sh.

.DESCRIPTION
      powershell -ExecutionPolicy Bypass -File scripts\fleet\setup.ps1 "ssh-ed25519 AAAA... itsanas-fleet-ci"

    What it does, and nothing else:

    1. copies agent.ps1 and the acceptance kit to ~\.itsanas-fleet\ -- a copy,
       so a `git pull` of this checkout never changes what the key can run;
       run this again to update it;
    2. writes ~\.itsanas-fleet\agent.json from the nodes in the profile (the
       ~\.itsanas-NAME directories holding a keystore, as `itsanas instances`
       counts them): each one's account and home, and the test account's
       folder;
    3. adds the CI key to the file Win32-OpenSSH reads for this user behind
       `restrict,command=`, so that key runs the agent and cannot open a
       shell, forward a port or get a terminal. For a member of
       Administrators that file is C:\ProgramData\ssh\administrators_authorized_keys
       (sshd_config's `Match Group administrators`), which needs an elevated
       PowerShell and is left readable by Administrators and SYSTEM only;
       otherwise ~\.ssh\authorized_keys;
    4. asks the agent `hello` and `status test` as the key would, shows a
       refused `sh`, and prints this machine's host key for FLEET_KNOWN_HOSTS
       and what it found about sshd (installed, passwords, the firewall).

    It installs nothing and changes no system setting: OpenSSH Server,
    Tailscale and the firewall rule are Nicolas's to set (docs/FLEET-CI.md,
    "The laptop"). ASCII only: PowerShell 5.1 reads a file without a BOM as
    the ANSI code page.

    For check-fleet.sh, which runs this with pwsh on Linux:
    ITSANAS_FLEET_PROFILE replaces the profile, ITSANAS_FLEET_KEYS the keys file.
#>
param([string] $Key = '')

$ErrorActionPreference = 'Stop'

function Die([string] $Why) {
    Write-Output "error $Why"
    exit 1
}

# The same key shape as setup.sh, plus the base64 itself: it ends up inside
# a line sshd parses, so nothing but the key's own characters goes in.
if ($Key -cnotmatch '^ssh-ed25519 [A-Za-z0-9+/]+={0,2}( [A-Za-z0-9._@-]+)?$') {
    Write-Output 'usage: setup.ps1 "ssh-ed25519 AAAA... itsanas-fleet-ci"  (the public half of the CI key)'
    exit 2
}
$body = ($Key -split ' ')[1]

$onWindows = ($PSVersionTable.PSEdition -eq 'Desktop') -or $IsWindows
$userProfile = if ($env:ITSANAS_FLEET_PROFILE) { $env:ITSANAS_FLEET_PROFILE }
               elseif ($onWindows) { $env:USERPROFILE } else { $HOME }
$dest = Join-Path $userProfile '.itsanas-fleet'
$here = Split-Path -Parent $PSCommandPath
$kitSource = Join-Path (Split-Path -Parent $here) 'acceptance.ps1'

# 2 first, so a machine without a test node is refused before anything is
# written. A home is a directory holding a keystore (itsanas instances): not
# ~\.itsanas-passphrase, not a home emptied by `clean --purge-account`, and
# not ~\.itsanas-fleet, which this script writes.
$homes = [ordered] @{}
$testFolder = $null
$candidates = @(Get-ChildItem -LiteralPath $userProfile -Directory -Force -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ceq '.itsanas' -or $_.Name -clike '.itsanas-*' } | Sort-Object Name)
foreach ($dir in $candidates) {
    $config = Join-Path $dir.FullName 'config'
    if (-not (Test-Path -LiteralPath (Join-Path $dir.FullName 'keystore.bin') -PathType Leaf)) { continue }
    if (-not (Test-Path -LiteralPath $config -PathType Leaf)) { continue }
    $account = $null
    $folder = $null
    foreach ($line in Get-Content -LiteralPath $config) {
        if ($line -match '^username = (.+)$') { $account = $Matches[1].Trim() }
        if ($line -match '^folder = (.+)$') { $folder = $Matches[1].Trim() }
    }
    # agent.ps1 compares names whole and setup.sh skips the same ones.
    if ($account -cnotmatch '^[a-z0-9_]+$') { Write-Output "skipped $($dir.Name): account '$account' is not a plain name"; continue }
    if ($homes.Contains($account)) { Write-Output "skipped $($dir.Name): account $account is already $($homes[$account])"; continue }
    $homes[$account] = $dir.FullName
    if ($account -ceq 'test' -and $folder) {
        $testFolder = if ([System.IO.Path]::IsPathRooted($folder)) { $folder } else { Join-Path $dir.FullName $folder }
    }
}
if (-not $homes.Contains('test')) {
    Die "no node of the account test in $userProfile (a ~\.itsanas-NAME with a keystore whose account is test): add it first with install\provision.ps1 -Instance test -PhraseFile <the 24 words> -Folder $(Join-Path $userProfile 'ITSaNAS-test')"
}
if (-not $testFolder -or -not (Test-Path -LiteralPath $testFolder -PathType Container)) {
    Die "the test node at $($homes['test']) has no folder, or its folder '$testFolder' does not exist"
}

# 1.
New-Item -ItemType Directory -Force -Path (Join-Path $dest 'receipts') | Out-Null
Copy-Item -LiteralPath (Join-Path $here 'agent.ps1') -Destination (Join-Path $dest 'agent.ps1') -Force
Copy-Item -LiteralPath $kitSource -Destination (Join-Path $dest 'acceptance.ps1') -Force

$conf = [ordered] @{
    Folder   = $testFolder
    Kit      = (Join-Path $dest 'acceptance.ps1')
    Receipts = (Join-Path $dest 'receipts')
    Homes    = $homes
    MaxFiles = 50
}
# With a BOM: PowerShell 5.1 reads a file without one as the ANSI code page,
# and a profile path is not always ASCII.
[System.IO.File]::WriteAllText((Join-Path $dest 'agent.json'), ($conf | ConvertTo-Json), (New-Object System.Text.UTF8Encoding($true)))
Write-Output "agent.json: accounts $(@($homes.Keys) -join ' '), test folder $testFolder"

# 3.
$admin = $false
if ($onWindows) {
    # whoami /groups lists Administrators even in a non-elevated window (as a
    # deny-only group), which is the case sshd's Match Group sees. But the
    # file is the one sshd_config names, not the one we expect: a config
    # whose Match block is gone reads the user's file for everybody. No
    # sshd_config yet means sshd is not installed, and its default has it.
    $sshdConfig = Join-Path $env:ProgramData 'ssh\sshd_config'
    $matchAdmins = (-not (Test-Path -LiteralPath $sshdConfig)) -or
        [bool] ((Get-Content -LiteralPath $sshdConfig) -match '^\s*Match\s+Group\s+administrators\s*$')
    $admin = $matchAdmins -and [bool] ((whoami /groups) -match 'S-1-5-32-544')
}
$keys = if ($env:ITSANAS_FLEET_KEYS) { $env:ITSANAS_FLEET_KEYS }
        elseif ($admin) { Join-Path $env:ProgramData 'ssh\administrators_authorized_keys' }
        else { Join-Path (Join-Path $userProfile '.ssh') 'authorized_keys' }
if ($admin -and -not $env:ITSANAS_FLEET_KEYS) {
    $elevated = (New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $elevated) {
        Die "this account is an administrator, so sshd reads its keys from $keys, which only an elevated PowerShell can write: run this again as administrator (agent.json is written already)"
    }
}
$shell = if ($onWindows) { 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File' } else { 'pwsh -NoProfile -File' }
$agent = Join-Path $dest 'agent.ps1'
# The path quoted, with \" as sshd's options want it: a profile can be
# C:\Users\Nicolas Girard.
$line = "restrict,command=`"$shell \`"$agent\`"`" $Key"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $keys) | Out-Null
$kept = @()
if (Test-Path -LiteralPath $keys) {
    # A key already present is replaced, not added a second time with other
    # options; every other key stays as it was.
    $kept = @(Get-Content -LiteralPath $keys | Where-Object { -not $_.Contains($body) })
}
# No BOM: sshd would read it as part of the first key's options.
[System.IO.File]::WriteAllLines($keys, [string[]] ($kept + $line), (New-Object System.Text.UTF8Encoding($false)))
if ($admin -and -not $env:ITSANAS_FLEET_KEYS) {
    # sshd ignores this file unless only Administrators and SYSTEM can touch
    # it. SIDs, not names: the names are translated on a French Windows.
    & icacls.exe $keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null
    if ($LASTEXITCODE -ne 0) { Die "icacls could not restrict $keys; sshd will ignore it" }
}
Write-Output "the CI key may now run $agent and nothing else ($keys)"
if ($admin) {
    # An administrator's SSH logon is not filtered by UAC: the agent runs
    # with the full administrator token, which it never has on the Pi.
    Write-Output 'warn  this account is an administrator: the agent runs as one (docs/FLEET-CI.md, "The laptop, once")'
}

# 4. Native programs below may write to stderr, which 5.1 turns into a
# terminating error under Stop; what they say is shown, not thrown.
$ErrorActionPreference = 'Continue'
Write-Output ''
Write-Output 'as the key would see it:'
foreach ($request in 'hello', 'status test', 'sh') {
    $env:SSH_ORIGINAL_COMMAND = $request
    $env:ITSANAS_FLEET_CONF = ''
    $out = if ($onWindows) { & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $agent 2>&1 }
           else { & pwsh -NoProfile -File $agent 2>&1 }
    $out | ForEach-Object { Write-Output "  $_" }
}
Remove-Item Env:SSH_ORIGINAL_COMMAND -ErrorAction SilentlyContinue

if ($onWindows) {
    Write-Output ''
    $sshd = Get-Service -Name sshd -ErrorAction SilentlyContinue
    $hostKey = Join-Path $env:ProgramData 'ssh\ssh_host_ed25519_key.pub'
    if (-not $sshd) {
        Write-Output 'OpenSSH Server is not installed: nothing can reach this agent yet. To install it'
        Write-Output '(elevated): Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0;'
        Write-Output 'Set-Service sshd -StartupType Automatic; Start-Service sshd -- then run this again.'
    } else {
        Write-Output "sshd: $($sshd.Status), start $($sshd.StartType)"
        if (Test-Path -LiteralPath $hostKey) {
            Write-Output "this machine's host key, for FLEET_KNOWN_HOSTS (the name the runner's ssh config"
            Write-Output 'gives this machine, then the key):'
            $fields = (Get-Content -LiteralPath $hostKey -Raw).Trim() -split ' '
            Write-Output "  laptop $($fields[0]) $($fields[1])"
        }
        if ((Test-Path -LiteralPath $sshdConfig) -and -not ((Get-Content -LiteralPath $sshdConfig) -match '^\s*PasswordAuthentication\s+no')) {
            Write-Output "warn  $sshdConfig lets passwords in: add 'PasswordAuthentication no' and restart sshd"
        }
        $rule = Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue
        if ($rule) {
            $from = ($rule | Get-NetFirewallAddressFilter).RemoteAddress -join ','
            # Tailscale's IPv4 and IPv6 ranges, as Windows prints them back.
            $tailnet = '100.64.0.0/255.192.0.0', '100.64.0.0/10', 'fd7a:115c:a1e0::/48'
            if (@(($rule | Get-NetFirewallAddressFilter).RemoteAddress | Where-Object { $tailnet -notcontains $_ }).Count -gt 0) {
                Write-Output "warn  port 22 is open to $from, not only to Tailscale: Set-NetFirewallRule -Name OpenSSH-Server-In-TCP -RemoteAddress 100.64.0.0/10,fd7a:115c:a1e0::/48"
            }
        }
    }
}
