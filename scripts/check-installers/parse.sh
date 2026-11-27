# Sourced by scripts/check-installers.sh, in its shell and in its order:
# every script parses, units and kit scripts included, and each is in the README.

# ------------------------------------------------------------------- syntax

for script in "${SH_SCRIPTS[@]}"; do
    if sh -n "$script" 2>/tmp/itsanas-shcheck; then
        say "$script parses as POSIX sh"
    else
        bad "$script does not parse:"
        sed 's/^/       /' /tmp/itsanas-shcheck
    fi
done
rm -f /tmp/itsanas-shcheck

# Comment lines are stripped first. The first version did not, and flagged the
# sentence in linux.sh explaining why bashisms are avoided -- the same mistake
# as a dead-code check that counts a doc link as a call site, made twice in one
# week by the same person.
BASHISM='\[\[|^[[:space:]]*local |^[[:space:]]*function [A-Za-z_]'
for script in "${SH_SCRIPTS[@]}"; do
    hits=$(grep -vE '^[[:space:]]*#' "$script" | grep -nE "$BASHISM")
    if [ -n "$hits" ]; then
        bad "$script uses bash-only syntax:"
        printf '%s\n' "$hits" | sed 's/^/       /'
    fi
done
say "no bash-only syntax in the POSIX scripts"

if command -v shellcheck >/dev/null 2>&1; then
    for script in "${SH_SCRIPTS[@]}"; do
        if shellcheck -s sh -S warning "$script"; then
            say "$script passes shellcheck"
        else
            bad "$script has shellcheck warnings"
        fi
    done
else
    say "shellcheck is not installed here; skipped"
fi

for script in "${PS_SCRIPTS[@]}"; do
    if command -v pwsh >/dev/null 2>&1; then
        if pwsh -NoProfile -Command "
            \$errors = \$null
            \$null = [System.Management.Automation.Language.Parser]::ParseFile(
                (Resolve-Path '$script'), [ref]\$null, [ref]\$errors)
            if (\$errors) { \$errors | ForEach-Object { \$_.Message }; exit 1 }
        "; then
            say "$script parses"
        else
            bad "$script does not parse"
        fi
    else
        say "pwsh is not installed here; $script was not parsed"
    fi
done

# ------------------------------------------------------------- systemd units

