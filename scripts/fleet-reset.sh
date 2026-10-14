#!/usr/bin/env bash
# Wipe every ITSaNAS account on this Linux machine and set the fleet's three
# test accounts up again, from the signed release.
#
#   curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/scripts/fleet-reset.sh -o ~/fleet-reset.sh && bash ~/fleet-reset.sh vm
#   ... && bash ~/fleet-reset.sh pi      # every other machine, once the VM is done
#
# Why a file and not a block to paste: a 60-line paste over SSH reached the
# Pi truncated (2026-10-14), and the Pi had no checkout to run anything from.
# This needs neither: it downloads the three installer scripts it uses.
#
# `vm` is the machine running the coordinator: its member directory is emptied
# (every account forgotten) but its device key is kept, because 0.1.8 pins
# that key and a coordinator with a new one would be refused by every machine.
# The VM creates the accounts and prints their 24 words; `pi` (any other role)
# joins each one from words pasted at a prompt, never from a command line.
#
# Destroys data: every node, every synced folder holding the ITSaNAS marker,
# and old word files in $HOME. That is the point; read it before running it.
set -u

ROLE="${1:-}"
case "$ROLE" in
    vm|pi) ;;
    *) echo "usage: bash fleet-reset.sh vm|pi   (vm = the coordinator's machine, first)"; exit 2 ;;
esac

ACCOUNTS="nicolas test mandarine"
COORD=itsanas.ngas.fr:9898
COORD_ID=2cfb515fc90749d7248f4af404ecb34b5417e0c1339565c8036a9dde70cf72ce
RAW=https://raw.githubusercontent.com/SigSegGit/itsanas/main/install
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

for script in clean.sh get.sh provision.sh; do
    curl -fsSL "$RAW/$script" -o "$WORK/$script" || { echo "FAILED: download $script"; exit 1; }
done

echo; echo "== 1/4 Removing every ITSaNAS account on this machine"
for d in "$HOME"/.itsanas-*; do
    [ -d "$d" ] || continue
    sh "$WORK/clean.sh" --yes --purge-account --purge-folder --drain-for 0 --without-passphrase --instance "${d##*/.itsanas-}"
done
sh "$WORK/clean.sh" --yes --purge-account --purge-folder --drain-for 0 --without-passphrase
rm -fv "$HOME"/*mots*.txt "$HOME"/itsanas-phrase*.txt 2>/dev/null

echo; echo "== 2/4 The coordinator"
if [ "$ROLE" = vm ]; then
    sudo systemctl stop itsanas-coordinator
    sudo rm -f /var/lib/itsanas-coordinator/directory.redb
    id=$(sudo -u itsanas-coord /usr/local/bin/itsanas-coordinator --state /var/lib/itsanas-coordinator --identity 2>/dev/null \
        | grep -o '[0-9a-f]\{64\}' | head -1)
    sudo systemctl start itsanas-coordinator
    if [ "$id" != "$COORD_ID" ]; then
        echo "  STOP: the coordinator's identity is '$id', not the pinned $COORD_ID"
        exit 1
    fi
    echo "  ok: directory empty, identity unchanged ($(sudo systemctl is-active itsanas-coordinator))"
else
    if sudo systemctl disable --now itsanas-coordinator 2>/dev/null; then
        echo "  an old coordinator on this machine was stopped"
    else
        echo "  no coordinator here (expected)"
    fi
fi

echo; echo "== 3/4 Installing the signed release"
sh "$WORK/get.sh" --no-setup --yes || { echo "FAILED: install"; exit 1; }
export PATH="$HOME/.local/bin:$PATH"
itsanas --version

echo; echo "== 4/4 The accounts: $ACCOUNTS"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
for acct in $ACCOUNTS; do
    echo; echo "---------- account $acct ----------"
    # One passphrase per account, as three people sharing a machine would
    # have: the test must show an instance never opens another's keys.
    while :; do
        read -r -s -p "Passphrase for '$acct' on this machine: " PASS; echo
        read -r -s -p "Again: " PASS2; echo
        [ -n "$PASS" ] && [ "$PASS" = "$PASS2" ] && break
        echo "They differ or are empty; again."
    done
    printf -v "PASS_${acct//-/_}" '%s' "$PASS"
    inst=()
    [ "$acct" = nicolas ] || inst=(--instance "$acct")
    phrase=()
    if [ "$ROLE" != vm ]; then
        f="$WORK/phrase-$acct"
        (umask 077; : > "$f")
        read -r -p "Paste the 24 words of '$acct' (one line), then Enter: " words
        printf '%s\n' "$words" > "$f"
        phrase=(--phrase-file "$f")
    fi
    ITSANAS_PASSPHRASE="$PASS" sh "$WORK/provision.sh" --no-install "${inst[@]}" --username "$acct" \
        --pledge 10G --keep 3G --folder "$HOME/ITSaNAS-$acct" \
        --coordinator "$COORD" --coordinator-device "$COORD_ID" "${phrase[@]}"
    rm -f "$WORK/phrase-$acct"
    if [ -d "$HOME/ITSaNAS-$acct" ]; then
        echo "bonjour de $(hostname) pour $acct, $(date)" > "$HOME/ITSaNAS-$acct/bonjour-$(hostname).txt"
    fi
done

echo; echo "== Done. One id per account; it must be the same on every machine:"
for acct in $ACCOUNTS; do
    inst=()
    [ "$acct" = nicolas ] || inst=(--instance "$acct")
    printf '%-10s ' "$acct"
    var="PASS_${acct//-/_}"
    ITSANAS_PASSPHRASE="${!var}" itsanas "${inst[@]}" whoami 2>&1 | tail -1
done
