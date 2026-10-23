<#
.SYNOPSIS
    MVP acceptance tests on Windows: H (is the daemon cheap to leave running?),
    and B, E, F and C's plant, line for line as scripts/acceptance.sh writes them.

.DESCRIPTION
    `scripts/acceptance.sh` measures H through /proc, so it runs on the Pi and
    the VM -- where nobody asked. MVP.md's criterion is about the laptop:
    battery, CPU at idle, memory, and whether the machine still sleeps. This
    measures those, on the machine they are about, and writes the same receipt
    line as the shell kit to ~\.itsanas-receipts\acceptance.txt.

      acceptance.ps1 H schedule -- sample every five minutes, as this user
      acceptance.ps1 H sample -- one sample (what the schedule runs)
      acceptance.ps1 H report -- CPU and memory, after 24 hours
      acceptance.ps1 H sleep -- did the machine sleep, and did the daemon stop it or wake it?
      acceptance.ps1 H unschedule -- stop sampling

    **H passes only when `H report` and `H sleep` both pass and the battery
    report has been read.** `H report` alone is half of the criterion.

    CPU is averaged over the hours the machine was awake. Intervals longer than
    three sampling periods are left out -- the machine slept, or a sample was
    missed -- because the daemon's CPU time does not advance while the laptop
    sleeps and wall time does: on a laptop awake a quarter of the day, averaging
    over the whole day divided the figure by four and passed a daemon using
    fifteen percent of a core.

    Thresholds, stated so they can be argued with: under 200 MiB resident and
    under 5% of one core on average while awake, over a window of at least 24
    hours with at least four of them awake. That is this kit's reading of
    "indistinguishable from the daemon being stopped"; MVP.md records it as a
    reading decided before the result.

    Bytes written are reported, not judged. On Windows the process counter
    includes network I/O, so the figure is not comparable with the Linux kit's,
    which counts the disk alone.

    Battery is not judged here, because one day of one laptop measures the day
    as much as the daemon. `H report` writes a powercfg battery report to read
    beside the verdict.

    B, E, F and C plant are the shell kit's phases, for a laptop or a partner on
    Windows (docs/TEST-PARTENAIRE.md). Same file names, same digests, same
    8-character codes, so a file written here is checked by the shell kit on a
    Mac and the other way round:

      acceptance.ps1 B write <folder> -- name, sha256, code
      acceptance.ps1 B check <folder> <code> -- or <name> <sha256>
      acceptance.ps1 E write|check ... -- as B
      acceptance.ps1 F delete <folder> <name>
      acceptance.ps1 F check <folder> <name>
      acceptance.ps1 C plant <folder>
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)] [string] $Test = '',
    [Parameter(Position = 1)] [string] $Phase = '',
    [Parameter(Position = 2, ValueFromRemainingArguments = $true)] [string[]] $Rest = @()
)

$ErrorActionPreference = 'Stop'

$receipts = if ($env:ITSANAS_RECEIPTS) { $env:ITSANAS_RECEIPTS } else { Join-Path $env:USERPROFILE '.itsanas-receipts' }
$samples = Join-Path $receipts 'h-samples-windows.tsv'
$taskName = 'ITSaNAS-acceptance-H'

# A gap longer than this between samples means the machine slept or the task
# did not run; the interval is not counted as awake time.
$periodSeconds = 300
$gapSeconds = 3 * $periodSeconds

function Verdict([string] $Result, [string] $Phase, [string] $Detail) {
    Verdict-Of $Result 'H' $Phase $Detail
}

# The shell kit's line, `verdict` in acceptance.sh, for any test.
function Verdict-Of([string] $Result, [string] $Of, [string] $Phase, [string] $Detail) {
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $machine = if ($env:COMPUTERNAME) { $env:COMPUTERNAME } else { [System.Net.Dns]::GetHostName() }
    $line = '{0,-4}  {1} {2,-6}  {3}  [{4} {5}]' -f $Result, $Of, $Phase, $Detail, $machine, $stamp
    Write-Output $line
    New-Item -ItemType Directory -Force -Path $receipts | Out-Null
    Add-Content -LiteralPath (Join-Path $receipts 'acceptance.txt') -Value $line -Encoding utf8
    if ($Result -ne 'PASS') { exit 1 }
}

