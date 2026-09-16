#!/usr/bin/env bash
# The MVP acceptance tests of docs/MVP.md §3, one phase at a time, each ending
# in a verdict a person does not have to interpret.
#
#   bash scripts/acceptance.sh B write <folder>
#   bash scripts/acceptance.sh B check <folder> <name> <sha256>
#
# Why this exists
# ---------------
#
# Every acceptance test in docs/MVP.md is built, and on 2026-09-14 the verdict of
# §4 had still never been taken: E, F, G and I had never left the laboratory, J
# had no power cut, and D had been "passed" with the 24 words its own criterion
# forbids. What stood between the project and a verdict was not code. It was a
# morning of hands on four machines, each step ending in "does that look right?"
# -- which is the question a tired person answers yes to.
#
# So each phase here checks one thing and prints PASS or FAIL with the numbers,
# and appends the same line to ~/.itsanas-receipts/acceptance.txt. It does not
# orchestrate: moving power, cables and machines stays with the person, because
# that is the part the tests are about.
#
# Every check can fail. `scripts/acceptance-local.sh` runs each phase once where
# it must pass and once where it must not, on every push; a check that cannot
# say FAIL is decoration, and this project has shipped decoration before.
#
# Environment
# -----------
#
#   ITSANAS_BIN       the itsanas binary (default: itsanas on PATH)
#   ITSANAS_HOME      the node, as for itsanas itself
#   ITSANAS_RECEIPTS  where the receipt goes (default: ~/.itsanas-receipts)

set -uo pipefail

BIN=${ITSANAS_BIN:-itsanas}
RECEIPTS=${ITSANAS_RECEIPTS:-$HOME/.itsanas-receipts}

usage() {
    cat <<'EOF'
usage: acceptance.sh <test> <phase> [arguments]

  B write <folder>                     a file of random bytes into the synced folder
  B check <folder> <name> <sha256>     that file, byte for byte, on another machine
  C plant <folder>                     a file carrying a unique canary
  C scan <canary> <host-state-dir>     the canary appears nowhere a host stores
  D check <path> <sha256>              a file of the restored account reads back intact
  E write <folder> / E check ...       as B, for machines never awake together
  F delete <folder> <name>             delete a file from the synced folder
  F check <folder> <name>              it is gone here (run again after another round)
  G edit <folder> <name> <tag>         change a file differently on each machine
  G check <folder> <name>              both versions kept, one of them a conflict copy
  H sample                             one sample of the running daemon (Linux)
  H report                             the samples so far, against the criterion
  I check <daemon-log>                 syncing went on while the coordinator was down
  J count <folder>                     how many files there are, before rebooting
  J check <folder> <count>             same count after, and doctor --deep clean

Run a phase on the machine it is about. Each prints PASS or FAIL and appends the
line to $ITSANAS_RECEIPTS/acceptance.txt.
EOF
    exit 2
}

# One verdict, printed and kept. The exit status is the verdict too, so a
# script driving several phases cannot read a FAIL as success.
verdict() {
    local result=$1 test=$2 phase=$3
    shift 3
    local line
    line=$(printf '%-4s  %s %-6s  %s  [%s %s]' "$result" "$test" "$phase" "$*" \
        "$(uname -n 2>/dev/null || hostname)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)")
    printf '%s\n' "$line"
    mkdir -p "$RECEIPTS" 2>/dev/null && printf '%s\n' "$line" >>"$RECEIPTS/acceptance.txt"
    [ "$result" = PASS ]
}

sha_of() { sha256sum "$1" | cut -d' ' -f1; }

need_dir() { [ -d "$1" ] || { echo "no such directory: $1" >&2; exit 2; }; }

random_bytes() {
    # A mebibyte and a little, so the file is several chunks rather than one --
    # a one-chunk fixture cannot tell a working reassembly from a broken one.
    head -c $((1024 * 1024 + 4099)) /dev/urandom >"$1"
}

write_phase() {
    local test=$1 folder=$2
    need_dir "$folder"
    local name="acceptance-$test-$(date -u +%Y%m%dT%H%M%SZ)-$$.bin"
    random_bytes "$folder/$name"
    verdict PASS "$test" write "$name sha256 $(sha_of "$folder/$name")"
}

