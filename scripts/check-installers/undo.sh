# Sourced by scripts/check-installers.sh, in its shell and in its order:
# every installer undoes itself with --clean, and the coordinator setup re-runs in place.

# ------------------------------------------------------- undoing the install
#
# install/README.md says "every installer takes --clean". That sentence is worth
# exactly what checks it, and until this block existed the answer was nothing —
# the same shape as every other prose claim in this repository that turned out
# to be false months after it was written.
#
# It is not a grep for the string. Each script is *run* with --clean and has to
# reach the uninstaller's dry run, which is what proves the delegation resolves,
# the sibling is found, and the argument is not being swallowed by a case arm
# that shifts and falls through. A grep would have passed on all three faults.
#
# Discovered, not listed, for the reason at the top of this file.

CLEAN_MARK="Nothing was changed"

# A hang here is worse than a failure -- it is a gate that never finishes on
# somebody else's machine -- but `timeout` is not everywhere, and a missing
# tool must not turn into seven false accusations against the installers.
if command -v timeout >/dev/null 2>&1; then
    LIMIT="timeout 60"
else
    LIMIT=""
fi

for script in "${SH_SCRIPTS[@]}"; do
    [ "$script" = "install/clean.sh" ] && continue
    # shellcheck disable=SC2086
    out=$($LIMIT sh "$script" --clean 2>&1 </dev/null)
    if printf '%s' "$out" | grep -q "$CLEAN_MARK"; then
        say "$script --clean reaches the uninstaller"
    else
        bad "$script --clean does not reach the uninstaller's dry run"
        printf '%s\n' "$out" | tail -3 | sed 's/^/       /'
        say "  Every entry point must be able to undo itself, by delegating to"
        say "  install/clean.sh. One list of paths, not six."
    fi
done

for script in "${PS_SCRIPTS[@]}"; do
    [ "$script" = "install/clean.ps1" ] && continue
    if ! command -v pwsh >/dev/null 2>&1; then
        say "pwsh is not here; $script -Clean was not run"
        continue
    fi
    # shellcheck disable=SC2086
    out=$($LIMIT pwsh -NoProfile -File "$script" -Clean 2>&1 </dev/null)
    if printf '%s' "$out" | grep -q "$CLEAN_MARK"; then
        say "$script -Clean reaches the uninstaller"
    else
        bad "$script -Clean does not reach the uninstaller's dry run"
        printf '%s\n' "$out" | tail -3 | sed 's/^/       /'
    fi
done

# And the flag has to be findable. A switch that works and is undocumented is a
# switch nobody types.
for script in "${SH_SCRIPTS[@]}"; do
    [ "$script" = "install/clean.sh" ] && continue
    # shellcheck disable=SC2086
    if ! $LIMIT sh "$script" --help 2>&1 </dev/null | grep -q -- '--clean'; then
        bad "$script --help does not mention --clean"
    fi
done
say "every installer offers --clean, and says so in its help"

# ------------------------------------- re-running the coordinator setup in place
#
# install/coordinator.sh tells its reader to run it a second time, without
# --admit-first. The binary it finds on that run is the one it installed on the
# first, and `install` refuses to copy a file onto itself, so the documented
# second step died on the Freebox VM with the service already stopped.
#
# The script as a whole needs root and systemd, so the copy step is run alone:
# the function is cut out of the installer as written and called with a
# temporary directory standing in for /usr/local/bin. Both directions are asked,
# because a guard that skipped every copy would pass the first half.

place=$(sed -n '/^place_binary() {$/,/^}$/p' install/coordinator.sh)
if [ -z "$place" ]; then
    bad "install/coordinator.sh has no place_binary() for this check to run"
    say "  The copy step was renamed or reshaped; update the extraction here."
else
    bin=$(mktemp -d)
    printf '#!/bin/sh\necho old\n' > "$bin/installed"
    printf '#!/bin/sh\necho new\n' > "$bin/fresh"
    chmod 755 "$bin/installed" "$bin/fresh"
    harness='ok() { printf "ok %s\n" "$*"; }; die() { printf "error %s\n" "$1"; exit 1; }'

    out=$(sh -c "$harness
$place
place_binary \"\$1\" \"\$1\"" place "$bin/installed" 2>&1)
    status=$?
    if [ "$status" -ne 0 ] || printf '%s' "$out" | grep -q '^error'; then
        bad "install/coordinator.sh cannot be re-run over the binary it installed"
        printf '%s\n' "$out" | sed 's/^/       /'
        say "  Changing a flag means re-running it; the service is down by then."
    else
        say "install/coordinator.sh re-runs over its own installed binary"
    fi

    out=$(sh -c "$harness
$place
place_binary \"\$1\" \"\$2\"" place "$bin/fresh" "$bin/installed" 2>&1)
    status=$?
    if [ "$status" -ne 0 ] || ! grep -q new "$bin/installed"; then
        bad "install/coordinator.sh does not replace an installed binary with a new one"
        printf '%s\n' "$out" | sed 's/^/       /'
    else
        say "install/coordinator.sh still replaces an older installed binary"
    fi
    rm -rf "$bin"
fi
