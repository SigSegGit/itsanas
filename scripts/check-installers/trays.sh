# Sourced by scripts/check-installers.sh, in its shell and in its order:
# the tray icons: per instance on Windows, one menu on every platform, autostart.

# ----------------------------------------- the tray icon, per instance, on Windows
#
# HANDOVER §8 0f: provision.ps1 starts scripts/itsanas-tray.ps1 at logon through
# a Startup shortcut per node, and clean.ps1 -Instance NAME removes that one.
# provision.ps1 cannot run here (it installs, registers a task, opens a store),
# so the two expressions that decide the shortcut -- its name and its
# arguments -- are read out of the script as written and evaluated for
# instances a, b and the default node. Then clean.ps1 -Instance b -Yes runs for
# real against a throwaway LOCALAPPDATA, USERPROFILE and Startup folder holding
# the three shortcuts. The failures this catches: an icon started without
# -Instance shows the default node's state under another account's name, and
# a clean that globs takes a sibling's icon away. Instance names are prefixed
# zz-check- so the task lookup clean.ps1 makes on a Windows machine cannot
# match a real node. What this cannot see: that the icon draws.

if command -v pwsh >/dev/null 2>&1; then
    tray=$(mktemp -d)
    trayroot=$tray
    command -v cygpath >/dev/null 2>&1 && trayroot=$(cygpath -w "$tray")
    cat > "$tray/check.ps1" <<'PS'
