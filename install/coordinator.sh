#!/usr/bin/env sh
# Set up an ITSaNAS coordinator on a Linux machine with a public address.
#
#   sh install/coordinator.sh
#
# This is the Freebox Delta VM in Nicolas's fleet, and anything like it: a small
# always-on machine whose only job is to be reachable. It is a different role
# from a member node, so it gets a different script rather than a flag on the
# other one.
#
# What a coordinator is
# ---------------------
#
# A notice board. It holds usernames, device addresses, and sealed escrow blobs
# it cannot open. It holds **no file data and no keys**, it cannot read anything
# a member stores, and if it disappears the members keep syncing with the peers
# they already know — they just cannot find new ones. `docs/ECONOMICS.md` §7 is
# the argument; this script is the deployment.
#
# What that means for how it is run, and why this script differs from the others
# ----------------------------------------------------------------------------
#
# **It is exposed.** A member node usually sits behind NAT and talks outwards.
# This one has a port open to the internet, so it is the only machine in the
# fleet a stranger can reach unprompted. That changes three things, and this
# script does all three:
#
#   - it runs as a **system service under its own unmixed user**, not as your
#     login, because a process on a public port should own nothing else;
#   - it is **invite-only** from the moment it has a first member, because
#     otherwise "who is a member" means "anyone who can open a socket";
#   - it prints its **device id**, because members must pin it. A coordinator
#     supplies addresses and is never trusted to say who lives at one.
#
# **It needs no passphrase.** It holds no user keys, only its own device key, so
# unlike a member node it can start unattended without anything secret in an
# environment file. That is the whole reason it can be a system service.

set -u

VERSION="1.0"

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_OK=$(printf '\033[32m'); C_WARN=$(printf '\033[33m')
    C_ERR=$(printf '\033[31m'); C_DIM=$(printf '\033[2m'); C_OFF=$(printf '\033[0m')
else
    C_OK=''; C_WARN=''; C_ERR=''; C_DIM=''; C_OFF=''
fi

step()  { printf '\n%s==>%s %s\n' "$C_DIM" "$C_OFF" "$*"; }
ok()    { printf '  %sok%s   %s\n' "$C_OK" "$C_OFF" "$*"; }
warn()  { printf '  %swarn%s %s\n' "$C_WARN" "$C_OFF" "$*"; }
info()  { printf '       %s\n' "$*"; }
die() {
    printf '\n%serror%s %s\n' "$C_ERR" "$C_OFF" "$1"
    shift
    for line in "$@"; do printf '       %s\n' "$line"; done
    printf '\n'
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

PORT="${ITSANAS_COORD_PORT:-9898}"
SERVICE_USER="itsanas-coord"
STATE_DIR="/var/lib/itsanas-coordinator"
BIN_SRC=""
OPEN_DOOR=0
DO_INSTALL=1
# Whether --check found everything the real run needs, and whether any address
# on this machine is reachable from outside. --check used to stop with no
# verdict at all, which left the reader to decide from a list of warnings.
READY=1
ROUTABLE=0

usage() {
    cat <<'USAGE'
ITSaNAS coordinator setup

  sudo sh install/coordinator.sh [options]

Options
  --port N          port to listen on (default 9898)
  --binary PATH     use this itsanas-coordinator instead of looking for one
  --admit-first     let the next registration in without an invitation, once
  --check           look at the machine and stop, changing nothing
  --help            this

Run --admit-first exactly once, register your own account from another
machine, then restart the service without it. An invitation to admit the
first member has no author, so something has to open the door once — and a
door that opens by itself on a public address is opened by whoever finds
the port first.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --port) [ $# -ge 2 ] || die "--port needs a number"; PORT="$2"; shift 2 ;;
        --port=*) PORT="${1#--port=}"; shift ;;
        --binary) [ $# -ge 2 ] || die "--binary needs a path"; BIN_SRC="$2"; shift 2 ;;
        --binary=*) BIN_SRC="${1#--binary=}"; shift ;;
        --admit-first) OPEN_DOOR=1; shift ;;
        --check) DO_INSTALL=0; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1" "Run with --help for the list." ;;
    esac
done

case "$PORT" in
    ''|*[!0-9]*) die "--port must be a number, got: $PORT" ;;
    *) [ "$PORT" -ge 1 ] && [ "$PORT" -le 65535 ] || die "--port must be 1-65535, got: $PORT" ;;
esac

printf '%sITSaNAS coordinator setup %s%s\n' "$C_DIM" "$VERSION" "$C_OFF"

# ------------------------------------------------------------------ checks

step "Looking at this machine"