# An invalid unit fails at `systemctl start` with a message naming a line in a
# file the user has never seen. systemd will say so now instead, when it is here
# to ask: it is on the CI runner and on any Linux this installs to, and absent
# on a Mac or on Windows, where skipping is correct rather than lax.
# The acceptance kit's Windows half is not an installer -- no --clean, no
# README row -- but a PowerShell script nobody parses is the same risk: it
# is run once, on the morning it matters, by the person it is for. The fleet
# agent and its setup (scripts/fleet/) too. What the night run executes --
# those and the kit -- is ASCII or carries a BOM: the agent runs the kit with
# powershell.exe 5.1, which reads a file without one as the ANSI code page,
# so one typographic quote in a string is a parse error nobody sees.
shopt -s nullglob
KIT_PS=(scripts/*.ps1 scripts/fleet/*.ps1)
shopt -u nullglob
for script in scripts/acceptance.ps1 scripts/fleet/*.ps1; do
    [ "$(head -c3 "$script" | od -An -tx1 | tr -d ' ')" = efbbbf ] && continue
    if LC_ALL=C grep -q '[^ -~[:space:]]' "$script"; then
        bad "$script has a byte outside ASCII and no BOM, which powershell.exe 5.1 misreads: $(LC_ALL=C grep -n '[^ -~[:space:]]' "$script" | head -1 | cut -c1-80)"
    fi
done
for script in "${KIT_PS[@]}"; do
    if command -v pwsh >/dev/null 2>&1; then
        if pwsh -NoProfile -Command "
            \$errors = \$null
            \$null = [System.Management.Automation.Language.Parser]::ParseFile(
                (Resolve-Path '$script'), [ref]\$null, [ref]\$errors)
            if (\$errors) { \$errors | ForEach-Object { \$_.Message }; exit 1 }
        "; then
            say "$script parses"
        else
            bad "$script does not parse"
        fi
    else
        say "pwsh is not installed here; $script was not parsed"
    fi
done

if command -v systemd-analyze >/dev/null 2>&1; then
    units=$(mktemp -d)

    # linux.sh writes two units from one heredoc: the default node, and the
    # template for named instances. Both flavours are verified as written; the
    # template under an instance name, because systemd-analyze cannot verify a
    # bare `name@.service`.
    member_unit() {
        sed -n '/^\[Unit\]/,/^WantedBy=default.target$/p' install/linux.sh |
            sed 's|\$BIN_DIR|/usr/local/bin|g' |
            sed "s|[\$]UNIT_DESC|$1|; s|[\$]UNIT_HOME_ENV|$2|; s|[\$]UNIT_ENV_FILE|$3|; s|[\$]UNIT_STATE|$4|"
    }
    member_unit "ITSaNAS peer-to-peer storage" "" "%h/.config/itsanas/environment" "%h/.itsanas" \
        > "$units/itsanas.service"
    member_unit "ITSaNAS peer-to-peer storage, instance %i" "Environment=ITSANAS_HOME=%h/.itsanas-%i" \
        "%h/.config/itsanas/%i.environment" "%h/.itsanas-%i" > "$units/itsanas@check.service"
    if grep -q '[$]UNIT_' "$units/itsanas.service" "$units/itsanas@check.service"; then
        bad "a unit variable in install/linux.sh is not substituted by this check"
    fi

    sed -n '/^\[Unit\]/,/^WantedBy=multi-user.target$/p' install/coordinator.sh \
        | sed 's|\$SERVICE_USER|itsanas-coord|g' \
        | sed 's|\$STATE_DIR|/var/lib/itsanas-coordinator|g' \
        | sed 's|\$PORT|9898|g' \
        | sed 's|\$ADMIT||g' \
        > "$units/itsanas-coordinator.service"

    # A ReadWritePaths entry without a leading dash refuses to start the unit
    # when the path does not exist yet. That is not hypothetical: the member
    # unit listed ~/.itsanas, which `itsanas init` creates, so enabling the
    # service before initialising -- exactly what somebody does when an
    # installer says "enable this" -- failed with an error about mount
    # namespaces. systemd-analyze does not catch it, because the unit is valid.
    for unit in "$units"/*.service; do
        # Split the entry into its paths, keep the ones that are not prefixed
        # with a dash. A trailing pipe continues the line on its own, which is
        # the point: this pipeline was written with backslashes and reached the
        # repository with them eaten and the indentation left in place.
        risky=$(grep -oE '^ReadWritePaths=.*' "$unit" |
            tr ' ' '\n' |
            grep -E '^(/|%h|ReadWritePaths=[^-])' |
            grep -vE '^ReadWritePaths=-|^-')
        if [ -n "$risky" ]; then
            bad "$(basename "$unit") has a ReadWritePaths without a leading dash:"
            printf '%s\n' "$risky" | sed 's/^/       /'
            say "  Without it the unit refuses to start if the path is not there yet."
        fi
    done

    for unit in "$units"/*.service; do
        # The binary is not installed on the machine doing the checking, so that
        # one complaint is expected and is not what is being asked about.
        problems=$(systemd-analyze verify "$unit" 2>&1 | grep -v 'is not executable')
        if [ -z "$problems" ]; then
            say "$(basename "$unit") is a valid unit"
        else
            bad "$(basename "$unit") is not:"
            printf '%s\n' "$problems" | sed 's/^/       /'
        fi
    done
    rm -rf "$units"
else
    say "systemd-analyze is not here; the units were not verified"
fi


# -------------------------------------------------------------- the claims

# Every script must be named in the README. One that nothing links to is one
# nobody runs, and `install/coordinator.sh` reached the repository that way.
for script in "${SH_SCRIPTS[@]}" "${PS_SCRIPTS[@]}" install/*.md; do
    name=$(basename "$script")
    [ "$name" = "README.md" ] && continue
    if grep -q "$name" install/README.md; then
        say "README.md mentions $name"
    else
        bad "install/$name exists and install/README.md does not mention it"
    fi
done
