#!/usr/bin/env sh
# Take a reading of the filesystem's error counters, and compare it with an
# earlier one.
#
# Why this exists
# ---------------
#
# On 2026-09-01 a Raspberry Pi in this fleet was about to be given a long
# compile. Its ext4 had logged block-bitmap corruption at boot. The check that
# was made before starting was a single reading of the error counter — it said
# six, the same six that were there at boot — and that reading was treated as
# evidence that nothing was getting worse. It could not have said anything of
# the kind: a counter that has not been read twice has no direction. The build
# ran, the filesystem failed during it, and the card was replaced.
#
# So this script refuses to answer the question from one reading. `before`
# writes a snapshot; `after` takes a second one and reports what MOVED. A number
# is only evidence when it is a difference.
#
# It needs no privileges. `dumpe2fs` needs root; `/sys/fs/ext4/<dev>/` does not,
# and it is live rather than a copy of the superblock. The kernel log is read
# through `journalctl -k`, which works for anyone in `adm` or `systemd-journal`.
#
# Every count carries a CONTROL: a second grep, over the same input, for a
# pattern that must match. A zero from a search that cannot find anything is not
# a clean bill of health, it is a broken search — which is the same mistake as
# the one above, wearing different clothes.
#
# Usage
# -----
#
#   scripts/disk-health.sh before /tmp/health     # before the heavy work
#   ... the heavy work ...
#   scripts/disk-health.sh after /tmp/health      # exits non-zero if it moved
#
# POSIX sh, no `set -e`: this is a diagnostic, and a diagnostic that exits
# silently in the middle of itself is worse than useless.

set -u

usage() {
    cat <<'USAGE'
Read this filesystem's error counters, and compare two readings.

  scripts/disk-health.sh before <file>   write a snapshot
  scripts/disk-health.sh after  <file>   compare with it, and say what moved

Exit status of `after`:
  0  nothing moved
  1  something moved, or the earlier snapshot is missing or unreadable
  2  the readings cannot be trusted, because a control search matched nothing

Needs no root. Reads /sys/fs/ext4 and, for the kernel log, `journalctl -k`,
which works for members of `adm` or `systemd-journal`.
USAGE
}

# The block device behind `/`, as the bare name /sys/fs/ext4 uses.
root_device() {
    if command -v findmnt >/dev/null 2>&1; then
        device=$(findmnt -no SOURCE / 2>/dev/null)
    else
        device=$(df -P / 2>/dev/null | awk 'NR == 2 { print $1 }')
    fi
    basename "$device" 2>/dev/null
}

# One `key=value` line per counter. Anything unavailable is reported as
# `unavailable` rather than as zero: a missing reading and a reading of zero are
# not the same claim, and writing the second when you mean the first is how a
# check comes to pass on a machine it never looked at.
snapshot() {
    device=$(root_device)
    printf 'device=%s\n' "${device:-unknown}"

    base="/sys/fs/ext4/$device"
    for counter in errors_count first_error_time last_error_time; do
        if [ -r "$base/$counter" ]; then
            printf '%s=%s\n' "$counter" "$(cat "$base/$counter" 2>/dev/null)"
        else
            printf '%s=unavailable\n' "$counter"
        fi
    done

    # The kernel log. `journalctl -k` on a systemd machine, `dmesg` where an
    # unprivileged read of it is allowed, and neither on a machine that permits
    # neither — in which case say so.
    log=""
    if command -v journalctl >/dev/null 2>&1; then
        log=$(journalctl -k --no-pager 2>/dev/null)
    fi
    if [ -z "$log" ] && command -v dmesg >/dev/null 2>&1; then
        log=$(dmesg 2>/dev/null)
    fi

    if [ -z "$log" ]; then
        printf 'kernel_errors=unavailable\n'
        printf 'kernel_control=0\n'
    else
        printf 'kernel_errors=%s\n' "$(printf '%s\n' "$log" |
            grep -icE 'I/O error|EXT4-fs error|EFSCORRUPTED|structure needs cleaning|Remounting filesystem read-only')"
        # The control. This pattern names the root device and the filesystem
        # driver, both of which appear in the log of any machine that booted
        # from this disk. If it is zero, the search above proved nothing.
        #
        # The device name is only put in the pattern when there is one. Written
        # as `grep -E "$device|EXT4-fs"` with $device empty, the alternation has
        # an empty branch, which matches every line — a control that always
        # passes, in a script whose whole subject is checks that cannot fail.
        if [ -n "$device" ]; then
            control="$device|EXT4-fs"
        else
            control="EXT4-fs"
        fi
        printf 'kernel_control=%s\n' "$(printf '%s\n' "$log" | grep -icE "$control")"
    fi

    # Thermal state, on the machines that report it. Not an error counter, but
    # the first thing anybody asks when a small machine misbehaves under load,
    # and cheaper to record now than to reconstruct later.
    if command -v vcgencmd >/dev/null 2>&1; then
        printf 'temperature=%s\n' "$(vcgencmd measure_temp 2>/dev/null | cut -d= -f2)"
        printf 'throttled=%s\n' "$(vcgencmd get_throttled 2>/dev/null | cut -d= -f2)"
    fi
}

value_of() {
    grep "^$1=" "$2" 2>/dev/null | cut -d= -f2-
}

case "${1:-}" in
    before)
        [ $# -ge 2 ] || { usage; exit 1; }
        snapshot > "$2" || { printf 'cannot write %s\n' "$2"; exit 1; }
        printf 'snapshot written to %s\n' "$2"
        sed 's/^/  /' "$2"

        if [ "$(value_of kernel_control "$2")" = "0" ]; then
            printf '\nthe kernel log could not be searched here, so a later\n'
            printf 'reading of zero errors will mean nothing. Add this user to\n'
            printf 'the `adm` group, or run this where the log is readable.\n'
            exit 2
        fi
        ;;

    after)
        [ $# -ge 2 ] || { usage; exit 1; }
        [ -r "$2" ] || { printf 'no earlier snapshot at %s\n' "$2"; exit 1; }

        now="$2.after"
        snapshot > "$now" || { printf 'cannot write %s\n' "$now"; exit 1; }

        moved=0
        for counter in errors_count first_error_time last_error_time kernel_errors; do
            was=$(value_of "$counter" "$2")
            is=$(value_of "$counter" "$now")
            if [ "$was" != "$is" ]; then
                printf 'MOVED  %s: %s -> %s\n' "$counter" "$was" "$is"
                moved=1
            else
                printf 'same   %s: %s\n' "$counter" "$is"
            fi
        done

        # Both readings, not just this one. A difference is only as good as the
        # weaker of the two numbers it is taken between: if the earlier snapshot
        # was written where the kernel log could not be searched, its error
        # count is not a measurement and subtracting from it is arithmetic on a
        # value that was never observed.
        for which in "$2" "$now"; do
            if [ "$(value_of kernel_control "$which")" = "0" ]; then
                printf '\nthe control in %s matched nothing, so its error count\n' "$which"
                printf 'is not evidence of anything and neither is the comparison\n'
                printf 'above. Treat this run as unmeasured.\n'
                exit 2
            fi
        done
        control=$(value_of kernel_control "$now")
        printf 'control: %s lines mention this disk, so the search works\n' "$control"

        if [ "$moved" -eq 1 ]; then
            printf '\nSomething moved. Stop writing to this filesystem and check it\n'
            printf 'before running anything else on this machine.\n'
            exit 1
        fi
        printf '\nNothing moved between the two readings.\n'
        ;;

    *)
        usage
        exit 1
        ;;
esac
