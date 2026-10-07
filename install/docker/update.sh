#!/bin/sh
# Pull this checkout's main (fast-forward only) and rebuild the container if
# it moved. For cron or a systemd timer, as the checkout's owner (in the
# docker group): 0 4 * * * sh ~/itsanas/install/docker/update.sh
set -eu
cd "$(dirname -- "$0")/../.."
before=$(git rev-parse HEAD)
git fetch --quiet origin main
git merge --ff-only --quiet origin/main
[ "$before" != "$(git rev-parse HEAD)" ] || exit 0
docker compose -f install/docker/compose.yml up -d --build
docker image prune -f >/dev/null
echo "itsanas-coordinator rebuilt at $(git rev-parse --short HEAD)"