[ "$(uname -s)" = "Linux" ] || die "this script is for Linux" \
    "A coordinator can run anywhere the binary does, but the service setup" \
    "below is systemd. On anything else, run it by hand:" \
    "  itsanas-coordinator --state ./coordinator --listen 0.0.0.0:$PORT"
ok "Linux on $(uname -m)"

if [ "$DO_INSTALL" -eq 1 ] && [ "$(id -u)" -ne 0 ]; then
    die "this needs root" \
        "It creates a system user and a system service, because a process on a" \
        "public port should not run as you and should not own your files." \
        "" \
        "  sudo sh install/coordinator.sh" \
        "" \
        "Or check without changing anything:" \
        "  sh install/coordinator.sh --check"
fi

if ! have systemctl; then
    warn "no systemd here; the service will not be installed"
    info "Run it by hand, or write a unit for whatever this machine uses."
fi

# Is the port already taken? Finding out now beats a service that fails to
# start with "address in use" three steps later.
if have ss; then
    if ss -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]$PORT\$"; then
        die "something is already listening on port $PORT" \
            "Find it with:  sudo ss -ltnp | grep :$PORT" \
            "Then stop it, or choose another port with --port N."
    fi
    ok "port $PORT is free"
elif have netstat; then
    if netstat -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]$PORT\$"; then
        die "something is already listening on port $PORT"
    fi
    ok "port $PORT is free"
else
    warn "neither ss nor netstat is here; cannot check whether port $PORT is free"
fi

# The address. A coordinator behind NAT with no forwarding is a coordinator
# nobody outside can reach, which is the one thing it exists to be.
step "Reachability"

LOCAL_ADDRS=$(ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1)
if [ -n "$LOCAL_ADDRS" ]; then
    for addr in $LOCAL_ADDRS; do
        case "$addr" in
            10.*|192.168.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*|100.6[4-9].*|100.[7-9][0-9].*|100.1[0-2][0-9].*)
                warn "$addr is a private address" ;;
            *) ok "$addr looks routable"; ROUTABLE=1 ;;
        esac
    done
else
    warn "could not list this machine's addresses"
fi

if [ "$ROUTABLE" -eq 0 ]; then
    info ""
    info "A coordinator is only useful if members elsewhere can reach it. Every"
    info "address above is private, so forward TCP port $PORT to this machine on"
    info "the router — on a Freebox that is Paramètres > Gestion des ports."
    info ""
    info "Check from outside once it is running:"
    info "  nc -vz <your-public-address> $PORT"
fi

# ----------------------------------------------------------------- binary
#
# Looked for before `--check` stops rather than after it. A missing binary is
# the most likely reason the real run will not work, and a check that reports
# the port and the routing and then says nothing about whether there is anything
# to install is answering the easy half of the question.

step "Finding the binary"

if [ -n "$BIN_SRC" ]; then
    if [ -x "$BIN_SRC" ]; then
        ok "$BIN_SRC"
    elif [ "$DO_INSTALL" -eq 0 ]; then
        warn "not an executable: $BIN_SRC"
        READY=0
    else
        die "not an executable: $BIN_SRC"
    fi
else
    HERE=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd)
    for candidate in \
        "$HERE/target/release/itsanas-coordinator" \
        "$(command -v itsanas-coordinator 2>/dev/null)"
    do
        [ -n "$candidate" ] && [ -x "$candidate" ] && BIN_SRC="$candidate" && break
    done
    if [ -n "$BIN_SRC" ]; then
        ok "$BIN_SRC"
    elif [ "$DO_INSTALL" -eq 0 ]; then
        warn "no itsanas-coordinator binary found"
        info "Build it first:  sh install/linux.sh --no-service"
        READY=0
    else
        die "no itsanas-coordinator binary found" \
            "Build it first:" \
            "  sh install/linux.sh --no-service" \
            "or point at one you already have:" \
            "  sudo sh install/coordinator.sh --binary /path/to/itsanas-coordinator"
    fi
fi

if [ "$DO_INSTALL" -eq 0 ]; then
    step "Stopping here (--check)"
    if [ "$READY" -eq 1 ]; then
        ok "this machine can host a coordinator"
        [ "$ROUTABLE" -eq 0 ] && info "Forward the port first; see above."
        exit 0
    fi
    warn "not ready; see what is missing above"
    exit 1
fi

# ------------------------------------------------------------------ user

step "Setting up the service account"

if id "$SERVICE_USER" >/dev/null 2>&1; then
    ok "user $SERVICE_USER already exists"
else
    # No login, no home worth having, no shell. It runs one program and owns
    # one directory.
    if have useradd; then
        useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER" \
            || die "could not create the user $SERVICE_USER"
    elif have adduser; then
        adduser --system --no-create-home --disabled-login "$SERVICE_USER" \
            || die "could not create the user $SERVICE_USER"
    else
        die "neither useradd nor adduser is here" \
            "Create a system user called $SERVICE_USER and run this again."
    fi
    ok "created $SERVICE_USER"