param([string]$Repo, [string]$Root)
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $Repo 'install/provision.ps1'), [ref]$tokens, [ref]$errors)
function Get-Right([string]$Name) {
    $found = @($ast.FindAll({ param($n)
        $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
        $n.Left.Extent.Text -eq "`$$Name" }, $true))
    if ($found.Count -ne 1) { throw "provision.ps1 assigns `$$Name $($found.Count) times, expected once" }
    return $found[0].Right.Extent.Text
}
$nameCode = Get-Right 'trayShortcutName'
$argsCode = Get-Right 'trayArguments'
$trayInstalled = 'C:\installed\itsanas-tray.ps1'
$failures = @()
$startup = Join-Path $Root 'startup'
New-Item -ItemType Directory -Force -Path $startup, (Join-Path $Root 'local'), (Join-Path $Root 'profile') | Out-Null
$names = @{}
foreach ($Instance in @('', 'zz-check-a', 'zz-check-b')) {
    $name = Invoke-Expression $nameCode
    $arguments = Invoke-Expression $argsCode
    $names[$Instance] = $name
    if ($arguments -notlike '--headless powershell.exe *') { $failures += "icon for '$Instance' is not started through conhost --headless: $arguments" }
    if ($arguments -notlike "* -File `"$trayInstalled`"*") { $failures += "icon for '$Instance' does not run the installed tray script: $arguments" }
    if ($Instance -and $arguments -notmatch " -Instance $([regex]::Escape($Instance))$") {
        $failures += "icon for instance $Instance is started without -Instance $Instance, so it shows another node: $arguments"
    }
    if (-not $Instance -and $arguments -match '-Instance') { $failures += "the default node's icon names an instance: $arguments" }
    Set-Content -LiteralPath (Join-Path $startup $name) -Value 'shortcut'
}
if (@($names.Values | Sort-Object -Unique).Count -ne 3) {
    $failures += "two nodes share one shortcut name, so one provision overwrites the other's icon: $($names.Values -join ', ')"
}
$env:LOCALAPPDATA = Join-Path $Root 'local'
$env:USERPROFILE = Join-Path $Root 'profile'
$env:ITSANAS_STARTUP_DIR = $startup
Remove-Item Env:\ITSANAS_HOME -ErrorAction SilentlyContinue
$out = & pwsh -NoProfile -File (Join-Path $Repo 'install/clean.ps1') -Instance zz-check-b -Yes 2>&1
if ($LASTEXITCODE -ne 0) { $failures += "clean.ps1 -Instance zz-check-b -Yes failed: $out" }
if (Test-Path -LiteralPath (Join-Path $startup $names['zz-check-b'])) {
    $failures += "clean.ps1 -Instance zz-check-b left its icon at logon ($($names['zz-check-b']))"
}
foreach ($kept in @('', 'zz-check-a')) {
    if (-not (Test-Path -LiteralPath (Join-Path $startup $names[$kept]))) {
        $failures += "clean.ps1 -Instance zz-check-b removed $($names[$kept]), a sibling's icon"
    }
}
$failures
PS
    repo=$(pwd)
    command -v cygpath >/dev/null 2>&1 && repo=$(cygpath -w "$repo")
    if ! trayout=$(pwsh -NoProfile -File "$trayroot/check.ps1" -Repo "$repo" -Root "$trayroot" 2>&1); then
        bad "the tray check could not run:"
        printf '%s\n' "$trayout" | sed 's/^/    /'
    elif [ -n "$trayout" ]; then
        printf '%s\n' "$trayout" | while IFS= read -r line; do bad "$line"; done
        failed=1
    else
        say "each node's tray icon starts with its own -Instance; clean.ps1 -Instance b removes only b's"
    fi
    rm -rf "$tray"
else
    say "pwsh is not here; the tray's logon shortcut was not checked"
fi

# --------------------------------- the tray icon on a Mac and a Linux desktop
#
# HANDOVER §8 0w (5). Three trays draw one menu: scripts/itsanas-tray.ps1
# (Windows), scripts/itsanas-menubar.js (macOS, JXA) and scripts/itsanas-tray.py
# (Linux desktops). Each prints its menu for a `status --brief` line without
# opening anything, and the three texts must be identical, state by state: a
# Sign out that confirms on one platform and not on another, or a Pause that
# lasts an hour here and forever there, is a person told one thing and given
# another. Then scripts/tray-autostart.sh writes and removes the login
# autostart in a throwaway HOME (launchctl and systemctl are loggers), and
# clean.sh --instance b must take b's icon and leave the default node's.
# What this cannot see: that the icon draws -- no Mac and no Linux desktop here.

PY=$(command -v python3 || command -v python || true)
if [ -n "$PY" ]; then
    if "$PY" -m py_compile scripts/itsanas-tray.py 2>/dev/null; then
        say "scripts/itsanas-tray.py compiles"
    else
        bad "scripts/itsanas-tray.py does not compile"
    fi
    rm -rf scripts/__pycache__
fi
if command -v node >/dev/null 2>&1; then
    if node --check scripts/itsanas-menubar.js 2>/dev/null; then
        say "scripts/itsanas-menubar.js parses as JavaScript"
    else
        bad "scripts/itsanas-menubar.js does not parse as JavaScript"
    fi
fi
if command -v pwsh >/dev/null 2>&1; then
    if pwsh -NoProfile -Command "
        \$errors = \$null
        \$null = [System.Management.Automation.Language.Parser]::ParseFile(
            (Resolve-Path 'scripts/itsanas-tray.ps1'), [ref]\$null, [ref]\$errors)
        if (\$errors) { \$errors | ForEach-Object { \$_.Message }; exit 1 }
    "; then
        say "scripts/itsanas-tray.ps1 parses"
    else
        bad "scripts/itsanas-tray.ps1 does not parse"
    fi
fi

tray_menu() {  # $1 tray, $2 instance, $3 brief, $4 interval line, $5 the brief before
    case "$1" in
        py) "$PY" scripts/itsanas-tray.py ${2:+"$2"} --describe "$3" --interval "$4" --previous "${5:-}" ;;
        js) node scripts/itsanas-menubar.js ${2:+"$2"} --describe "$3" --interval "$4" --previous "${5:-}" ;;
        ps1) pwsh -NoProfile -File scripts/itsanas-tray.ps1 ${2:+-Instance "$2"} -Describe "$3" -Interval "$4" -Previous "${5:-}" ;;
    esac 2>&1 | tr -d '\r'
}
if [ -n "$PY" ]; then
    menus_ok=1
    others=""
    command -v node >/dev/null 2>&1 && others="$others js"
    command -v pwsh >/dev/null 2>&1 && others="$others ps1"
    briefs=('healthy 30' 'paused 4000' 'stale 900' 'stopped' 'departed' 'signed-out' '')
    briefs+=('exposed 30 0 2' 'exposed 30 1 2' 'isolated 30 25200' 'isolated 30 400000' 'unwritable 30' 'failing 30 2' 'lowdisk 30 104857600')
    briefs+=('catching-up 30 600' 'twin 30')
    for brief in "${briefs[@]}"; do
        for instance in '' b; do
            for every in 'every 15 min (set with `itsanas interval`)' 'auto: the daemon'"'"'s own setting'; do
                reference=$(tray_menu py "$instance" "$brief" "$every")
                for tray in $others; do
                    drawn=$(tray_menu "$tray" "$instance" "$brief" "$every")
                    if [ "$drawn" != "$reference" ]; then
                        bad "the $tray tray draws another menu than the Linux one for '$brief' (instance '$instance'):"
                        diff <(printf '%s\n' "$reference") <(printf '%s\n' "$drawn") | sed 's/^/    /'
                        menus_ok=0
                    fi
                done
            done
        done
    done
    # And the one text they share says what HANDOVER §8 0w promises.
    healthy=$(tray_menu py b 'healthy 30' 'every 15 min (set with x)')
    for line in 'icon green' 'item Open the synced folder -> open-folder' \
                '  item For 1 hour -> itsanas --instance b pause --for 1h [confirm]' \
                '  item For 8 hours -> itsanas --instance b pause --for 8h [confirm]' \
                '  item Until I resume -> itsanas --instance b pause [confirm]' \
                '  item 15 min -> itsanas --instance b interval 15m [checked]' \
                'item Settings... -> itsanas --instance b settings' \
                'item Copy this account to a USB key... -> itsanas --instance b settings --section key' \
                'item Sign out... -> itsanas --instance b signout [confirm]' \
                'item Remove this machine... -> itsanas --instance b leave [confirm]' 'item Quit the icon -> quit'; do
        printf '%s\n' "$healthy" | grep -qxF -- "$line" \
            || { bad "the tray menu of a healthy node lacks: $line"; menus_ok=0; }
    done
    # MVP.md test L: files on this disk alone are red and say so, one copy of
    # two is orange, both with what to do; the notification comes once on
    # becoming exposed and again only when a copy is lost, the same on every
    # tray. A notification each poll is one nobody reads.
    for tray in py $others; do
        alone=$(tray_menu "$tray" '' 'exposed 30 0 2' 'auto: x')
        advice='Add a machine to this account, or keep its other machines on, so copies exist elsewhere.'
        wanted=('icon red' 'status ITSaNAS: only on this machine (0 of 2 copies elsewhere) (30 s ago)' "status $advice" "notice ITSaNAS: your files are only on this machine. $advice")
        for line in "${wanted[@]}"; do
            printf '%s\n' "$alone" | grep -qxF -- "$line" || { bad "the $tray tray of a node whose files are on its disk alone lacks: $line"; menus_ok=0; }
        done
        one=$(tray_menu "$tray" '' 'exposed 30 1 2' 'auto: x' 'healthy 30')
        if ! printf '%s\n' "$one" | grep -qxF 'icon orange' || ! printf '%s\n' "$one" | grep -q '^notice .*1 of 2 copies elsewhere'; then
            bad "the $tray tray does not turn orange and say so when a safe node loses a copy"; menus_ok=0
        fi
        for pair in 'exposed 30 1 2|exposed 30 1 2' 'exposed 30 1 2|exposed 30 0 2' 'healthy 30|exposed 30 0 2'; do
            if tray_menu "$tray" '' "${pair%%|*}" 'auto: x' "${pair#*|}" | grep -q '^notice '; then
                bad "the $tray tray notifies again for '${pair%%|*}' after '${pair#*|}': an alarm every poll"; menus_ok=0
            fi
        done
        lost=$(tray_menu "$tray" '' 'exposed 30 0 2' 'auto: x' 'exposed 30 1 2')
        printf '%s\n' "$lost" | grep -q '^notice .*only on this machine' || { bad "the $tray tray says nothing when the last copy elsewhere goes"; menus_ok=0; }
        # ARCHITECTURE §7: a node whose rounds reach nobody for hours is
        # orange, says for how long and what to check, and is told once when
        # it starts, not at every poll after.
        isolated=$(tray_menu "$tray" '' 'isolated 30 25200' 'auto: x' 'healthy 30')
        advice="Check that this machine is online, and that the account's other machines are on."
        wanted=('icon orange' 'status ITSaNAS: not synced with any machine for 7 h (30 s ago)' "status $advice" "notice ITSaNAS: not synced with any machine for 7 h. $advice")
        for line in "${wanted[@]}"; do
            printf '%s\n' "$isolated" | grep -qxF -- "$line" || { bad "the $tray tray of a node that reached nobody for seven hours lacks: $line"; menus_ok=0; }
        done
        if tray_menu "$tray" '' 'isolated 30 28800' 'auto: x' 'isolated 30 25200' | grep -q '^notice '; then
            bad "the $tray tray notifies again for a node still reaching nobody: an alarm every poll"; menus_ok=0
        fi
        unwritable=$(tray_menu "$tray" '' 'unwritable 30' 'auto: x')
        for line in 'icon orange' 'status ITSaNAS: its vault cannot be written (itsanas status says why) (30 s ago)'; do
            printf '%s\n' "$unwritable" | grep -qxF -- "$line" || { bad "the $tray tray of a node whose vault takes no write lacks: $line"; menus_ok=0; }
        done
        # A machine that failed a storage check, and a disk below the reserve:
        # orange, said, and told once; a further machine failing is news.
        failing=$(tray_menu "$tray" '' 'failing 30 2' 'auto: x' 'healthy 30')
        wanted=('icon orange' 'status ITSaNAS: 2 machine(s) failed a storage check (itsanas status says which) (30 s ago)' 'notice ITSaNAS: 2 machine(s) failed a storage check: what they hold no longer counts as a copy. itsanas status says which.')
        for line in "${wanted[@]}"; do
            printf '%s\n' "$failing" | grep -qxF -- "$line" || { bad "the $tray tray of a node whose machines failed a storage check lacks: $line"; menus_ok=0; }
        done
        if tray_menu "$tray" '' 'failing 30 2' 'auto: x' 'failing 30 2' | grep -q '^notice '; then
            bad "the $tray tray notifies again for the same failed machines: an alarm every poll"; menus_ok=0
        fi
        if ! tray_menu "$tray" '' 'failing 30 2' 'auto: x' 'failing 30 1' | grep -q '^notice '; then
            bad "the $tray tray says nothing when one more machine fails a storage check"; menus_ok=0
        fi
        # Compared as numbers: as strings, 9 machines read as more than 10.
        if ! tray_menu "$tray" '' 'failing 30 10' 'auto: x' 'failing 30 9' | grep -q '^notice '; then
            bad "the $tray tray says nothing when a tenth machine fails a storage check: the counts compare as strings"; menus_ok=0
        fi
        if tray_menu "$tray" '' 'failing 30 9' 'auto: x' 'failing 30 10' | grep -q '^notice '; then
            bad "the $tray tray notifies when a machine answers again: the counts compare as strings"; menus_ok=0
        fi
        low=$(tray_menu "$tray" '' 'lowdisk 30 104857600' 'auto: x' 'healthy 30')
        wanted=('icon orange' 'status ITSaNAS: only 100 MB free on its disk (30 s ago)' 'notice ITSaNAS: only 100 MB free on its disk. Free some room: writes fail when the disk is full.')
        for line in "${wanted[@]}"; do
            printf '%s\n' "$low" | grep -qxF -- "$line" || { bad "the $tray tray of a node whose disk is below the reserve lacks: $line"; menus_ok=0; }
        done
        if tray_menu "$tray" '' 'lowdisk 30 94857600' 'auto: x' 'lowdisk 30 104857600' | grep -q '^notice '; then
            bad "the $tray tray notifies again for a disk still below the reserve: an alarm every poll"; menus_ok=0
        fi
    done
    paused=$(tray_menu py '' 'paused 60' 'auto: x')
    for line in 'icon blue' 'item Resume syncing -> itsanas resume' '  item Automatic -> itsanas interval auto [checked]'; do
        printf '%s\n' "$paused" | grep -qxF -- "$line" \
            || { bad "the tray menu of a paused node lacks: $line"; menus_ok=0; }
    done
    if printf '%s\n' "$paused" | grep -q 'Pause syncing'; then
        bad "a paused node's tray still offers Pause, and no Resume in its place"; menus_ok=0
    fi
    signed_out=$(tray_menu py '' 'signed-out' '')
    if ! printf '%s\n' "$signed_out" | grep -qxF 'item Sign in... -> itsanas signin' \
        || ! printf '%s\n' "$signed_out" | grep -qxF 'icon grey' \
        || printf '%s\n' "$signed_out" | grep -qE 'Sync now|Pause|Sign out'; then
        bad "a signed-out node's tray does not offer Sign in alone, in grey:"
        printf '%s\n' "$signed_out" | sed 's/^/    /'
        menus_ok=0
    fi
    [ "$menus_ok" -eq 1 ] && say "the three trays draw the same menu, in every state, for $(printf 'py%s' "$others" | tr ' ' ',')"
else
    say "python is not here; the trays' menus were not compared"
fi

traydesk=$(mktemp -d)
mkdir -p "$traydesk/home" "$traydesk/fake"
for tool in launchctl systemctl; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/calls.log"\nexit 0\n' "$tool" "$traydesk" > "$traydesk/fake/$tool"
    chmod +x "$traydesk/fake/$tool"
done
tray_run() {  # [VAR=value ...] script args...
    env -u DISPLAY -u WAYLAND_DISPLAY HOME="$traydesk/home" PATH="$traydesk/fake:/usr/bin:/bin" \
        ITSANAS_BIN=/opt/itsanas/bin/itsanas "$@" </dev/null >"$traydesk/out" 2>&1
}
th="$traydesk/home"
desk_ok=1
tray_run ITSANAS_TRAY_OS=Linux sh scripts/tray-autostart.sh install b
if [ -e "$th/.config/autostart/itsanas-tray-b.desktop" ]; then
    bad "with no DISPLAY and no WAYLAND_DISPLAY (a Pi, a server) a tray autostart was written anyway"; desk_ok=0
fi
for name in '' b; do
    tray_run ITSANAS_TRAY_OS=Linux DISPLAY=:0 sh scripts/tray-autostart.sh install $name \
        || { bad "tray-autostart.sh install $name failed on a Linux desktop:"; sed 's/^/    /' "$traydesk/out"; desk_ok=0; }
    tray_run ITSANAS_TRAY_OS=Darwin sh scripts/tray-autostart.sh install $name \
        || { bad "tray-autostart.sh install $name failed on a Mac:"; sed 's/^/    /' "$traydesk/out"; desk_ok=0; }
done
desktop_b="$th/.config/autostart/itsanas-tray-b.desktop"
agent_b="$th/Library/LaunchAgents/net.itsanas.menubar.b.plist"
grep -q '^Exec=env ITSANAS_BIN="/opt/itsanas/bin/itsanas" python3 ".*/itsanas-tray.py" b$' "$desktop_b" 2>/dev/null \
    || { bad "instance b's Linux autostart does not run its own tray with its binary"; desk_ok=0; }
grep -q '^Exec=.*itsanas-tray.py"$' "$th/.config/autostart/itsanas-tray.desktop" 2>/dev/null \
    || { bad "the default node's Linux autostart is missing or names an instance"; desk_ok=0; }
grep -q '<string>b</string>' "$agent_b" 2>/dev/null && grep -q 'itsanas-menubar.js' "$agent_b" \
    || { bad "instance b's menu-bar LaunchAgent is missing or does not pass b"; desk_ok=0; }
[ -f "$th/Library/LaunchAgents/net.itsanas.menubar.plist" ] \
    || { bad "the default node's menu-bar LaunchAgent was not written"; desk_ok=0; }
[ -f "$th/.local/share/itsanas/itsanas-tray.py" ] && [ -f "$th/.local/share/itsanas/itsanas-menubar.js" ] \
    || { bad "the tray scripts were not copied out of the checkout"; desk_ok=0; }
if command -v plutil >/dev/null 2>&1 && ! plutil -lint "$agent_b" >/dev/null 2>&1; then
    bad "plutil rejects $agent_b"; desk_ok=0
fi
# The daemon of an instance named menubar owns net.itsanas.menubar.plist on a
# Mac (testbed.sh): the default node's menu-bar item must not write over it,
# and cleaning that instance must not take the default node's item.
mkdir -p "$traydesk/home2/Library/LaunchAgents"
daemon_plist="$traydesk/home2/Library/LaunchAgents/net.itsanas.menubar.plist"
printf '<plist><string>itsanas daemon</string></plist>\n' > "$daemon_plist"
env HOME="$traydesk/home2" PATH="$traydesk/fake:/usr/bin:/bin" ITSANAS_TRAY_OS=Darwin \
    sh scripts/tray-autostart.sh install </dev/null >/dev/null 2>&1
grep -q 'itsanas daemon' "$daemon_plist" \
    || { bad "the default node's menu-bar item overwrote the LaunchAgent of an instance named menubar"; desk_ok=0; }
tray_run sh install/clean.sh --yes --instance b \
    || { bad "clean.sh --instance b failed:"; sed 's/^/    /' "$traydesk/out"; desk_ok=0; }
for gone in "$desktop_b" "$agent_b"; do
    [ -e "$gone" ] && { bad "clean.sh --instance b left ${gone#"$th"/}: an icon for a node that is gone"; desk_ok=0; }
done
for kept in "$th/.config/autostart/itsanas-tray.desktop" "$th/Library/LaunchAgents/net.itsanas.menubar.plist" \
            "$th/.local/share/itsanas/itsanas-tray.py"; do
    [ -e "$kept" ] || { bad "clean.sh --instance b removed ${kept#"$th"/}, which the default node's icon needs"; desk_ok=0; }
done
tray_run sh install/clean.sh --yes \
    || { bad "clean.sh --yes failed:"; sed 's/^/    /' "$traydesk/out"; desk_ok=0; }
left=$(find "$th/.config/autostart" "$th/Library/LaunchAgents" "$th/.local/share/itsanas" -type f 2>/dev/null)
if [ -n "$left" ]; then
    bad "clean.sh --yes left tray files behind, starting an icon for a program that is gone:"
    printf '%s\n' "$left" | sed 's/^/    /'
    desk_ok=0
fi
[ "$desk_ok" -eq 1 ] && say "tray autostart: written per instance on a Mac and a Linux desktop, none headless, b cleaned alone, all cleaned"
# red_team: ITSANAS_BIN comes from macos.sh --prefix or provision.sh. A newline
# in it would split the Exec line and add keys of its own; a % is a field code
# that makes the entry silently not start. Both must be refused, nothing written.
inject_ok=1
for evil in "$(printf '/x/a\nExec=evil')" '/x/100%/itsanas'; do
    rm -rf "$th/.config/autostart"
    if env -u WAYLAND_DISPLAY HOME="$th" PATH="$traydesk/fake:/usr/bin:/bin" DISPLAY=:0 \
        ITSANAS_TRAY_OS=Linux ITSANAS_BIN="$evil" sh scripts/tray-autostart.sh install b </dev/null >/dev/null 2>&1 \
        || [ -e "$th/.config/autostart/itsanas-tray-b.desktop" ]; then
        bad "red_team: tray-autostart.sh accepted ITSANAS_BIN=$(printf '%s' "$evil" | tr '\n' '|') and wrote a .desktop entry"; inject_ok=0
    fi
done
[ "$inject_ok" -eq 1 ] && say "red_team: a newline or % in ITSANAS_BIN is refused, no .desktop entry written"
rm -rf "$traydesk"
