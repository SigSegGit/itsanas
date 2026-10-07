#!/bin/sh
# Start a node's tray icon at each login on a Mac or a Linux desktop, or stop.
#
#   sh scripts/tray-autostart.sh install [NAME]   # the default node, or instance NAME
#   sh scripts/tray-autostart.sh remove [NAME]    # that node's only
#   sh scripts/tray-autostart.sh remove --all     # every node's, and the copies
#
# install/provision.sh (Linux) and install/macos.sh call it; install/clean.sh
# removes the same fixed names itself, so a clean-up needs no checkout. Windows
# has its own: the Startup shortcut install/provision.ps1 writes.
#
#   macOS  ~/Library/LaunchAgents/net.itsanas.menubar[.NAME].plist, running
#          osascript -l JavaScript ~/.local/share/itsanas/itsanas-menubar.js [NAME]
#   Linux  ~/.config/autostart/itsanas-tray[-NAME].desktop (the XDG autostart
#          every desktop reads), running
#          python3 ~/.local/share/itsanas/itsanas-tray.py [NAME]
#
# The tray script is copied out of the checkout first, for the reason
# provision.ps1 gives: a checkout gets moved or deleted, and an autostart into
# it would then start nothing, every login, silently.
#
# On Linux with no desktop session (no DISPLAY, no WAYLAND_DISPLAY) it does
# nothing: a Pi or a server has no panel to draw in, and an autostart file
# there is clutter. Run over ssh on a desktop machine, it is skipped too --
# say `DISPLAY=:0 sh scripts/tray-autostart.sh install` to have it anyway.
#
# ITSANAS_BIN, when set, is written into the autostart: a login session's PATH
# rarely holds ~/.local/bin, and macos.sh takes --prefix. ITSANAS_TRAY_OS
# (Darwin or Linux) overrides `uname -s`, so scripts/check-installers.sh can
# exercise both on one machine. POSIX sh, like the installers.

set -u

ACTION="${1:-}"
NAME="${2:-}"
OS="${ITSANAS_TRAY_OS:-$(uname -s)}"
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)
SHARE="$HOME/.local/share/itsanas"
AUTOSTART="$HOME/.config/autostart"
AGENTS="$HOME/Library/LaunchAgents"

say() { printf '  %s\n' "$*"; }
fail() { printf 'tray-autostart: %s\n' "$*" >&2; exit 2; }

case "$NAME" in
    --all) [ "$ACTION" = remove ] || fail "--all goes with remove" ;;
    '') ;;
    *[!a-z0-9-]*|-*|*-) fail "an instance is lowercase letters, digits and inner dashes" ;;
esac

desktop_file() { printf '%s/itsanas-tray%s.desktop' "$AUTOSTART" "${1:+-$1}"; }
agent_label() { printf 'net.itsanas.menubar%s' "${1:+.$1}"; }

# A path that needs escaping in a .desktop Exec line or a plist is refused
# rather than escaped: three escaping rules deep, a mistake starts nothing and
# says nothing.
plain_path() {
    case "$1" in
        *'"'*|*'`'*|*'$'*|*'\'*|*'&'*|*'<'*|*'>'*) return 1 ;;
        # % is a .desktop field code (the entry would silently not start), and
        # a newline or other control character would split the Exec line and
        # add keys of its own.
        *'%'*|*[[:cntrl:]]*) return 1 ;;
    esac
    return 0
}

copy_script() {
    mkdir -p "$SHARE" || return 1
    cp "$HERE/$1" "$SHARE/.$1.new" && mv -f "$SHARE/.$1.new" "$SHARE/$1"
}

