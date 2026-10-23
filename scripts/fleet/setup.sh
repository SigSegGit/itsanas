#!/usr/bin/env bash
# Put the fleet agent on this machine (the Pi or the VM), once, as the user
# whose containers run the accounts (scripts/fleet-docker.sh).
#
#   bash scripts/fleet/setup.sh "ssh-ed25519 AAAA... itsanas-fleet-ci"
#
# What it does, and nothing else:
#
# 1. copies agent.sh and the acceptance kit to ~/.itsanas-fleet/ -- a copy,
#    so a `git pull` of this checkout never changes what the key can run;
#    run this again to update it;
# 2. writes ~/.itsanas-fleet/agent.conf from the accounts fleet-docker.sh
#    made (~/itsanas-data/<account>/home, ~/ITSaNAS-test);
# 3. adds the CI key to ~/.ssh/authorized_keys behind `restrict,command=`,
#    so that key runs the agent and cannot open a shell, forward a port or
#    get a terminal;
# 4. asks the agent `hello` and `status test` as the key would, and prints
#    this machine's host key for FLEET_KNOWN_HOSTS.
#
# No sudo, no new user, no docker: the agent reads and writes this user's
# files only (docs/FLEET-CI.md, "What a leaked key can do").

set -euo pipefail
cd "$(dirname "$0")/../.."

KEY=${1:-}
case "$KEY" in
    ssh-ed25519\ *) ;;
    *) echo "usage: setup.sh \"ssh-ed25519 AAAA... itsanas-fleet-ci\"  (the public half of the CI key)"; exit 2 ;;
esac

DEST="$HOME/.itsanas-fleet"
DATA="$HOME/itsanas-data"
FOLDER="$HOME/ITSaNAS-test"
[ -d "$FOLDER" ] || { echo "no $FOLDER: run scripts/fleet-docker.sh first (the test account)"; exit 1; }

mkdir -p "$DEST/receipts"
chmod 700 "$DEST"
install -m 755 scripts/fleet/agent.sh "$DEST/agent.sh"
install -m 644 scripts/acceptance.sh "$DEST/acceptance.sh"

accounts=""
homes=""
for home in "$DATA"/*/home; do
    [ -d "$home" ] || continue
    account=$(basename "$(dirname "$home")")
    [[ "$account" =~ ^[a-z0-9_]+$ ]] || { echo "skipped $account: not a plain name"; continue; }
    accounts="$accounts $account"
    homes="${homes}HOME_$account=\"$home\""$'\n'
done
[[ " $accounts " == *" test "* ]] || { echo "no test account under $DATA"; exit 1; }
# The test account's user id, which names its directory in every vault (C scan).
test_uid=$(sed -n 's/^ *user id *\([0-9a-f]\{64\}\).*/\1/p' "$DATA/test/home/status.snapshot" 2>/dev/null | head -1)
[ -n "$test_uid" ] || { echo "the test node has no snapshot with its user id yet: start its container, wait a round, run this again"; exit 1; }

cat >"$DEST/agent.conf" <<EOF
# Written by scripts/fleet/setup.sh on $(date -u +%Y-%m-%d). Read by agent.sh.
FOLDER="$FOLDER"
KIT="$DEST/acceptance.sh"
RECEIPTS="$DEST/receipts"
ACCOUNTS="${accounts# }"
TEST_UID="$test_uid"
${homes}MAX_FILES=50
EOF
chmod 600 "$DEST/agent.conf"

mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
touch "$HOME/.ssh/authorized_keys"
chmod 600 "$HOME/.ssh/authorized_keys"
line="restrict,command=\"$DEST/agent.sh\" $KEY"
# The key's base64 part, so a key already present is replaced rather than
# added a second time with other options.
body=$(printf '%s' "$KEY" | awk '{print $2}')
grep -vF "$body" "$HOME/.ssh/authorized_keys" >"$HOME/.ssh/authorized_keys.new" || true
printf '%s\n' "$line" >>"$HOME/.ssh/authorized_keys.new"
mv "$HOME/.ssh/authorized_keys.new" "$HOME/.ssh/authorized_keys"
echo "the CI key may now run $DEST/agent.sh and nothing else"

echo
echo "as the key would see it:"
SSH_ORIGINAL_COMMAND=hello ITSANAS_FLEET_CONF="$DEST/agent.conf" sh "$DEST/agent.sh" | sed 's/^/  /'
SSH_ORIGINAL_COMMAND="status test" ITSANAS_FLEET_CONF="$DEST/agent.conf" sh "$DEST/agent.sh" | sed 's/^/  /' || true
SSH_ORIGINAL_COMMAND="sh" ITSANAS_FLEET_CONF="$DEST/agent.conf" sh "$DEST/agent.sh" | sed 's/^/  /' || true

echo
echo "this machine's host key, for FLEET_KNOWN_HOSTS (replace localhost by the"
echo "name the runner's ssh config gives this machine):"
ssh-keyscan -t ed25519 localhost 2>/dev/null | sed 's/^/  /'
