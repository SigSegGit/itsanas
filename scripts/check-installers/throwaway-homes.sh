# Sourced by scripts/check-installers.sh, in its shell and in its order:
# installs run in a throwaway HOME: two instances, the test bed, the PATH line; and a PowerShell name clash.

# ------------------------------------------- two instances, one cleaned
#
# HANDOVER §8 0i's red-team test, hermetic: provision instances `a` and `b`
# in a throwaway HOME, clean `b` with --purge-account, and check that `a`'s
# home, passphrase file, the shared unit template and a's enablement are all
# still there. #18 once had provision.ps1 kill every itsanas process; a clean
# that globbed `*.environment` or disabled `itsanas@*` would take a sibling's
# service down, and nobody would notice until its owner's files stopped
# arriving. What this cannot see: that `a` really keeps syncing -- the
# `itsanas` here is a stub, systemd a logger.

twin=$(mktemp -d)
mkdir -p "$twin/home/.local/bin" "$twin/home/.config/systemd/user" "$twin/fake" "$twin/scripts"
# A copy outside the checkout, so provision finds no scripts/smoke.sh to run
# against the stub.
cp install/provision.sh install/clean.sh scripts/tray-autostart.sh scripts/itsanas-tray.py "$twin/scripts/"
cat > "$twin/home/.local/bin/itsanas" <<'STUB'
#!/bin/sh
case "$1" in
    init) mkdir -p "$ITSANAS_HOME" && : > "$ITSANAS_HOME/keystore.bin" && echo "word word word" ;;
    status) echo "stub node at $ITSANAS_HOME" ;;
esac
exit 0
STUB
cat > "$twin/fake/systemctl" <<'FAKE'
#!/bin/sh
echo "$*" >> "$TWIN_LOG"
case "$*" in *is-active*) exit 3 ;; esac
exit 0
FAKE
chmod +x "$twin/home/.local/bin/itsanas" "$twin/fake/systemctl"
: > "$twin/home/.config/systemd/user/itsanas@.service"
twin_run() {
    env HOME="$twin/home" PATH="$twin/fake:/usr/bin:/bin" TWIN_LOG="$twin/systemctl.log" \
        ITSANAS_PASSPHRASE=a-test-passphrase XDG_RUNTIME_DIR="$twin" DISPLAY=:0 ITSANAS_TRAY_OS=Linux \
        timeout 60 sh "$@" </dev/null >"$twin/out" 2>&1
}
twin_ok=1
for name in a b; do
    if ! twin_run "$twin/scripts/provision.sh" --username "u$name" --no-install --instance "$name"; then
        bad "provision.sh --instance $name failed in a throwaway home"
        sed 's/^/    /' "$twin/out"
        twin_ok=0
    fi
done
if [ "$twin_ok" -eq 1 ] && ! twin_run "$twin/scripts/clean.sh" --yes --purge-account --instance b; then
    bad "clean.sh --instance b failed in a throwaway home"
    sed 's/^/    /' "$twin/out"
    twin_ok=0
fi
if [ "$twin_ok" -eq 1 ]; then
    h="$twin/home"
    for kept in "$h/.itsanas-a/keystore.bin" "$h/.config/itsanas/a.environment" \
                "$h/.config/systemd/user/itsanas@.service" "$h/.config/autostart/itsanas-tray-a.desktop"; do
        [ -e "$kept" ] || { bad "cleaning instance b removed ${kept#"$h"/}, which belongs to a or to both"; twin_ok=0; }
    done
    for gone in "$h/.itsanas-b" "$h/.config/itsanas/b.environment" "$h/.config/autostart/itsanas-tray-b.desktop"; do
        [ -e "$gone" ] && { bad "clean.sh --instance b --purge-account left ${gone#"$h"/}"; twin_ok=0; }
    done
    grep -q 'enable --now itsanas@a' "$twin/systemctl.log" \
        || { bad "provision.sh --instance a never enabled itsanas@a"; twin_ok=0; }
    if grep -E '(disable|stop).*itsanas@a|(disable|stop).*itsanas@\*|(disable|stop).* itsanas( |$)' "$twin/systemctl.log" >/dev/null; then
        bad "provisioning or cleaning b stopped or disabled a sibling's service:"
        sed 's/^/    /' "$twin/systemctl.log"
        twin_ok=0
    fi
    [ "$twin_ok" -eq 1 ] && say "two instances provisioned, b cleaned, a untouched"