install_linux() {
    if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
        say "no desktop session here, so no tray icon (itsanas settings opens the web page)"
        return 0
    fi
    copy_script itsanas-tray.py || fail "could not copy itsanas-tray.py to $SHARE"
    plain_path "$SHARE${ITSANAS_BIN:-}" || fail "$HOME or ITSANAS_BIN holds a character a .desktop line would need escaped"
    mkdir -p "$AUTOSTART" || fail "could not create $AUTOSTART"
    file=$(desktop_file "$NAME")
    exec_line="python3 \"$SHARE/itsanas-tray.py\"${NAME:+ $NAME}"
    [ -n "${ITSANAS_BIN:-}" ] && exec_line="env ITSANAS_BIN=\"$ITSANAS_BIN\" $exec_line"
    cat > "$file" <<DESKTOP
[Desktop Entry]
Type=Application
Name=ITSaNAS tray${NAME:+ ($NAME)}
Comment=Shows whether this machine's ITSaNAS node is syncing, and its menu
Exec=$exec_line
Terminal=false
NoDisplay=true
X-GNOME-Autostart-enabled=true
DESKTOP
    say "the tray icon starts at each login: $file"
}

install_macos() {
    label=$(agent_label "$NAME")
    file="$AGENTS/$label.plist"
    # The daemon of an instance called "menubar" is the LaunchAgent
    # net.itsanas.menubar (install/testbed.sh's naming), the very name of the
    # default node's menu-bar item: writing over it would stop that node.
    if [ -f "$file" ] && ! grep -q 'itsanas-menubar.js' "$file"; then
        say "$file runs something else (an instance named menubar?); the menu-bar item was not written"
        return 0
    fi
    copy_script itsanas-menubar.js || fail "could not copy itsanas-menubar.js to $SHARE"
    plain_path "$SHARE${ITSANAS_BIN:-}" || fail "$HOME or ITSANAS_BIN holds a character a plist would need escaped"
    mkdir -p "$AGENTS" || fail "could not create $AGENTS"
    {
        cat <<PLIST_HEAD
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/osascript</string>
    <string>-l</string>
    <string>JavaScript</string>
    <string>$SHARE/itsanas-menubar.js</string>
PLIST_HEAD
        [ -n "$NAME" ] && printf '    <string>%s</string>\n' "$NAME"
        printf '  </array>\n'
        if [ -n "${ITSANAS_BIN:-}" ]; then
            printf '  <key>EnvironmentVariables</key>\n  <dict>\n'
            printf '    <key>ITSANAS_BIN</key>\n    <string>%s</string>\n  </dict>\n' "$ITSANAS_BIN"
        fi
        cat <<PLIST_TAIL
  <key>RunAtLoad</key>
  <true/>
  <!-- Only in a session with a screen: an ssh login has no menu bar. -->
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
PLIST_TAIL
    } > "$file"
    if command -v plutil >/dev/null 2>&1 && ! plutil -lint "$file" >/dev/null 2>&1; then
        fail "plutil says $file is malformed"
    fi
    say "the menu-bar item starts at each login: $file"
    say "to show it now: launchctl bootstrap gui/\$(id -u) $file"
}

# Only a plist that runs the menu-bar script is ours: net.itsanas.menubar is
# also the daemon's LaunchAgent for an instance named menubar (see above).
remove_agent() {
    [ -f "$1" ] && grep -q 'itsanas-menubar.js' "$1" || return 0
    if command -v launchctl >/dev/null 2>&1; then
        launchctl bootout "gui/$(id -u)/$(basename "$1" .plist)" >/dev/null 2>&1 || true
    fi
    rm -f "$1"
}

remove_one() {
    rm -f "$(desktop_file "$1")"
    remove_agent "$AGENTS/$(agent_label "$1").plist"
}

case "$ACTION" in
    install)
        case "$OS" in
            Darwin) install_macos ;;
            Linux) install_linux ;;
            *) say "no tray autostart for $OS here" ;;
        esac
        ;;
    remove)
        if [ "$NAME" = --all ]; then
            rm -f "$AUTOSTART"/itsanas-tray.desktop "$AUTOSTART"/itsanas-tray-*.desktop
            for file in "$AGENTS"/net.itsanas.menubar.plist "$AGENTS"/net.itsanas.menubar.*.plist; do
                remove_agent "$file"
            done
            rm -f "$SHARE/itsanas-tray.py" "$SHARE/itsanas-menubar.js"
        else
            remove_one "$NAME"
        fi
        ;;
    *) fail "say install [NAME], remove [NAME] or remove --all" ;;
esac
exit 0
