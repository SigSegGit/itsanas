# Sourced by scripts/check-installers.sh, in its shell and in its order:
# what an installer refuses: no terminal, an unsupported CPU, a sudo rule that does not match, a reserved name.

# --------------------------------------------- refusing without a terminal
#
# The branch that asks a question, run where there is nobody to ask: no tty, no
# --yes, and a prerequisite missing. It must say so and exit 1.
#
# It killed the shell instead, on a Raspberry Pi 4 running Debian. The guard was
# `{ : < /dev/tty; } 2>/dev/null`, and `:` is a POSIX *special built-in*: a
# redirection error on one makes a non-interactive shell exit. dash obeys that
# rule and bash does not, so the script died with status 2 and no output on
# exactly the platform its own header says it is written for, while passing
# every test on bash.
#
# The path had never executed anywhere. It needs Rust missing *and* --yes
# absent, and every earlier run had one or the other. So this runs it, under
# both shells, with $HOME pointed somewhere empty so the script cannot find a
# toolchain to be satisfied by.

probe_no_terminal() {
    shell=$1
    empty=$(mktemp -d)
    # `setsid`, and it is the whole point: redirecting stdin from /dev/null is
    # not the same as having no terminal. A process whose parent has one can
    # still open /dev/tty, so the first version of this probe reached the
    # prompt, blocked on `read`, and hung the check for ten minutes. setsid
    # detaches from the controlling terminal, which is the state an `ssh host
    # 'command'` and a CI step are actually in.
    #
    # Without --no-build, deliberately: --no-build now reports a missing
    # toolchain instead of offering to install one, so it never reaches the
    # question. Probing it would test the wrong branch and pass while the real
    # one is broken -- which is what the version before this one did.
    env="HOME=$empty PATH=/usr/bin:/bin"
    # One line, because the continuation that was here got eaten by the script
    # that wrote it -- caught by check-messages.py in the same minute, which is
    # the fifth time today and the first time it was caught before a push.
    # shellcheck disable=SC2086
    output=$(env $env timeout 60 setsid "$shell" install/linux.sh 2>&1 </dev/null)
    status=$?
    rm -rf "$empty"

    if [ "$status" -eq 124 ]; then
        bad "install/linux.sh under $shell hung with no terminal to ask on"
        say "  It is waiting for an answer nobody can give."
        return
    fi

    if [ "$status" -eq 0 ]; then
        bad "install/linux.sh under $shell says a machine with no Rust is ready"
    elif [ -z "$output" ]; then
        bad "install/linux.sh under $shell exited $status without saying anything"
        say "  A silent non-zero exit is a shell error, not a refusal. dash exits"
        say "  2 on a redirection error against a special built-in."
    elif [ "$status" -ne 1 ]; then
        bad "install/linux.sh under $shell exited $status, not 1"
        printf '%s\n' "$output" | tail -3 | sed 's/^/         /'
    else
        say "install/linux.sh under $shell refuses with a message and exit 1"
    fi
}

if PATH=/usr/bin:/bin command -v rustc >/dev/null 2>&1; then
    say "rustc is in /usr/bin here, so the no-toolchain path cannot be probed"
elif ! command -v setsid >/dev/null 2>&1 || ! command -v timeout >/dev/null 2>&1; then
    say "setsid or timeout is missing here; the no-terminal path was not probed"
else
    for shell in sh dash bash; do
        command -v "$shell" >/dev/null 2>&1 && probe_no_terminal "$shell"
    done
fi

# ------------------------------------------------------- what it refuses to do
#
# The one branch of an installer that has to be right on a machine nobody here
# owns is the one that refuses. `install/linux.sh` gets its answer from
# `uname -m`, so a fake `uname` earlier on PATH exercises every case.
#
# This found a real hole. The list was `armv7l|armv6l`, and a 64-bit Raspberry
# Pi running a 32-bit userland reports **armv8l** — the exact case the error
# message describes, waved through with "untested architecture" and left to fail
# an hour into the build. 32-bit x86 was let through too, with the same 64-bit
# counters the message cites as the reason.

fake=$(mktemp -d)
cat > "$fake/uname" <<'FAKEUNAME'
#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = "-m" ]; then
        echo "$FAKE_ARCH"
        exit 0
    fi
done
exec /usr/bin/uname "$@"
FAKEUNAME
chmod +x "$fake/uname"

# Does the harness work at all here? The probe below shadows `uname` on PATH and
# reads what the installer says about it, which needs a shell that resolves
# commands through PATH the way a POSIX shell does. Under Git Bash on Windows it
# does not, and the result was seven confident FAILs saying `install/linux.sh`
# refuses nothing -- an accusation against the installer for something the test
# harness could not do.
#
# The first attempt at this control asked the fake `uname` to identify itself,
# and it did -- the harness was fine. The installer was refusing earlier, on
# `uname -s`: "unsupported system: MINGW64_NT-10.0-26200", before it ever
# reached the architecture. A control has to test the thing that can be wrong,
# and the thing that was wrong was not PATH.
#
# So: run the installer with the fake reporting a *supported* architecture. If
# it still refuses the system, the probes below cannot reach the code they test,
# and "not checked here" is the only honest report. A check whose apparatus
# cannot run reports zero findings, and zero findings from a broken apparatus
# reads exactly like a clean bill of health.
control=$(FAKE_ARCH="x86_64" PATH="$fake:$PATH" sh install/linux.sh --no-build 2>&1)
case "$control" in
    *"unsupported system"*)
        say "install/linux.sh refuses this operating system before it looks at the"
        say "  architecture, so the 32-bit probes cannot reach the code they test."
        say "  Not run here; CI runs them on Linux."
        rm -rf "$fake"
        fake=""
        ;;