fi
rm -rf "$twin"


# ------------------------------- a PowerShell variable that erases a parameter
#
# PowerShell names are case-insensitive. testbed.ps1 shipped with a temporary
# `$phraseFile = $null` that silently emptied its `-PhraseFile` parameter, so
# an unattended join fell through to a prompt and hung with no terminal
# (found deploying on this laptop, 2026-10-06). Text only, so it runs where
# pwsh does not: any assignment to a parameter's name in another case.
for script in install/*.ps1; do
    params=$(sed -n '/^param(/,/^)/p' "$script" | grep -oE '\$[A-Za-z]+' | sort -u)
    for param in $params; do
        name=${param#\$}
        clash=$(grep -niE "^[[:space:]]*\\\$$name[[:space:]]*=" "$script" | grep -vE "\\\$$name[[:space:]]*=" || true)
        if [ -n "$clash" ]; then
            bad "$script assigns to a variable that is its -$name parameter in another case; PowerShell treats them as one"
            printf '%s\n' "$clash" | sed 's/^/       /'
        fi
    done
done
say "no PowerShell script reassigns a parameter under another case"

# ------------------------------------------ the test bed spares real accounts
#
# HANDOVER §8 0r's red-team test: testbed.sh --fresh on a home that holds a
# real account (`nicolas`) and an earlier bed. The real node, its passphrase
# file and its service must come through untouched, and the earlier bed must
# be in an archive, not gone: it may hold the only copy of something.

bed=$(mktemp -d)
h="$bed/home"
mkdir -p "$h/.local/bin" "$h/.config/systemd/user" "$h/.config/itsanas" "$bed/fake" "$bed/scripts" \
    "$h/.itsanas-nicolas" "$h/.itsanas-essai" "$h/ITSaNAS-essai"
cp install/testbed.sh install/provision.sh "$bed/scripts/"
: > "$h/.itsanas-nicolas/keystore.bin"
echo 'ITSANAS_PASSPHRASE=real' > "$h/.config/itsanas/nicolas.environment"
echo 'old bed' > "$h/.itsanas-essai/only-copy.txt"
: > "$h/.itsanas-essai/keystore.bin"
echo 'ITSANAS_PASSPHRASE=an-earlier-bed' > "$h/.config/itsanas/essai.environment"
cat > "$h/.local/bin/itsanas" <<'STUB'
#!/bin/sh
case "$1" in
    init) mkdir -p "$ITSANAS_HOME" && : > "$ITSANAS_HOME/keystore.bin" && echo "word word word" ;;
    login) mkdir -p "$ITSANAS_HOME" && : > "$ITSANAS_HOME/keystore.bin" ;;
esac
exit 0
STUB
cat > "$bed/fake/systemctl" <<'FAKE'
#!/bin/sh
echo "$*" >> "$BED_LOG"
case "$*" in *is-active*) exit 3 ;; esac
exit 0
FAKE
chmod +x "$h/.local/bin/itsanas" "$bed/fake/systemctl"
: > "$h/.config/systemd/user/itsanas@.service"
bed_ok=1
if ! env HOME="$h" PATH="$bed/fake:/usr/bin:/bin" TESTBED_OS=Linux BED_LOG="$bed/systemctl.log" XDG_RUNTIME_DIR="$bed" \
        timeout 60 sh "$bed/scripts/testbed.sh" --coordinator 192.0.2.1:9898 --coordinator-device d \
        --invite c --fresh --no-install </dev/null >"$bed/out" 2>&1; then
    bad "testbed.sh --fresh failed in a throwaway home"
    sed 's/^/    /' "$bed/out"
    bed_ok=0
fi
for kept in "$h/.itsanas-nicolas/keystore.bin" "$h/.config/itsanas/nicolas.environment"; do
    [ -e "$kept" ] || { bad "testbed.sh removed ${kept#"$h"/}, which belongs to a real account"; bed_ok=0; }
done
# shellcheck disable=SC2010 # one directory of known names
ls "$h"/itsanas-archive-*/.itsanas-essai/only-copy.txt >/dev/null 2>&1 \
    || { bad "testbed.sh --fresh did not archive the earlier bed; its files are gone"; bed_ok=0; }
[ -e "$h/.itsanas-essai/keystore.bin" ] || { bad "testbed.sh made no new essai node"; bed_ok=0; }
if grep -E '(disable|stop).*itsanas@nicolas' "$bed/systemctl.log" >/dev/null 2>&1; then
    bad "testbed.sh stopped or disabled the real account's service"; bed_ok=0