function Usage {
    Write-Output 'usage: acceptance.ps1 H schedule|sample|report|sleep|unschedule'
    Write-Output '       acceptance.ps1 B|E write <folder>  |  B|E check <folder> <code>  (or <name> <sha256>)'
    Write-Output '       acceptance.ps1 F delete|check <folder> <name>  |  C plant <folder>'
    exit 2
}

# ------------------------------------------------- B, E, F, C as the shell kit

function Sha-Of([string] $Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
# acceptance.sh's code_of: the first eight hex digits, as two groups of four.
function Code-Of([string] $Sha) { '{0}-{1}' -f $Sha.Substring(0, 4), $Sha.Substring(4, 4) }

function Need-Dir([string] $Folder) {
    if (-not (Test-Path -LiteralPath $Folder -PathType Container)) {
        Write-Output "no such directory: $Folder"
        exit 2
    }
}

function Write-Phase([string] $Of, [string] $Folder) {
    Need-Dir $Folder
    $name = 'acceptance-{0}-{1}-{2}.bin' -f $Of, (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ'), $PID
    # A mebibyte and a little, as the shell kit: several chunks, not one.
    $bytes = New-Object byte[] (1024 * 1024 + 4099)
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    $path = Join-Path $Folder $name
    [System.IO.File]::WriteAllBytes($path, $bytes)
    $sha = Sha-Of $path
    Verdict-Of 'PASS' $Of 'write' "$name sha256 $sha code $(Code-Of $sha)"
}

function Check-Phase([string] $Of, [string] $Folder, [string] $Name, [string] $Want) {
    $path = Join-Path $Folder $Name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        Verdict-Of 'FAIL' $Of 'check' "$Name is not in $Folder"
    }
    $got = Sha-Of $path
    if ($got -eq $Want.ToLowerInvariant()) {
        Verdict-Of 'PASS' $Of 'check' "$Name identical, sha256 $($got.Substring(0, 16))"
    } else {
        Verdict-Of 'FAIL' $Of 'check' "$Name differs: expected $($Want.Substring(0, [Math]::Min(16, $Want.Length))), found $($got.Substring(0, 16))"
    }
}

function Check-Code-Phase([string] $Of, [string] $Folder, [string] $Code) {
    Need-Dir $Folder
    $want = ($Code -replace '[\s-]', '').ToLowerInvariant()
    if ($want -notmatch '^[0-9a-f]{8}$') {
        Verdict-Of 'FAIL' $Of 'check' "'$Code' is not a code: eight characters, 0-9 and a-f, as write printed them"
    }
    foreach ($file in Get-ChildItem -LiteralPath $Folder -File -Filter "acceptance-$Of-*.bin") {
        if ((Sha-Of $file.FullName).StartsWith($want)) {
            Verdict-Of 'PASS' $Of 'check' "$($file.Name) identical, code $(Code-Of $want)"
            return
        }
    }
    Verdict-Of 'FAIL' $Of 'check' "no file of test $Of in $Folder has code $(Code-Of $want): not here yet, or it arrived different"
}

function F-Delete([string] $Folder, [string] $Name) {
    $path = Join-Path $Folder $Name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { Verdict-Of 'FAIL' 'F' 'delete' "$Name is not in $Folder to delete" }
    Remove-Item -LiteralPath $path -Force
    Verdict-Of 'PASS' 'F' 'delete' "$Name removed from $Folder"
}

function F-Check([string] $Folder, [string] $Name) {
    Need-Dir $Folder
    if (Test-Path -LiteralPath (Join-Path $Folder $Name)) { Verdict-Of 'FAIL' 'F' 'check' "$Name is still in $Folder (or came back)" }
    $left = @(Get-ChildItem -LiteralPath $Folder -File -Recurse).Count
    Verdict-Of 'PASS' 'F' 'check' "$Name absent; $left other files still here. Run again after the next round: it must stay absent"
}

function C-Plant([string] $Folder) {
    Need-Dir $Folder
    $random = New-Object byte[] 12
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($random)
    $canary = 'ITSANAS-CANARY-' + (($random | ForEach-Object { $_.ToString('x2') }) -join '')
    $name = "$canary.txt"
    # Byte for byte what the shell kit writes, LF and no BOM: scripts/fleet/run.sh
    # works the canary file's digest out from the canary alone.
    [System.IO.File]::WriteAllText((Join-Path $Folder $name), "A private note.`n$canary`n", (New-Object System.Text.UTF8Encoding $false))
    Verdict-Of 'PASS' 'C' 'plant' "$name carries $canary"
}

function Sample {
    # The daemon, not any itsanas.exe: `itsanas status` run by hand is one too.
    $daemons = @(Get-CimInstance Win32_Process -Filter "Name = 'itsanas.exe'" |
        Where-Object { $_.CommandLine -match '\sdaemon(\s|$)' })
    if ($daemons.Count -eq 0) { Verdict 'FAIL' 'sample' 'no itsanas daemon is running' }
    # With two accounts on the machine there are two daemons. Picking the first
    # measured one at random, and the other's restart would read as a pid
    # change of this one. H is about one node; say so instead of guessing.
    if ($daemons.Count -gt 1) {
        Verdict 'FAIL' 'sample' "$($daemons.Count) itsanas daemons are running (one per account); H measures one node: stop the others for the measurement"
    }
    $daemon = $daemons[0]
    $process = Get-Process -Id $daemon.ProcessId
    # The version measured, so a day of samples taken before an upgrade is not
    # read as a verdict on the code after it.
    # The binary's modification time beside it, because every build so far
    # reports `itsanas 0.1.0`: the version string alone cannot tell the build
    # before an upgrade from the one after, which is the case this is for.
    $version = ''
    try { $version = ((& $daemon.ExecutablePath --version) -join ' ').Trim() } catch { $version = 'unknown' }
    try {
        $built = (Get-Item -LiteralPath $daemon.ExecutablePath).LastWriteTimeUtc.ToString('yyyy-MM-ddTHH:mmZ')
        $version = "$version built $built"
    } catch { }
    # The criterion is "memory under 200 MiB **with a large folder**", and
    # until 2026-09-16 no kit recorded the folder -- so a PASS on an empty
    # account was indistinguishable from a PASS on a full one, and MVP.md's
    # own figures came from about a megabyte. `itsanas status` answers without
    # a passphrase while the daemon holds the node, so the sampler can simply
    # ask. Unknown rather than absent when it cannot: a blank column would be
    # read as zero.
    $files = 'unknown'
    try {
        $state = & $daemon.ExecutablePath status 2>&1
        $line = $state | Select-String -Pattern '^\s*files\s+(\d+)' | Select-Object -First 1
        if ($line) { $files = $line.Matches[0].Groups[1].Value }
    } catch { }

    $unix = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $cpu = [math]::Round($process.TotalProcessorTime.TotalSeconds, 2).ToString([Globalization.CultureInfo]::InvariantCulture)
    $rss = [int64]($process.WorkingSet64 / 1KB)
    $written = [int64]$daemon.WriteTransferCount
    New-Item -ItemType Directory -Force -Path $receipts | Out-Null
    if (-not (Test-Path -LiteralPath $samples)) {
        "unix`tpid`tcpu_s`trss_kib`twritten_b`tfiles`tversion" | Set-Content -LiteralPath $samples -Encoding utf8
    }
    "$unix`t$($daemon.ProcessId)`t$cpu`t$rss`t$written`t$files`t$version" | Add-Content -LiteralPath $samples -Encoding utf8
    Write-Output "sampled pid $($daemon.ProcessId) ($version): cpu ${cpu}s total, rss ${rss} KiB, written ${written} B, account holds $files file(s)"
}

function Report {
    if (-not (Test-Path -LiteralPath $samples)) { Verdict 'FAIL' 'report' "no samples in $samples" }
    # @(), because Windows PowerShell hands back a single object rather than a
    # list when the file has one row, and `.Count` of that is empty: the first
    # run printed "only  sample(s)" with no number in it.
    $rows = @(Import-Csv -LiteralPath $samples -Delimiter "`t")
    if ($rows.Count -lt 2) { Verdict 'FAIL' 'report' "only $($rows.Count) sample(s) in $samples" }

    $inv = [Globalization.CultureInfo]::InvariantCulture
    # CPU and writes are cumulative per process, so a restart resets them.
    # Deltas are summed only between consecutive samples of the same pid, and
    # only over intervals short enough to be awake time.
    $cpu = 0.0; $written = 0.0; $awake = 0.0; $restarts = 0; $gaps = 0; $peak = [int64]$rows[0].rss_kib
    for ($i = 1; $i -lt $rows.Count; $i++) {
        $a = $rows[$i - 1]; $b = $rows[$i]
        $peak = [math]::Max($peak, [int64]$b.rss_kib)
        if ($a.pid -ne $b.pid) { $restarts++; continue }
        $interval = [double]::Parse($b.unix, $inv) - [double]::Parse($a.unix, $inv)
        if ($interval -gt $gapSeconds) { $gaps++; continue }
        $awake += $interval
        $cpu += [double]::Parse($b.cpu_s, $inv) - [double]::Parse($a.cpu_s, $inv)
        $written += [double]::Parse($b.written_b, $inv) - [double]::Parse($a.written_b, $inv)
    }
    $span = ([double]::Parse($rows[-1].unix, $inv) - [double]::Parse($rows[0].unix, $inv)) / 3600
    $awakeHours = $awake / 3600
    $avg = if ($awake -gt 0) { 100 * $cpu / $awake } else { 0 }
    $perDay = if ($awake -gt 0) { $written * 86400 / $awake / 1MB } else { 0 }
    $peakMiB = $peak / 1024
    $versions = @($rows | ForEach-Object { $_.version } | Where-Object { $_ } | Select-Object -Unique)
    $versionText = if ($versions.Count -eq 0) { 'version not recorded' } else { $versions -join ' then ' }

    # Reported, never judged: the criterion says "a large folder" without
    # saying how large, so a threshold here would be invented. What the receipt
    # must carry is what was actually measured, so that a PASS taken on an
    # empty account cannot later be read as evidence for a full one.
    $counts = @($rows | ForEach-Object { $_.files } | Where-Object { $_ -and $_ -ne 'unknown' } | ForEach-Object { [int]$_ })
    $filesText = if ($counts.Count -eq 0) {
        'account size NOT recorded, so this says nothing about "with a large folder"'
    } elseif (($counts | Measure-Object -Minimum).Minimum -eq ($counts | Measure-Object -Maximum).Maximum) {
        "on an account of $($counts[0]) file(s)"
    } else {
        "on an account of $(($counts | Measure-Object -Minimum).Minimum)-$(($counts | Measure-Object -Maximum).Maximum) file(s)"
    }

    $battery = Join-Path $receipts 'battery-report.html'
    powercfg /batteryreport /output $battery 2>&1 | Out-Null

    # Invariant culture: on a French Windows `-f` writes "0,0 h", and a receipt
    # from the laptop would then disagree with one from the Pi in the one thing
    # both are compared on.
    $detail = [string]::Format($inv,
        'CPU and memory half of H: {0} samples over {1:0.0} h, {2:0.0} h awake measured ({3} gap(s) left out as sleep or missed samples): cpu {4:0.00}% of a core while awake, peak {5:0.0} MiB {10}, {6:0.0} MiB/day written counting network (not comparable with the Linux kit), {7} restart(s), {8}; battery report {9}; sleep is H sleep',
        $rows.Count, $span, $awakeHours, $gaps, $avg, $peakMiB, $perDay, $restarts, $versionText, $battery, $filesText)
    if ($span -lt 24) { Verdict 'FAIL' 'report' "$detail -- a window shorter than the 24 hours the test asks for" }
    if ($awakeHours -lt 4) { Verdict 'FAIL' 'report' "$detail -- fewer than 4 hours awake, too little to average" }
    # Named separately: "over 200 MiB or 5% of a core" made the reader work out
    # which of the two it was, from a line that carries both numbers, on the one
    # morning this is read.
    if ($peakMiB -ge 200 -and $avg -ge 5) {
        Verdict 'FAIL' 'report' "$detail -- peak memory is over 200 MiB and CPU over 5% of a core"
    }
    if ($peakMiB -ge 200) { Verdict 'FAIL' 'report' "$detail -- peak memory is over the 200 MiB limit" }
    if ($avg -ge 5) { Verdict 'FAIL' 'report' "$detail -- CPU while awake is over the 5% of a core limit" }
    Verdict 'PASS' 'report' $detail
}

function Sleep-Check {
    $since = (Get-Date).AddDays(-1)
    $problems = @()

    # Proof the machine slept at all. Without it "no wake names itsanas" is true
    # of a laptop that never slept, and this check would pass on nothing -- the
    # first version did, on the machine it was written on. 42 is classic sleep,
    # 506 is entering Modern Standby, which most recent laptops use instead.
    $sleeps = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Kernel-Power'; Id = 42, 506; StartTime = $since } -ErrorAction SilentlyContinue)

    # Wakes from classic sleep name their source here; a wake the daemon
    # caused would name its process or a timer it set.
    $wakes = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Power-Troubleshooter'; Id = 1; StartTime = $since } -ErrorAction SilentlyContinue)
    $byItsanas = @($wakes | Where-Object { $_.Message -match 'itsanas' })
    if ($byItsanas.Count -gt 0) { $problems += "$($byItsanas.Count) of $($wakes.Count) wake(s) name itsanas" }

    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($admin) {
        # A snapshot: it says whether the daemon holds the machine awake at this
        # second, and a request held only during a sync round slips past it.
        # The sleep entries above are what show the machine does sleep.
        if (((powercfg /requests) -join "`n") -match 'itsanas') {
            $problems += 'powercfg /requests lists itsanas now: it is holding the machine awake'
        }
        $requests = 'power requests snapshot taken'
    } else {
        $requests = 'power requests NOT checked (needs an administrator PowerShell)'
    }

    $detail = "$($sleeps.Count) sleep entr(ies) and $($wakes.Count) wake(s) in 24 h, $($byItsanas.Count) naming itsanas; $requests"
    if ($problems.Count -gt 0) { Verdict 'FAIL' 'sleep' "$detail -- $($problems -join '; ')" }
    if ($sleeps.Count -eq 0) {
        Verdict 'FAIL' 'sleep' "$detail -- the machine did not sleep in the last 24 hours, so whether it sleeps normally is not shown; let it sleep with the daemon running and run this again"
    }
    if (-not $admin) { Verdict 'FAIL' 'sleep' "$detail -- run again elevated for a verdict" }
    Verdict 'PASS' 'sleep' $detail
}

function Schedule {
    $self = $PSCommandPath
    $action = New-ScheduledTaskAction -Execute 'conhost.exe' `
        -Argument "--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$self`" H sample"
    $trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
        -RepetitionInterval (New-TimeSpan -Seconds $periodSeconds) -RepetitionDuration (New-TimeSpan -Days 3)
    # WakeToRun stays off, which is the default and the point: a sampler that
    # woke the laptop would fail the test it is measuring.
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
    Write-Output "sampling every five minutes for three days as task $taskName; `acceptance.ps1 H report` after 24 hours"
}

switch ("$Test $Phase $($Rest.Count)") {
    { $_ -in 'B write 1', 'E write 1' } { Write-Phase $Test $Rest[0]; exit 0 }
    { $_ -in 'B check 3', 'E check 3', 'O check 3' } { Check-Phase $Test $Rest[0] $Rest[1] $Rest[2]; exit 0 }
    { $_ -in 'B check 2', 'E check 2', 'O check 2' } { Check-Code-Phase $Test $Rest[0] $Rest[1]; exit 0 }
    'F delete 2' { F-Delete $Rest[0] $Rest[1]; exit 0 }
    'F check 2' { F-Check $Rest[0] $Rest[1]; exit 0 }
    'C plant 1' { C-Plant $Rest[0]; exit 0 }
}
if ($Test -ne 'H') { Usage }
switch ($Phase) {
    'sample' { Sample }
    'report' { Report }
    'sleep' { Sleep-Check }
    'schedule' { Schedule }
    'unschedule' {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
        Write-Output "task $taskName removed; samples kept in $samples"
    }
    default { Usage }
}