check_phase() {
    local test=$1 folder=$2 name=$3 want=$4
    local file="$folder/$name"
    if [ ! -f "$file" ]; then
        verdict FAIL "$test" check "$name is not in $folder"
        return
    fi
    local got
    got=$(sha_of "$file")
    if [ "$got" = "$want" ]; then
        verdict PASS "$test" check "$name identical, sha256 ${got:0:16}"
    else
        verdict FAIL "$test" check "$name differs: expected ${want:0:16}, found ${got:0:16}"
    fi
}

# Searches a directory tree for a fixed string, binary files included. Written
# once and used for both the control and the real scan, so the control proves
# the thing that then finds nothing.
contains() { grep -r -a -F -q -- "$1" "$2" 2>/dev/null; }

c_plant() {
    local folder=$1
    need_dir "$folder"
    local canary name
    canary="ITSANAS-CANARY-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    # The canary is the file's name as well as its content. Test C forbids a
    # host from holding the filename too, and one search for one string then
    # covers both -- a scan for the content alone passed a host that kept names
    # in the clear.
    name="$canary.txt"
    printf 'A private note.\n%s\n' "$canary" >"$folder/$name"
    verdict PASS C plant "$name carries $canary"
}

c_scan() {
    local canary=$1 host=$2
    need_dir "$host"
    # The control first: the same search, on a copy known to contain it. Without
    # it "nothing found" is also what a wrong path or a broken grep prints.
    local control
    control=$(mktemp -d)
    printf 'x%sx' "$canary" >"$control/planted.bin"
    if ! contains "$canary" "$control"; then
        rm -rf "$control"
        verdict FAIL C scan "the control could not find a planted canary, so this search proves nothing"
        return
    fi
    rm -rf "$control"
    if contains "$canary" "$host"; then
        verdict FAIL C scan "the canary is readable under $host: $(grep -r -a -F -l -- "$canary" "$host" | head -3 | tr '\n' ' ')"
    else
        verdict PASS C scan "canary absent from $(du -sh "$host" 2>/dev/null | cut -f1) under $host; control found it"
    fi
}

d_check() {
    local path=$1 want=$2 out
    out=$(mktemp)
    # stderr goes to the terminal as well as the file: `get` asks for the
    # passphrase, and a prompt swallowed into a file reads as a hang. A pipe
    # rather than `2> >(tee ...)`, whose writer bash does not wait for, so the
    # reason read just below could be read before it was written.
    "$BIN" get "$path" "$out" 2>&1 >/dev/null | tee "$out.err" >&2
    if [ "${PIPESTATUS[0]}" -ne 0 ]; then
        local why
        why=$(tail -1 "$out.err")
        rm -f "$out" "$out.err"
        # The verdict last: `return` alone hands back the status of whatever ran
        # before it, and a cleanup that succeeds turned this FAIL into exit 0.
        verdict FAIL D check "$path could not be read: $why"
        return
    fi
    local got
    got=$(sha_of "$out")
    rm -f "$out" "$out.err"
    if [ "$got" = "$want" ]; then
        verdict PASS D check "$path identical, sha256 ${got:0:16}"
    else
        verdict FAIL D check "$path differs: expected ${want:0:16}, found ${got:0:16}"
    fi
}

f_delete() {
    local folder=$1 name=$2
    [ -f "$folder/$name" ] || { verdict FAIL F delete "$name is not in $folder to delete"; return; }
    rm -f "$folder/$name"
    verdict PASS F delete "$name removed from $folder"
}

f_check() {
    local folder=$1 name=$2
    need_dir "$folder"
    if [ -e "$folder/$name" ]; then
        verdict FAIL F check "$name is still in $folder (or came back)"
        return
    fi
    # A delete that takes a neighbour with it is the other half of the failure.
    local left
    left=$(find "$folder" -type f | wc -l | tr -d ' ')
    verdict PASS F check "$name absent; $left other files still here. Run again after the next round: it must stay absent"
}

g_edit() {
    local folder=$1 name=$2 tag=$3
    [ -f "$folder/$name" ] || { verdict FAIL G edit "$name is not in $folder"; return; }
    printf '\nedited on %s: %s\n' "$(uname -n)" "$tag" >>"$folder/$name"
    verdict PASS G edit "$name edited with tag $tag"
}

