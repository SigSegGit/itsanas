#!/bin/sh
# Bring this machine's containers to the latest published release, so that a
# fleet run tests that release. Asked by the agent (`update`), behind the CI key.
#
#   sh update.sh start     says UPTODATE, or starts the work in the background
#   sh update.sh status    says how the last one went
#
# Why this exists
# ---------------
#
# `itsanas update` does not apply in a container: a Docker machine's version is
# its image's, rebuilt from the checkout. Nothing rebuilt them unless somebody
# did it by hand, so on 2026-11-03 a fleet run after the release of 0.1.16 tested
# 0.1.13 and was green: row V only asked that the machines agree.
#
# What a leaked key can make this do, and nothing more
# ----------------------------------------------------
#
# Bring the machine to the release GitHub calls the latest. The request carries
# no version and no path, and the tag must merge fast-forward, so it cannot pick
# an older or a private build. Already there, it does nothing, so asking again
# restarts nothing; after a failure it does not try that tag again for
# RETRY_AFTER seconds, so asking again does not keep a Pi compiling. It trusts
# GitHub's tag the way `git pull` by hand does, and checks no signature: the
# signed manifest covers binaries, and an image is built from source.
#
# It says only fixed words about what went wrong: a run's output is public, so
# the commands' own output stays in the log beside it.

set -u

HERE=$(dirname "$0")
CONF="${ITSANAS_FLEET_CONF:-$HERE/agent.conf}"
RECEIPTS="$HERE/receipts"
ACCOUNTS=""
CHECKOUT=""
DATA=""
COORDINATOR=0
RELEASES_URL=https://api.github.com/repos/SigSegGit/itsanas/releases/latest
# Long enough for every account's daemon to start and finish a round.
UPDATE_SETTLE=900
UPDATE_POLL=15
RETRY_AFTER=21600
# A build needs room, and the VM's disk has been full before (2026-10-06).
MIN_FREE_KB=3145728
# shellcheck disable=SC1090
. "$CONF"

STATE="$RECEIPTS/update.state"
LOG="$RECEIPTS/update.log"

ran_version() {
    sed -n '1s/.* version \([0-9A-Za-z.+-]*\).*/\1/p' "$DATA/$1/home/status.snapshot" 2>/dev/null
}

every_daemon_runs() {
    for account in $ACCOUNTS; do
        [ "$(ran_version "$account")" = "$1" ] || return 1
    done
    return 0
}

give_up() {
    printf 'UPDATEFAILED %s %s %s\n' "$TAG" "$(date +%s)" "$1" >"$STATE"
    exit 1
}

case "${1:-}" in
    status)
        if [ -r "$STATE" ]; then
            cat "$STATE"
        else
            echo "NOUPDATE no update has run on this machine"
        fi
        exit 0
        ;;
    start)
        TAG=$(curl -fsSL --max-time 30 "$RELEASES_URL" 2>/dev/null |
            sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
        printf '%s' "$TAG" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || {
            echo "UPDATEFAILED could not read the latest release"
            exit 1
        }
        if every_daemon_runs "${TAG#v}"; then
            echo "UPTODATE $TAG"
            exit 0
        fi
        # read leaves nothing behind when the state file is absent.
        read -r was_word was_tag was_time _ 2>/dev/null <"$STATE" || was_word=""
        case "$was_word" in
            UPDATING)
                echo "UPDATING $was_tag"
                exit 0
                ;;
            UPDATEFAILED)
                if [ "$was_tag" = "$TAG" ] && [ $(($(date +%s) - was_time)) -lt "$RETRY_AFTER" ]; then
                    echo "UPDATEFAILED $TAG failed recently and is not tried again yet"
                    exit 1
                fi
                ;;
        esac
        printf 'UPDATING %s %s\n' "$TAG" "$(date +%s)" >"$STATE"
        # On its own, in the background: a Pi compiles for longer than a
        # request lasts, and the caller's connection ending must not stop it.
        nohup sh "$0" run "$TAG" >"$LOG" 2>&1 </dev/null &
        echo "UPDATING $TAG"
        exit 0
        ;;
    run)
        TAG=${2:-}
        ;;
    *)
        echo "usage: update.sh start|status"
        exit 2
        ;;
esac

want=${TAG#v}
docker info >/dev/null 2>&1 || give_up "docker does not answer this user"
free=$(df -Pk "$CHECKOUT" | awk 'NR==2 {print $4}')
[ "${free:-0}" -ge "$MIN_FREE_KB" ] || give_up "under $((MIN_FREE_KB / 1048576)) GB free for the build"

git -C "$CHECKOUT" fetch --quiet --tags origin || give_up "git fetch failed"
git -C "$CHECKOUT" merge --ff-only --quiet "refs/tags/$TAG" || give_up "the checkout does not fast-forward to the tag"
cargo_says=$(sed -n 's/^version = "\(.*\)"/\1/p' "$CHECKOUT/Cargo.toml" | head -1)
[ "$cargo_says" = "$want" ] || give_up "the tag's Cargo.toml says another version"

DOCKER_DIR="$CHECKOUT/install/docker"
if [ "$COORDINATOR" = 1 ]; then
    docker compose -f "$DOCKER_DIR/compose.yml" up -d --build || give_up "the coordinator did not build or start"
fi
# The same commands as fleet-docker.sh: one image, then every account on it.
docker compose -f "$DOCKER_DIR/compose.client.yml" -p itsanas-build build || give_up "the client image did not build"
for account in $ACCOUNTS; do
    env ITSANAS_DATA="$DATA/$account/home" ITSANAS_FOLDER="$HOME/ITSaNAS-$account" ITSANAS_PLEDGES="$DATA/pledges" \
        ITSANAS_ENV_FILE="$DATA/$account/client.env" ITSANAS_UID="$(id -u)" ITSANAS_GID="$(id -g)" \
        docker compose -f "$DOCKER_DIR/compose.client.yml" -p "itsanas-$account" up -d ||
        give_up "the $account container did not start"
done

# A container that started is not a daemon on the new build: only its snapshot
# says that (the laptop, 2026-10-22, on the old program with the new on disk).
waited=0
until every_daemon_runs "$want"; do
    [ "$waited" -lt "$UPDATE_SETTLE" ] || give_up "a daemon does not say it runs the new build"
    sleep "$UPDATE_POLL"
    waited=$((waited + UPDATE_POLL))
done
docker image prune -f >/dev/null 2>&1
printf 'UPDATED %s %s\n' "$TAG" "$(date +%s)" >"$STATE"
