#!/usr/bin/env bash
# Put a Linux machine of the fleet on Docker: remove every native ITSaNAS
# install, then run the coordinator (vm) and one container per account, each
# restarted at boot unless it was stopped by hand (`restart: unless-stopped`).
#
#   curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/scripts/fleet-docker.sh -o ~/fleet-docker.sh && bash ~/fleet-docker.sh vm
#   ... && bash ~/fleet-docker.sh pi     # the other Linux machines, once the VM is done
#
# Why Docker on these two (Nicolas, 2026-10-14): they also run a VPN, home
# automation and other services, so each ITSaNAS process gets its own memory
# and CPU cap (install/docker/compose*.yml) and nothing it does reaches the rest.
#
# Every account is *joined* from its 24 words, typed at the container's own
# prompt: the accounts already exist (created on the VM on 2026-10-14). Each
# account has its own passphrase, kept in its own mode-600 env file, as three
# people sharing a machine would have.
#
# Safe to run again: an account whose node exists is left as it is and only
# brought up. The native install is removed every time, which is the point.
set -u

ROLE="${1:-}"
case "$ROLE" in
    vm|pi) ;;
    *) echo "usage: bash fleet-docker.sh vm|pi   (vm = the coordinator's machine, first)"; exit 2 ;;
esac

ACCOUNTS="nicolas test mandarine"
COORD=itsanas.ngas.fr:9898
COORD_ID=2cfb515fc90749d7248f4af404ecb34b5417e0c1339565c8036a9dde70cf72ce
REPO=https://github.com/SigSegGit/itsanas.git
CHECKOUT="$HOME/itsanas"
DATA="$HOME/itsanas-data"
COORD_STATE=/var/lib/itsanas-coordinator

say() { printf '%s\n' "$*"; }
fail() { say "FAILED: $*"; exit 1; }

command -v git >/dev/null || fail "git is not installed (sudo apt-get install -y git)"
if docker info >/dev/null 2>&1; then SUDO=""; else SUDO="sudo"; fi
$SUDO docker compose version >/dev/null 2>&1 || fail "docker compose is not available"

say; say "== 1/5 Removing every native ITSaNAS install"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/clean.sh -o "$WORK/clean.sh" \
    || fail "download clean.sh"
for d in "$HOME"/.itsanas-*; do
    [ -d "$d" ] || continue
    sh "$WORK/clean.sh" --yes --purge-account --purge-folder --drain-for 0 --without-passphrase --instance "${d##*/.itsanas-}" </dev/null >/dev/null