g_check() {
    local folder=$1 name=$2
    local dir base stem ext
    dir=$(dirname "$folder/$name")
    base=$(basename "$name")
    case "$base" in
        .*) stem=$base; ext= ;;
        *.*) stem=${base%.*}; ext=.${base##*.} ;;
        *) stem=$base; ext= ;;
    esac
    [ -f "$dir/$base" ] || { verdict FAIL G check "$base itself is missing from $dir"; return; }
    local siblings
    siblings=$(find "$dir" -maxdepth 1 -type f -name "$stem.conflict-*$ext" | sort)
    if [ -z "$siblings" ]; then
        verdict FAIL G check "no $stem.conflict-*$ext beside $base: one edit overwrote the other"
        return
    fi
    local distinct
    distinct=$( { sha_of "$dir/$base"; for s in $siblings; do sha_of "$s"; done; } | sort -u | wc -l | tr -d ' ')
    if [ "$distinct" -lt 2 ]; then
        verdict FAIL G check "a conflict copy exists but holds the same bytes as $base: one version was lost"
        return
    fi
    # One line both machines print, so "they agree" is a comparison of two
    # strings rather than of two directory listings read by eye.
    local digest
    digest=$( { printf '%s %s\n' "$base" "$(sha_of "$dir/$base")"; for s in $siblings; do printf '%s %s\n' "$(basename "$s")" "$(sha_of "$s")"; done; } | sha256sum | cut -c1-16)
    verdict PASS G check "$base and $(printf '%s\n' "$siblings" | wc -l | tr -d ' ') conflict copy, $distinct distinct versions, agreement digest $digest"
}

h_sample() {
    local pid
    pid=$(pgrep -f "itsanas.* daemon" | head -1)
    [ -n "$pid" ] || { verdict FAIL H sample "no itsanas daemon is running"; return; }
    [ -r "/proc/$pid/io" ] || { verdict FAIL H sample "/proc/$pid/io is unreadable; Linux only"; return; }
    local cpu rss written files
    cpu=$(ps -o %cpu= -p "$pid" | tr -d ' ')
    rss=$(ps -o rss= -p "$pid" | tr -d ' ')
    written=$(awk '/^write_bytes/ {print $2}' "/proc/$pid/io")
    # The criterion is "memory under 200 MiB **with a large folder**", and until
    # 2026-09-16 no kit recorded the folder -- so a PASS on an empty account was
    # indistinguishable from a PASS on a full one, and MVP.md's own figures came
    # from about a megabyte. `itsanas status` answers without a passphrase while
    # the daemon holds the node, so ask it. `unknown` rather than blank when it
    # cannot, because a blank column reads as zero. An older binary that still
    # demands a passphrase gives `unknown`, which is the honest answer.
    files=$("${ITSANAS_BIN:-itsanas}" status 2>/dev/null |
        awk '/^[[:space:]]*files[[:space:]]+[0-9]+/ {print $2; exit}')
    [ -n "$files" ] || files=unknown
    mkdir -p "$RECEIPTS"
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date +%s)" "$cpu" "$rss" "$written" "$files" >>"$RECEIPTS/h-samples.tsv"
    echo "sampled pid $pid: cpu ${cpu}% rss ${rss} KiB written ${written} B, account holds $files file(s)"
}

