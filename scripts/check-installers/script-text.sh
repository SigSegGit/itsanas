# Sourced by scripts/check-installers.sh, in its shell and in its order:
# what the scripts say: the Rust version, command substitution in a heredoc, reading a reply.

# ---------------------------------------------------------- the Rust version

msrv=$(grep -m1 '^rust-version' Cargo.toml | cut -d'"' -f2)
if [ -z "$msrv" ]; then
    bad "could not read rust-version from Cargo.toml"
else
    major=${msrv%%.*}
    minor=${msrv#*.}
    minor=${minor%%.*}
    stated=0

    for script in "${SH_SCRIPTS[@]}"; do
        got_major=$(grep -m1 '^MIN_RUST_MAJOR=' "$script" | cut -d= -f2)
        # A script that does not build anything states no version, and that is
        # correct rather than missing: install/coordinator.sh installs a binary
        # somebody else compiled.
        [ -z "$got_major" ] && continue
        stated=$((stated + 1))
        got_minor=$(grep -m1 '^MIN_RUST_MINOR=' "$script" | cut -d= -f2)
        if [ "$got_major" = "$major" ] && [ "$got_minor" = "$minor" ]; then
            say "$script wants Rust $got_major.$got_minor, same as Cargo.toml"
        else
            bad "$script wants Rust $got_major.$got_minor but Cargo.toml says $msrv"
        fi
    done

    for script in "${PS_SCRIPTS[@]}"; do
        got_major=$(grep -m1 '^\$MinRustMajor' "$script" | tr -dc '0-9')
        [ -z "$got_major" ] && continue
        stated=$((stated + 1))
        got_minor=$(grep -m1 '^\$MinRustMinor' "$script" | tr -dc '0-9')
        if [ "$got_major" = "$major" ] && [ "$got_minor" = "$minor" ]; then
            say "$script wants Rust $got_major.$got_minor, same as Cargo.toml"
        else
            bad "$script wants Rust $got_major.$got_minor but Cargo.toml says $msrv"
        fi
    done

    if [ "$stated" -eq 0 ]; then
        bad "no installer states a minimum Rust version"
        say "  If they all stopped building, say so here; otherwise one has lost its check."
    fi
fi

# ------------------------------------------- command substitution in a heredoc
#
# `install/linux.sh` writes the systemd unit with an unquoted heredoc, because
# the unit needs $BIN_DIR. Everything inside is therefore expanded — including
# the comments, and one of them explained the leading dash on ReadWritePaths
# with the phrase "created by `itsanas init`". The shell ran it.
#
# The result on a real machine: a clap usage error printed into the middle of
# the install, and a unit file reading "~/.itsanas is created by , so enabling
# the service". The command happened to require an argument and produce nothing.
# Had it printed anything, that would have gone into a systemd unit.
#
# Found by running the one-liner the script advertises, not by reading it.
# An escaped backtick is fine — macos.sh uses several on purpose — so escapes
# are removed before looking.

for script in "${SH_SCRIPTS[@]}"; do
    offenders=$(awk '
        state == 1 {
            if ($0 == term) { state = 0; next }
            line = $0
            gsub(/\\./, "", line)
            if (line ~ /`/ || line ~ /\$\(/) printf "%d: %s\n", FNR, $0
            next
        }
        state == 2 { if ($0 == term) state = 0; next }
        match($0, /<<-?[^ \t;|&<]+/) {
            w = substr($0, RSTART, RLENGTH)
            sub(/^<<-?/, "", w)
            state = (w ~ /^["\x27\\]/) ? 2 : 1
            gsub(/["\x27\\]/, "", w)
            term = w
        }
    ' "$script")
    if [ -n "$offenders" ]; then
        bad "$script expands a command inside an unquoted heredoc:"
        printf '%s\n' "$offenders" | sed 's/^/       /'
        say "  Everything in an unquoted heredoc is expanded, comments too."
        say "  Escape it, or quote the delimiter if nothing in it needs expanding."
    fi
done
say "no installer runs a command inside a heredoc by accident"

# ------------------------------------------------------------ reading a reply
#
# `install/linux.sh` advertises `curl -fsSL … | sh` on its fourth line. Under
# that pipe, **stdin is the script**: a bare `read` consumes the lines the shell
# has not executed yet, and the shell then runs a truncated program. Every
# prompt in these scripts did exactly that, so the entry point the file offers a
# stranger would have destroyed itself at the first question.
#
# `< /dev/tty` reads the terminal whatever stdin happens to be.

for script in "${SH_SCRIPTS[@]}"; do
    # `read` in *command position*: at the start of a statement, or after a
    # separator. Matching the word anywhere flagged the sentence "anything
    # running as you can read that file" and the message "cannot read
    # $PHRASE_FILE" -- and the answer to that is a tighter pattern, not a
    # longer list of phrases to forgive. That list grows every time somebody
    # writes a new sentence containing a common English verb.
    # Two shapes, and prose matches neither: `read` carrying a flag, which is
    # every careful use because POSIX says always pass -r; or `read` naming a
    # single variable at the end of a line. "anything running as you can read
    # that file" has words after it; "read this file. -->" likewise.
    bare=$(grep -nE 'read[[:space:]]+(-|[A-Za-z_][A-Za-z0-9_]*[[:space:]]*$)' "$script" |
        grep -vE '^[0-9]+:[[:space:]]*#' |
        grep -v '/dev/tty' || true)
    if [ -n "$bare" ]; then
        bad "$script reads a reply from standard input:"
        printf '%s\n' "$bare" | sed 's/^/         /'
        say "  Piped to sh, stdin is the script. Use: read -r x < /dev/tty"
    fi

    # And the wrong way to find out whether there is a terminal to read from.
    # `[ -r /dev/tty ]` reads the permission bits of a device node that exists
    # whether or not the process has a controlling terminal, so it says yes and
    # the open then fails with ENXIO. Found on the Freebox VM: the script asked
    # its question, the read failed, and `set -u` took it out on
    # "reply: parameter not set" instead of the sentence written for the case.
    # Comments are skipped, because the comment above each `confirm` quotes the
    # wrong form in order to explain it — and the first version of this check
    # failed the clean tree on its own explanation.
    tested=$(grep -nE '\[ *!? *-[rwe] +/dev/tty *\]' "$script" |
        grep -vE '^[0-9]+: *#' || true)
    if [ -n "$tested" ]; then
        bad "$script tests /dev/tty instead of opening it"
        printf '%s\n' "$tested" | sed 's/^/         /'
        say '  Use:  if ! { : < /dev/tty; } 2>/dev/null; then'
    fi
done
say "no installer reads a reply from standard input, or tests /dev/tty by hand"