done
sh "$WORK/clean.sh" --yes --purge-account --purge-folder --drain-for 0 --without-passphrase </dev/null >/dev/null
rm -f "$HOME/fleet-reset.sh" "$HOME/provision.sh" "$HOME/itsanas-reset.sh" "$HOME"/*mots*.txt
# The systemd coordinator is root's, so clean.sh run as this user leaves it.
if [ -e /etc/systemd/system/itsanas-coordinator.service ] || [ -e /usr/local/bin/itsanas-coordinator ]; then
    sudo systemctl disable --now itsanas-coordinator itsanas-coordinator-update.timer 2>/dev/null
    sudo rm -f /etc/systemd/system/itsanas-coordinator.service \
        /etc/systemd/system/itsanas-coordinator-update.service \
        /etc/systemd/system/itsanas-coordinator-update.timer \
        /usr/local/bin/itsanas-coordinator
    sudo systemctl daemon-reload
    say "  native coordinator removed"
fi
if [ "$ROLE" != vm ] && [ -d "$COORD_STATE" ]; then
    sudo rm -rf "$COORD_STATE"
    say "  an old coordinator's state on this machine removed"
fi
# Earlier Docker installs: containers this script does not run (an older
# compose with fixed names, as on the Pi on 2026-10-14) and the data they
# kept beside the compose file. Ours are left running on a re-run.
keep="^itsanas-coordinator$|^itsanas-(nicolas|test|mandarine)-client-1$"
for name in $($SUDO docker ps -a --format '{{.Names}}' 2>/dev/null | grep -i itsanas | grep -Ev "$keep"); do
    $SUDO docker rm -f "$name" >/dev/null && say "  old container $name removed"
done
for old in "$HOME"/itsanas/install/docker/data* "$HOME"/itsanas/install/docker/ITSaNAS* "$HOME"/itsanas/install/docker/client.env; do
    [ -e "$old" ] || continue
    rm -rf "$old" && say "  old Docker data $old removed"
done
say "  ok: no native node, service or binary left"

say; say "== 2/5 The checkout the images are built from ($CHECKOUT)"
if [ -d "$CHECKOUT/.git" ]; then
    git -C "$CHECKOUT" pull --ff-only -q || fail "git pull in $CHECKOUT"
else
    git clone -q "$REPO" "$CHECKOUT" || fail "git clone"
fi
DOCKER_DIR="$CHECKOUT/install/docker"

say; say "== 3/5 The coordinator"
if [ "$ROLE" = vm ]; then
    sudo test -f "$COORD_STATE/device.key" || fail "$COORD_STATE/device.key is missing: the pinned identity would be lost"
    sudo chown -R 65532:65532 "$COORD_STATE"
    $SUDO docker compose -f "$DOCKER_DIR/compose.yml" up -d --build || fail "coordinator build or start"
    sleep 3
    if $SUDO docker logs itsanas-coordinator 2>&1 | grep -q "$COORD_ID"; then
        say "  ok: running in Docker, identity $COORD_ID"
    else
        say "  STOP: the coordinator's log does not show the pinned identity:"
        $SUDO docker logs --tail 20 itsanas-coordinator
        exit 1
    fi
else
    say "  not on this machine (the VM runs it)"
fi

# Without the memory controller every cap in the compose files is dropped
# ("Your kernel does not support memory limit capabilities", the Pi on
# 2026-10-14): Raspberry Pi OS ships it off. Turned on here, once; it takes
# effect at the next boot, and the containers come back by themselves.
REBOOT_FOR_LIMITS=0
if [ -r /sys/fs/cgroup/cgroup.controllers ] && ! grep -qw memory /sys/fs/cgroup/cgroup.controllers; then
    for cmdline in /boot/firmware/cmdline.txt /boot/cmdline.txt; do
        [ -f "$cmdline" ] || continue
        if ! grep -q 'cgroup_memory=1' "$cmdline"; then
            sudo sed -i '1 s/$/ cgroup_enable=memory cgroup_memory=1/' "$cmdline"
            say "  memory limits enabled in $cmdline (they apply after a reboot)"
        fi
        REBOOT_FOR_LIMITS=1
        break
    done
fi

say; say "== 4/5 The client image (a Raspberry Pi takes a while: it compiles)"
$SUDO docker compose -f "$DOCKER_DIR/compose.client.yml" -p itsanas-build build || fail "client image build"

# The members listen on the host's network; ufw filters that like any process.
if command -v ufw >/dev/null && sudo ufw status 2>/dev/null | grep -q "Status: active"; then
    sudo ufw allow 9797:9799/tcp comment 'itsanas members' >/dev/null
    sudo ufw allow 9797:9799/udp comment 'itsanas discovery' >/dev/null
    say "  ufw: 9797-9799 opened for the members"
fi

say; say "== 5/5 The accounts: $ACCOUNTS"
mkdir -p "$DATA/pledges"
port=9797
for acct in $ACCOUNTS; do
    say; say "---------- account $acct (port $port) ----------"
    home="$DATA/$acct/home"
    envf="$DATA/$acct/client.env"
    folder="$HOME/ITSaNAS-$acct"
    mkdir -p "$home" "$folder"
    dc() {
        $SUDO env ITSANAS_DATA="$home" ITSANAS_FOLDER="$folder" ITSANAS_PLEDGES="$DATA/pledges" \
            ITSANAS_ENV_FILE="$envf" ITSANAS_UID="$(id -u)" ITSANAS_GID="$(id -g)" \
            docker compose -f "$DOCKER_DIR/compose.client.yml" -p "itsanas-$acct" "$@"
    }
    if [ -f "$home/keystore.bin" ]; then
        say "  the node exists; leaving it as it is"
    else
        while :; do
            read -r -s -p "Passphrase for '$acct' on this machine: " pass; echo
            read -r -s -p "Again: " pass2; echo
            [ -n "$pass" ] && [ "$pass" = "$pass2" ] && break
            say "They differ or are empty; again."
        done
        (umask 077; printf 'ITSANAS_PASSPHRASE=%s\n' "$pass" > "$envf")
        unset pass pass2
        say "  Now the 24 words of '$acct', at the prompt below (one line, spaces, no numbers):"
        dc run --rm -it client login --username "$acct" || fail "login $acct"
        dc run --rm client listen "0.0.0.0:$port" || fail "listen $acct"
        dc run --rm client folder /data/folder || fail "folder $acct"
        dc run --rm client coordinator "$COORD" --device "$COORD_ID" || fail "coordinator $acct"
        dc run --rm client register || fail "register $acct (is the coordinator reachable?)"
    fi
    dc up -d || fail "start $acct"
    echo "bonjour de $(hostname) pour $acct, $(date)" > "$folder/bonjour-$(hostname).txt"
    port=$((port + 1))
done

say; say "== Done. One id per account; it must be the same on every machine:"
# Read from the container's log rather than asked: the running daemon holds
# the node, so `whoami` beside it is refused.
for acct in $ACCOUNTS; do
    printf '%-10s ' "$acct"
    $SUDO docker logs "itsanas-$acct-client-1" 2>&1 | sed -n 's/^ *user id *//p' | head -1
done
$SUDO docker ps --filter name=itsanas --format '  {{.Names}}  {{.Status}}'

# Which build each daemon runs, from its snapshot: a re-run whose pull or
# build failed left the old daemons up, and nothing here said so (the VM,
# 2026-10-27, seen only in the next fleet run). The checkout's version is
# what they must say.
want=$(sed -n 's/^version = "\(.*\)"/\1/p' "$CHECKOUT/Cargo.toml" | head -1)
say; say "== The build each account's daemon runs (the checkout says $want):"
sleep 5
for acct in $ACCOUNTS; do
    ran=$(sed -n '1s/.* version \([0-9A-Za-z.+-]*\).*/\1/p' "$DATA/$acct/home/status.snapshot" 2>/dev/null)
    if [ "$ran" = "$want" ]; then
        printf '  %-10s %s\n' "$acct" "$ran"
    else
        printf '  %-10s %s   <- NOT the checkout'"'"'s build: look at the build step above\n' "$acct" "${ran:-unknown (0.1.12 or before, or no snapshot yet)}"
    fi
done
if [ "$REBOOT_FOR_LIMITS" -eq 1 ]; then
    say; say "  REBOOT this machine (sudo reboot) so the containers' memory limits apply;"
    say "  they restart by themselves (restart: unless-stopped)."
fi