fi

mkdir -p "$STATE_DIR" || die "could not create $STATE_DIR"
chown "$SERVICE_USER":"$SERVICE_USER" "$STATE_DIR" || die "could not chown $STATE_DIR"
chmod 700 "$STATE_DIR" || die "could not chmod $STATE_DIR"
ok "$STATE_DIR"

install -m 0755 "$BIN_SRC" /usr/local/bin/itsanas-coordinator \
    || die "could not install the binary into /usr/local/bin"
ok "/usr/local/bin/itsanas-coordinator"

# --------------------------------------------------------------- identity

step "Its identity"

# Generated by the binary itself on first run. Printed here because members
# have to pin it: the coordinator supplies addresses and is never trusted to
# say who lives at one.
DEVICE_ID=$(su -s /bin/sh "$SERVICE_USER" -c \
    "/usr/local/bin/itsanas-coordinator --state '$STATE_DIR' --identity" 2>/dev/null)
case "$DEVICE_ID" in
    [0-9a-f]*) ok "device id $DEVICE_ID" ;;
    *) die "could not read the coordinator's device id" \
           "Tried: itsanas-coordinator --state $STATE_DIR --identity" \
           "Got: ${DEVICE_ID:-nothing}" ;;
esac

# ---------------------------------------------------------------- service

if have systemctl; then
    step "The service"

    ADMIT=""
    [ "$OPEN_DOOR" -eq 1 ] && ADMIT=" --admit-first"

    cat > /etc/systemd/system/itsanas-coordinator.service <<UNIT
[Unit]
Description=ITSaNAS coordinator
Documentation=https://github.com/SigSegGit/itsanas
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
ExecStart=/usr/local/bin/itsanas-coordinator --state $STATE_DIR --listen 0.0.0.0:$PORT --invite-only$ADMIT
Restart=always
RestartSec=10

# It is on a public port and it holds no user data and no user keys, so it is
# given nothing. If it is ever compromised, what an attacker gets is a list of
# usernames and addresses and some blobs they cannot open.
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6
RestrictNamespaces=true
LockPersonality=true
MemoryDenyWriteExecute=true
SystemCallArchitectures=native

# StateDirectory rather than ReadWritePaths: systemd creates it, chowns it to
# the service user, and keeps it. ReadWritePaths on a directory that does not
# exist makes the unit refuse to start with an error about mount namespaces,
# which is what happens the first time somebody changes --state and forgets to
# create the new path by hand.
StateDirectory=itsanas-coordinator
StateDirectoryMode=0700

[Install]
WantedBy=multi-user.target
UNIT
    ok "/etc/systemd/system/itsanas-coordinator.service"

    systemctl daemon-reload || warn "systemctl daemon-reload failed"

    if [ "$OPEN_DOOR" -eq 1 ]; then
        warn "starting with --admit-first: the next registration is let in"
        info "Register your own account from another machine now, then run:"
        info "  sudo sh install/coordinator.sh --port $PORT"
        info "to rewrite the unit without it, and:"
        info "  sudo systemctl restart itsanas-coordinator"
    fi

    systemctl enable --now itsanas-coordinator >/dev/null 2>&1 \
        || die "the service would not start" \
               "Look at why with:" \
               "  sudo systemctl status itsanas-coordinator" \
               "  sudo journalctl -u itsanas-coordinator -n 50"

    # Started is not the same as serving. Give it a moment and check.
    sleep 2
    if systemctl is-active --quiet itsanas-coordinator; then
        ok "running"
    else
        die "the service started and then stopped" \
            "  sudo journalctl -u itsanas-coordinator -n 50"
    fi
fi

# ------------------------------------------------------------------- next

cat <<NEXT

${C_OK}The coordinator is up.${C_OFF}

  address    <this machine>:$PORT
  device id  $DEVICE_ID
  state      $STATE_DIR
  admits     invited members only${ADMIT:+ (and the next one, once)}

On each member machine, pin it:

  itsanas coordinator <address>:$PORT --device $DEVICE_ID
  itsanas register${OPEN_DOOR:+                     # the first one needs no code}

Members invite each other after that:

  itsanas invite                    on a machine that is already a member
  itsanas register --invite <code>  on the machine joining

Watch it:

  sudo journalctl -u itsanas-coordinator -f

Back it up: everything it holds is in $STATE_DIR. Losing it means members
cannot find each other until they are pointed at a new one by hand; it does
not mean losing any data, because it never had any.

NEXT