esac

refused=0
for arch in ${fake:+armv6l armv7l armv8l armhf arm i386 i686}; do
    output=$(FAKE_ARCH="$arch" PATH="$fake:$PATH" sh install/linux.sh --no-build 2>&1)
    if printf '%s' "$output" | grep -q "is not supported"; then
        refused=$((refused + 1))
    else
        bad "install/linux.sh does not refuse a 32-bit userland reporting $arch"
        say "  A 64-bit machine running a 32-bit image is the common case, not"
        say "  an exotic one, and the build fails an hour later without this."
    fi
done
if [ -n "$fake" ] && [ "$refused" -eq 7 ]; then
    say "install/linux.sh refuses all 7 spellings of a 32-bit userland"
fi

for arch in aarch64 arm64 x86_64; do
    # It may still stop later for want of a compiler on this machine, so look
    # for the line rather than the exit status.
    output=$(FAKE_ARCH="$arch" PATH="$fake:$PATH" sh install/linux.sh --no-build 2>&1)
    if printf '%s' "$output" | grep -q "is not supported"; then
        bad "install/linux.sh refuses $arch, which is one of its targets"
    fi
done
say "install/linux.sh accepts aarch64, arm64 and x86_64"
[ -n "$fake" ] && rm -rf "$fake"

# ------------------------------------------------- the passwordless sudo rule
#
# sudo matches arguments exactly. A command written in install/README.md in any
# form other than the one install/sudoers-itsanas allows asks for a password,
# which on an agent's SSH session means a hung command or a refusal -- and the
# README is where an agent copies the command from. So every `sudo -n` line in
# the README must be a command the rule lists, word for word; and the rule must
# name the service and the binary path coordinator.sh actually installs, or it
# permits operating something that does not exist.

rule=install/sudoers-itsanas
if [ ! -f "$rule" ]; then
    bad "$rule is missing; install/README.md tells people to install it"
else
    allowed=$(sed -n 's/^[[:space:]]*\(\/usr\/bin\/[^,\\]*[^,\\[:space:]]\).*$/\1/p' "$rule")
    if [ -z "$allowed" ]; then
        bad "$rule lists no commands this check can read"
    fi
    readme_cmds=$(sed -n 's/^sudo -n \(\/usr\/bin\/.*[^[:space:]]\)[[:space:]]*$/\1/p' install/README.md)
    if [ -z "$readme_cmds" ]; then
        bad "install/README.md shows no sudo -n command; the rule has no documented use"
    fi
    while IFS= read -r cmd; do
        [ -z "$cmd" ] && continue
        if printf '%s\n' "$allowed" | grep -qxF -- "$cmd"; then
            say "sudoers-itsanas allows: sudo -n ${cmd%% itsanas-coordinator*}..."
        else
            bad "install/README.md runs 'sudo -n $cmd', which $rule does not allow"
            say "  sudo compares arguments exactly; this would ask for a password."
        fi
    done <<< "$readme_cmds"
    if ! grep -q 'BIN_DST="/usr/local/bin/itsanas-coordinator"' install/coordinator.sh \
        || ! printf '%s\n' "$allowed" | grep -q ' /usr/local/bin/itsanas-coordinator$'; then
        bad "$rule and install/coordinator.sh disagree about where the binary lives"
    fi
    if ! grep -q 'itsanas-coordinator.service' install/coordinator.sh; then
        bad "install/coordinator.sh no longer installs itsanas-coordinator.service, which $rule operates"
    fi
fi

# ------------------------------------------------ a name that is not a node
#
# `~/.itsanas-passphrase` is the default node's passphrase *file*, and
# `--instance NAME` means `~/.itsanas-NAME`. The CLI refused `passphrase` from
# 0p; the shell scripts accepted it and aimed provision's mkdir, and clean's
# plan, at that file. Both must refuse, and leave the file alone.

reserved_home=$(mktemp -d)
printf 'secret\n' > "$reserved_home/.itsanas-passphrase"
for script in install/clean.sh install/provision.sh; do
    case "$script" in
        *clean.sh) args="--yes --purge-account" ;;
        *) args="--username u --no-install" ;;
    esac
    # shellcheck disable=SC2086
    out=$(env HOME="$reserved_home" PATH=/usr/bin:/bin ITSANAS_PASSPHRASE=a-test-passphrase \
        timeout 30 sh "$script" $args --instance passphrase 2>&1 </dev/null)
    status=$?
    if [ "$status" -eq 0 ] || ! printf '%s' "$out" | grep -q 'reserved'; then
        bad "$script --instance passphrase is not refused as reserved (exit $status)"
        say "  ~/.itsanas-passphrase is the default node's passphrase file, not an instance."
    elif [ "$(cat "$reserved_home/.itsanas-passphrase" 2>/dev/null)" != secret ]; then
        bad "$script --instance passphrase touched ~/.itsanas-passphrase"
    else
        say "$script refuses --instance passphrase"
    fi
done
rm -rf "$reserved_home"