fi
[ "$bed_ok" -eq 1 ] && say "test bed: real account untouched, earlier bed archived"

# An unattended join (--phrase-file, over SSH): the words file is the caller's.
# Deleting it would lose the only copy someone typed out on a machine with no
# other record of the 24 words.
rm -rf "$h/.itsanas-essai" "$h/.config/itsanas/essai.environment"
printf '%s\n' "w w w w w w w w w w w w w w w w w w w w w w w w" > "$bed/words.txt"
chmod 600 "$bed/words.txt"
if ! env HOME="$h" PATH="$bed/fake:/usr/bin:/bin" TESTBED_OS=Linux BED_LOG="$bed/systemctl.log" XDG_RUNTIME_DIR="$bed" \
        timeout 60 sh "$bed/scripts/testbed.sh" --coordinator 192.0.2.1:9898 --coordinator-device d \
        --phrase-file "$bed/words.txt" --no-install </dev/null >"$bed/out" 2>&1; then
    bad "testbed.sh --phrase-file failed in a throwaway home"
    sed 's/^/    /' "$bed/out"
elif [ ! -e "$bed/words.txt" ]; then
    bad "testbed.sh --phrase-file deleted the caller's file of 24 words"
elif [ ! -e "$h/.itsanas-essai/keystore.bin" ]; then
    bad "testbed.sh --phrase-file did not restore the essai account"
else
    say "test bed: an unattended join keeps the caller's words file"
fi

# The Mac's bed is a LaunchAgent of its own; clean.sh --instance must know it,
# or "--clean --yes" leaves a daemon starting at every login with a passphrase
# in its plist. The default node's agent is not the instance's to remove.
la="$h/Library/LaunchAgents"
mkdir -p "$la"
: > "$la/net.itsanas.essai.plist"
: > "$la/net.itsanas.daemon.plist"
env HOME="$h" PATH="$bed/fake:/usr/bin:/bin" timeout 30 sh install/clean.sh --instance essai </dev/null >/dev/null 2>&1
if [ ! -e "$la/net.itsanas.essai.plist" ]; then
    bad "clean.sh --instance essai removed the agent without --yes (a dry run changed something)"
else
    env HOME="$h" PATH="$bed/fake:/usr/bin:/bin" timeout 30 sh install/clean.sh --instance essai --yes </dev/null >/dev/null 2>&1
    if [ -e "$la/net.itsanas.essai.plist" ]; then
        bad "clean.sh --instance essai --yes left the bed's LaunchAgent in place"
    elif [ ! -e "$la/net.itsanas.daemon.plist" ]; then
        bad "clean.sh --instance essai removed the default node's LaunchAgent"
    else
        say "clean.sh --instance removes that instance's LaunchAgent and no other"
    fi
fi
rm -rf "$bed"

# --purge-folder takes the synced folder of the account it removes, and only a
# folder holding the marker: a config pointing at a home or a disk root must
# never be emptied by a clean-up (asked by Nicolas, 2026-10-08).
pf=$(mktemp -d)
mkdir -p "$pf/.itsanas-b" "$pf/My F" "$pf/plain" "$pf/.itsanas-c"
: > "$pf/.itsanas-b/keystore.bin"; : > "$pf/.itsanas-c/keystore.bin"
: > "$pf/My F/.itsanas-folder"; : > "$pf/My F/a file"; : > "$pf/plain/kept"
printf 'folder = %s\n' "$pf/My F" > "$pf/.itsanas-b/config"
printf 'folder = %s\n' "$pf/plain" > "$pf/.itsanas-c/config"
for i in b c; do
    env HOME="$pf" PATH=/usr/bin:/bin timeout 30 sh install/clean.sh --instance "$i" --yes \
        --purge-account --purge-folder --drain-for 0 --without-passphrase </dev/null >/dev/null 2>&1
done
# Without the passphrase (and no program to check one with) an account stays:
# one person on a shared machine must not remove another's by mistake.
mkdir -p "$pf/.itsanas-d"; : > "$pf/.itsanas-d/keystore.bin"
env HOME="$pf" PATH=/usr/bin:/bin timeout 30 sh install/clean.sh --instance d --yes \
    --purge-account --drain-for 0 </dev/null >/dev/null 2>&1
if [ ! -e "$pf/.itsanas-d/keystore.bin" ]; then
    bad "clean.sh --purge-account removed an account without its passphrase"