h_report() {
    local file="$RECEIPTS/h-samples.tsv"
    [ -s "$file" ] || { verdict FAIL H report "no samples in $file"; return; }
    # The criterion in MVP.md is partly a feeling ("no perceptible effect"); the
    # thresholds below are the kit's reading of it, stated so it can be argued
    # with: under 200 MiB resident, under 5% of a core on average, and the
    # write rate reported rather than judged, since the criterion names none.
    # Field 5 is the account's file count, added 2026-09-16. It is reported and
    # never judged -- the criterion says "a large folder" without saying how
    # large, so a threshold here would be invented. What the receipt must carry
    # is what was actually measured, so a PASS taken on an empty account cannot
    # later be read as evidence for a full one. Samples written before that
    # field existed, or by a binary whose `status` still wants a passphrase,
    # have no count and say so.
    awk -F'\t' '
        NR == 1 { t0 = $1; w0 = $4 }
        {
            n++; cpu += $2; if ($3 > peak) peak = $3; t1 = $1; w1 = $4
            if ($5 != "" && $5 != "unknown") {
                seen++
                if (seen == 1 || $5 < lo) lo = $5
                if (seen == 1 || $5 > hi) hi = $5
            }
        }
        END {
            hours = (t1 - t0) / 3600
            perday = (t1 > t0) ? (w1 - w0) * 86400 / (t1 - t0) / 1048576 : 0
            if (seen == 0) files = "none"
            else if (lo == hi) files = lo
            else files = lo "-" hi
            printf "%d %.2f %.1f %.1f %.1f %s\n", n, cpu / n, peak / 1024, hours, perday, files
        }' "$file" | {
        read -r n avg peak hours perday files
        local size
        if [ "$files" = none ]; then
            size='account size NOT recorded, so this says nothing about "with a large folder"'
        else
            size="on an account of $files file(s)"
        fi
        local detail="$n samples over ${hours} h: cpu ${avg}% avg, peak ${peak} MiB $size, ${perday} MiB written/day"
        if awk "BEGIN { exit !($hours < 24) }"; then
            verdict FAIL H report "$detail -- fewer than the 24 hours the test asks for"
        elif awk "BEGIN { exit !($peak < 200 && $avg < 5) }"; then
            verdict PASS H report "$detail"
        else
            verdict FAIL H report "$detail -- over 200 MiB or 5% of a core"
        fi
    }
}

i_check() {
    local log=$1
    [ -f "$log" ] || { verdict FAIL I check "no such log: $log"; return; }
    local down after
    down=$(grep -n -E 'coordinator: (still )?unreachable' "$log" | head -1 | cut -d: -f1)
    [ -n "$down" ] || { verdict FAIL I check "the log never reports the coordinator unreachable: it was not down, or the daemon did not say so"; return; }
    after=$(tail -n +"$down" "$log" | grep -c -E 'received [0-9]+ files')
    if [ "$after" -gt 0 ]; then
        verdict PASS I check "coordinator reported unreachable at line $down, and $after sync rounds completed after it"
    else
        # A round with nothing to move prints nothing -- the daemon went quiet on
        # purpose -- so an outage with no writes looks like no sync at all.
        verdict FAIL I check "coordinator reported unreachable at line $down, and no round moved anything after it. Idle rounds print nothing: write a file on another machine during the outage and check again"
    fi
}

j_count() {
    need_dir "$1"
    verdict PASS J count "$(find "$1" -type f | wc -l | tr -d ' ') files in $1"
}

j_check() {
    local folder=$1 want=$2 got
    need_dir "$folder"
    got=$(find "$folder" -type f | wc -l | tr -d ' ')
    if [ "$got" != "$want" ]; then
        verdict FAIL J check "$got files in $folder, $want before the reboot"
        return
    fi
    local report
    report=$(mktemp)
    if "$BIN" doctor --deep >"$report" 2>&1; then
        rm -f "$report"
        verdict PASS J check "$got files, as before; doctor --deep clean"
    else
        local why
        why=$(tail -1 "$report")
        rm -f "$report"
        verdict FAIL J check "$got files, but doctor --deep failed: $why"
    fi
}

[ $# -ge 2 ] || usage
test=$1 phase=$2
shift 2

case "$test $phase $#" in
    "B write 1" | "E write 1") write_phase "$test" "$1" ;;
    "B check 3" | "E check 3") check_phase "$test" "$1" "$2" "$3" ;;
    "C plant 1") c_plant "$1" ;;
    "C scan 2") c_scan "$1" "$2" ;;
    "D check 2") d_check "$1" "$2" ;;
    "F delete 2") f_delete "$1" "$2" ;;
    "F check 2") f_check "$1" "$2" ;;
    "G edit 3") g_edit "$1" "$2" "$3" ;;
    "G check 2") g_check "$1" "$2" ;;
    "H sample 0") h_sample ;;
    "H report 0") h_report ;;
    "I check 1") i_check "$1" ;;
    "J count 1") j_count "$1" ;;
    "J check 2") j_check "$1" "$2" ;;
    *) usage ;;
esac