elif [ -e "$pf/My F" ]; then
    bad "clean.sh --purge-folder left the synced folder of the account it removed"
elif [ ! -e "$pf/plain/kept" ]; then
    bad "clean.sh --purge-folder emptied a folder with no .itsanas-folder marker"
elif env HOME="$pf" sh install/clean.sh --purge-folder </dev/null >/dev/null 2>&1; then
    bad "clean.sh accepted --purge-folder without --purge-account"
else
    say "clean.sh --purge-folder removes the account's marked folder, and nothing unmarked"
fi
rm -rf "$pf"

# ------------------------------------------------- the PATH line, added once
#
# HANDOVER §8 3e. On the first real Mac the install succeeded and `itsanas` was
# "command not found": ~/.local/bin is on nobody's PATH there and the installer
# only warned. linux.sh and macos.sh now add one marked line to the login
# profile; this runs that code in a throwaway HOME. A second run must leave one
# line (a profile that grows a line per install is the next bug report), and
# clean.sh must remove that line and only that line.

path_block() { sed -n '/^# >>> path-line/,/^# <<< path-line/p' "$1"; }
if [ -z "$(path_block install/macos.sh)" ]; then
    bad "install/macos.sh has no path-line block; the PATH fix is gone"
elif [ "$(path_block install/macos.sh)" != "$(path_block install/linux.sh)" ]; then
    bad "the path-line block differs between install/linux.sh and install/macos.sh"
    say "  clean.sh removes one marked line; two versions of how it is written drift apart."
else
    ph=$(mktemp -d)
    path_block install/macos.sh > "$ph/block.sh"
    printf 'alias ll="ls -l"' > "$ph/.zprofile"   # a person's line, no final newline
    # shellcheck disable=SC2016
    out=$(env HOME="$ph" SHELL=/bin/zsh PATH=/usr/bin:/bin sh -c '
        . "$HOME/block.sh"
        prof=$(path_profile)
        add_path_line "$prof" "$HOME/.local/bin"; a=$?
        add_path_line "$prof" "$HOME/.local/bin"; b=$?
        echo "$prof $a $b"' 2>&1)
    lines=$(grep -c 'added by the ITSaNAS installer' "$ph/.zprofile" 2>/dev/null)
    if [ "$out" != "$ph/.zprofile 0 1" ]; then
        bad "path-line under zsh: expected '$ph/.zprofile 0 1', got '$out'"
    elif [ "$lines" != 1 ]; then
        bad "two installs left $lines PATH lines in ~/.zprofile, not one"
    elif ! grep -qx 'alias ll="ls -l"' "$ph/.zprofile"; then
        bad "adding the PATH line glued it onto the person's last line"
    else
        say "a second install leaves one PATH line in ~/.zprofile"
    fi
    if ! env HOME="$ph" PATH=/usr/bin:/bin timeout 30 sh install/clean.sh --yes \
            </dev/null >"$ph/out" 2>&1; then
        bad "clean.sh --yes failed in a throwaway home"
        sed 's/^/    /' "$ph/out"
    elif grep -q 'ITSaNAS installer' "$ph/.zprofile"; then
        bad "clean.sh --yes left the installer's PATH line in ~/.zprofile"
    elif [ "$(cat "$ph/.zprofile")" != 'alias ll="ls -l"' ]; then
        bad "clean.sh --yes changed ~/.zprofile beyond the installer's line:"
        sed 's/^/    /' "$ph/.zprofile"
    else
        say "clean.sh removes exactly the installer's PATH line"
    fi
    # Debian's stock ~/.profile already puts $HOME/.local/bin on the PATH at
    # login; a second line would only list it twice.
    printf '%s\n' 'if [ -d "$HOME/.local/bin" ] ; then PATH="$HOME/.local/bin:$PATH"; fi' > "$ph/.profile"
    # shellcheck disable=SC2016
    out=$(env HOME="$ph" SHELL=/bin/sh PATH=/usr/bin:/bin sh -c '
        . "$HOME/block.sh"; add_path_line "$(path_profile)" "$HOME/.local/bin"; echo $?' 2>&1)
    if [ "$out" != 1 ] || grep -q 'ITSaNAS installer' "$ph/.profile"; then
        bad "path-line added a second PATH entry to a Debian ~/.profile (got '$out')"
    else
        say "a Debian ~/.profile that already has ~/.local/bin is left alone"
    fi
    rm -rf "$ph"
fi
