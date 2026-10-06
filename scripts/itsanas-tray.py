#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""A tray icon for one ITSaNAS node on a Linux desktop.

    python3 scripts/itsanas-tray.py [NAME]

install/provision.sh copies this to ~/.local/share/itsanas/ and starts it at
each login through ~/.config/autostart/itsanas-tray[-NAME].desktop, one per
node, when the machine has a desktop; install/clean.sh removes it. Its
siblings draw the same menu: scripts/itsanas-tray.ps1 (Windows) and
scripts/itsanas-menubar.js (macOS).

It draws what `itsanas status --brief` says and runs `itsanas [--instance
NAME] ...` for every entry: no logic of its own beyond drawing (HANDOVER §8
0w (5)). The menu is built by `menu()`, a plain function of what the CLI said,
so `--describe 'paused 60'` prints it without a display, and
scripts/check-installers.sh compares that text with the two other trays'.

Toolkit: GTK 3 through python3-gi, with the Ayatana AppIndicator (what GNOME's
AppIndicator extension, KDE and most panels show), else the older
AppIndicator3, else Gtk.StatusIcon (XEmbed: XFCE, MATE, older panels). No gi
at all, and there is nothing to draw with: the web Settings page (`itsanas
settings`) does everything this menu does, in a browser. Python, not a Rust
crate, for the reason the Windows tray gives: the tray crates pull
dependencies `cargo deny` refuses.

The icon is a coloured dot written as a small SVG at run time into
~/.cache/itsanas/, so nothing binary ships with it.
"""

import os
import re
import shutil
import subprocess
import sys

# The words of the two dialogs, the same on every platform.
PAUSE_WORDS = (
    "Nothing is lost: what you change here waits until syncing resumes, and what "
    "your other machines change waits for you. This machine keeps hosting for the others."
)
SIGNOUT_WORDS = (
    "This stops ITSaNAS on this machine and forgets the passphrase here, so it no "
    "longer starts by itself.\n\nYour files, and the data this machine keeps for other "
    "people, stay on this disk. While signed out it does not sync, and the others cannot "
    "check what it keeps for them.\n\nTo come back, choose Sign in... and type your passphrase."
)
NO_TOOLKIT = (
    "itsanas-tray: no GTK for Python here (python3-gi), so no tray icon; "
    "the web Settings page does the same: itsanas settings"
)

COLOURS = {
    "green": "#2ea043",
    "blue": "#2f6feb",
    "orange": "#e68c14",
    "red": "#d22d2d",
    "grey": "#8c8c8c",
}

EVERY = [
    ("1 min", "1m", "1 min"),
    ("5 min", "5m", "5 min"),
    ("15 min", "15m", "15 min"),
    ("1 hour", "1h", "1 h"),
    ("Automatic", "auto", "auto"),
]

PAUSES = [
    ("For 1 hour", ["pause", "--for", "1h"], "for 1 hour"),
    ("For 8 hours", ["pause", "--for", "8h"], "for 8 hours"),
    ("Until I resume", ["pause"], "until you resume it"),
]


# ------------------------------------------------------------------ the model


def format_age(seconds):
    if seconds is None:
        return ""
    if seconds < 120:
        return " (%d s ago)" % seconds
    if seconds < 7200:
        return " (%d min ago)" % (seconds // 60)
    return " (%d h ago)" % (seconds // 3600)


def split_brief(brief):
    """`status --brief` is "WORD [AGE]"; anything else is unknown."""
    parts = (brief or "").split()
    if not parts:
        return "unknown", None
    age = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else None
    return parts[0], age


def colour(state):
    return {
        "healthy": "green",
        "paused": "blue",
        "stopped": "red",
        "departed": "red",
        "signed-out": "grey",
    }.get(state, "orange")


def current_every(line):
    """`itsanas interval` says "every 5 min (set with ...)" or "auto: ..."."""
    found = re.match(r"^every (.+?) \(", line or "")
    if found:
        return found.group(1)
    if (line or "").startswith("auto"):
        return "auto"
    return ""


def entry(kind, text, action=None, confirm=False, checked=False, children=None):
    return {
        "kind": kind,
        "text": text,
        "action": action,
        "confirm": confirm,
        "checked": checked,
        "children": children or [],
    }


def menu(brief, interval_line, label):
    state, age = split_brief(brief)
    items = [
        entry("item", "Open the synced folder", "open-folder"),
        entry("status", "%s: %s%s" % (label, state, format_age(age))),
        entry("sep", ""),
    ]
    if state == "signed-out":
        items.append(entry("item", "Sign in...", ["signin"]))
    else:
        if state == "paused":
            items.append(entry("item", "Resume syncing", ["resume"]))
        else:
            items.append(
                entry(
                    "menu",
                    "Pause syncing",
                    children=[entry("item", text, argv, confirm=True) for text, argv, _ in PAUSES],
                )
            )
        items.append(entry("item", "Sync now", ["sync-now"]))
        now = current_every(interval_line)
        items.append(
            entry(
                "menu",
                "Sync every",
                children=[
                    entry("item", text, ["interval", value], checked=(now == said))
                    for text, value, said in EVERY
                ],
            )
        )
        items.append(entry("sep", ""))
        items.append(entry("item", "Settings...", ["settings"]))
        items.append(entry("item", "Sign out...", ["signout"], confirm=True))
    items += [
        entry("sep", ""),
        entry("item", "Open the log", "open-log"),
        entry("item", "Restart", "restart"),
        entry("item", "Quit the icon", "quit"),
    ]
    return items


def cli_args(instance, argv):
    return (["--instance", instance] if instance else []) + list(argv)


def describe(items, instance, indent=""):
    """One line per entry, in the format the other trays print."""
    lines = []
    for item in items:
        if item["kind"] == "sep":
            lines.append(indent + "---")
        elif item["kind"] == "status":
            lines.append("%sstatus %s" % (indent, item["text"]))
        elif item["kind"] == "menu":
            lines.append("%smenu %s" % (indent, item["text"]))
            lines += describe(item["children"], instance, indent + "  ")
        else:
            action = item["action"]
            if isinstance(action, list):
                action = "itsanas " + " ".join(cli_args(instance, action))
            flags = (" [checked]" if item["checked"] else "") + (
                " [confirm]" if item["confirm"] else ""
            )
            lines.append("%sitem %s -> %s%s" % (indent, item["text"], action, flags))
    return lines


def svg(name):
    """The icon: a filled circle, as SVG text."""
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 22 22">'
        '<circle cx="11" cy="11" r="8" fill="%s"/></svg>\n' % COLOURS[name]
    )


def terminal_argv(command):
    """A terminal emulator running `command` (a list), then waiting for Enter;
    None when no known one is installed."""
    held = ["sh", "-c", '"$@"; echo; printf "Press Enter to close. "; read _', "sh"] + command
    for program, flag in (
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xfce4-terminal", "-x"),
        ("mate-terminal", "-x"),
        ("xterm", "-e"),
    ):
        if shutil.which(program):
            return [program, flag] + held
    return None


# ------------------------------------------------------------------ the icon


class Tray:
    """The drawing half: GTK objects, and the commands the entries run."""

    def __init__(self, gtk, glib, instance):
        self.gtk, self.glib, self.instance = gtk, glib, instance
        self.label = "ITSaNAS %s" % instance if instance else "ITSaNAS"
        self.exe = itsanas_binary()
        self.watched = None
        self.icons = write_icons()
        self.indicator = None
        self.status_icon = None
        self.make_icon()
        self.update()
        glib.timeout_add_seconds(30, self.update)

    def make_icon(self):
        appindicator = load_appindicator()
        if appindicator is not None:
            name = "itsanas-tray-%s" % (self.instance or "default")
            self.indicator = appindicator.Indicator.new(
                name, self.icons["orange"], appindicator.IndicatorCategory.APPLICATION_STATUS
            )
            self.indicator.set_status(appindicator.IndicatorStatus.ACTIVE)
            self.indicator.set_title(self.label)
        else:
            self.status_icon = self.gtk.StatusIcon.new_from_file(self.icons["orange"])
            self.status_icon.set_title(self.label)
            # A left click opens the folder, as on Windows; an indicator has
            # no left click of its own, so there it is the first entry.
            self.status_icon.connect("activate", lambda _icon: self.open_folder())
            self.status_icon.connect("popup-menu", self.popup)

    def run(self, argv, timeout=20):
        try:
            done = subprocess.run(
                [self.exe] + cli_args(self.instance, argv),
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                timeout=timeout,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        return done.stdout if done.returncode == 0 else None

    def build(self):
        brief = (self.run(["status", "--brief"]) or "").strip()
        interval = (self.run(["interval"]) or "").strip()
        state, age = split_brief(brief)
        gtk_menu = self.gtk.Menu()
        self.fill(gtk_menu, menu(brief, interval, self.label))
        gtk_menu.show_all()
        return state, age, gtk_menu

    def fill(self, gtk_menu, items):
        for item in items:
            if item["kind"] == "sep":
                gtk_menu.append(self.gtk.SeparatorMenuItem())
            elif item["kind"] == "status":
                shown = self.gtk.MenuItem(label=item["text"])
                shown.set_sensitive(False)
                gtk_menu.append(shown)
            elif item["kind"] == "menu":
                parent = self.gtk.MenuItem(label=item["text"])
                sub = self.gtk.Menu()
                self.fill(sub, item["children"])
                parent.set_submenu(sub)
                gtk_menu.append(parent)
            else:
                if item["checked"]:
                    shown = self.gtk.CheckMenuItem(label=item["text"])
                    shown.set_active(True)
                else:
                    shown = self.gtk.MenuItem(label=item["text"])
                shown.connect("activate", lambda _widget, chosen=item: self.choose(chosen))
                gtk_menu.append(shown)

    def update(self):
        state, age, gtk_menu = self.build()
        self.menu = gtk_menu
        icon = self.icons[colour(state)]
        tip = "%s: %s%s" % (self.label, state, format_age(age))
        if self.indicator is not None:
            self.indicator.set_icon_full(icon, tip)
            self.indicator.set_menu(gtk_menu)
        else:
            self.status_icon.set_from_file(icon)
            self.status_icon.set_tooltip_text(tip)
        return True

    def popup(self, _icon, button, when):
        self.update()
        self.menu.popup(None, None, None, None, button, when)

    def ask(self, question):
        dialog = self.gtk.MessageDialog(
            message_type=self.gtk.MessageType.QUESTION,
            buttons=self.gtk.ButtonsType.OK_CANCEL,
            text=question,
        )
        dialog.set_title(self.label)
        answer = dialog.run()
        dialog.destroy()
        return answer == self.gtk.ResponseType.OK

    def tell(self, text):
        dialog = self.gtk.MessageDialog(
            message_type=self.gtk.MessageType.INFO, buttons=self.gtk.ButtonsType.OK, text=text
        )
        dialog.set_title(self.label)
        dialog.run()
        dialog.destroy()

    def confirmed(self, item):
        if not item["confirm"]:
            return True
        if item["action"] == ["signout"]:
            return self.ask("Sign out of %s on this machine?\n\n%s" % (self.label, SIGNOUT_WORDS))
        lasts = next(said for text, _, said in PAUSES if text == item["text"])
        return self.ask("Pause syncing on this machine %s?\n\n%s" % (lasts, PAUSE_WORDS))

    def choose(self, item):
        if not self.confirmed(item):
            return
        action = item["action"]
        if isinstance(action, list):
            self.command(action)
        elif action == "open-folder":
            self.open_folder()
        elif action == "open-log":
            self.open_log()
        elif action == "restart":
            self.restart()
        elif action == "quit":
            self.gtk.main_quit()
            return
        self.update()

    def command(self, argv):
        full = [self.exe] + cli_args(self.instance, argv)
        if argv == ["settings"]:
            # It serves the page for as long as the page is open: never waited on.
            detach(full)
        elif argv == ["signin"]:
            # It asks for the passphrase, which only a terminal may receive.
            shown = terminal_argv(full)
            if shown:
                detach(shown)
            else:
                self.tell("Open a terminal and run:\n\n  " + " ".join(full))
        elif argv == ["signout"]:
            self.watch(full, "Sign out")
        elif argv == ["sync-now"] and self.run(argv) is None:
            self.tell("Not asked: syncing is paused, or no daemon is running for %s." % self.label)
        elif argv != ["sync-now"]:
            self.run(argv)

    def watch(self, full, what):
        """Run without waiting, and say what it printed when it ends."""
        try:
            process = subprocess.Popen(
                full,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
        except OSError as error:
            self.tell("%s did not start: %s" % (what, error))
            return

        def check():
            if process.poll() is None:
                return True
            said = (process.stdout.read() or "").strip()
            ending = "done" if process.returncode == 0 else "did not work"
            self.tell("%s %s.\n\n%s" % (what, ending, said))
            self.update()
            return False

        self.glib.timeout_add(1000, check)

    def open_folder(self):
        name = self.instance or "(unnamed)"
        for line in (self.run(["instances"]) or "").splitlines():
            if line.startswith(name + ":"):
                found = re.search(r", folder (.+?) (reachable|UNREACHABLE),", line)
                if found and os.path.isdir(found.group(1)):
                    detach(["xdg-open", found.group(1)])
                    return
        self.tell(
            "No synced folder is set for %s, or it is not reachable. Set one in Settings..."
            % self.label
        )

    def unit(self):
        return "itsanas@%s" % self.instance if self.instance else "itsanas"

    def open_log(self):
        # The daemon logs to the journal under systemd: there is no file to open.
        command = ["journalctl", "--user-unit", self.unit(), "-f"]
        shown = terminal_argv(command)
        if shown:
            detach(shown)
        else:
            self.tell("Open a terminal and run:\n\n  " + " ".join(command))

    def restart(self):
        if detach(["systemctl", "--user", "restart", self.unit()]) is None:
            self.tell("No systemd here to restart %s; start it with: itsanas daemon" % self.unit())


def detach(argv):
    try:
        return subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError:
        return None


def itsanas_binary():
    """ITSANAS_BIN, else where install/linux.sh puts it, else the PATH: a
    session started at login often has no ~/.local/bin on its PATH."""
    chosen = os.environ.get("ITSANAS_BIN")
    if chosen:
        return chosen
    local = os.path.expanduser("~/.local/bin/itsanas")
    if os.access(local, os.X_OK):
        return local
    return shutil.which("itsanas") or "itsanas"


def write_icons():
    folder = os.path.join(
        os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "itsanas"
    )
    os.makedirs(folder, exist_ok=True)
    paths = {}
    for name in COLOURS:
        path = os.path.join(folder, "tray-%s.svg" % name)
        with open(path, "w", encoding="utf-8") as out:
            out.write(svg(name))
        paths[name] = path
    return paths


def load_appindicator():
    import gi  # pylint: disable=import-outside-toplevel

    for namespace in ("AyatanaAppIndicator3", "AppIndicator3"):
        try:
            gi.require_version(namespace, "0.1")
            return __import__("gi.repository." + namespace, fromlist=[namespace])
        except (ImportError, ValueError):
            continue
    return None


def main(argv):
    instance, brief, interval, describing = "", None, "", False
    rest = list(argv)
    while rest:
        word = rest.pop(0)
        if word == "--describe" and rest:
            brief, describing = rest.pop(0), True
        elif word == "--interval" and rest:
            interval = rest.pop(0)
        elif word in ("-h", "--help"):
            print(__doc__.strip().splitlines()[0])
            print("usage: itsanas-tray.py [NAME] [--describe BRIEF [--interval LINE]]")
            return 0
        else:
            instance = word
    if describing:
        label = "ITSaNAS %s" % instance if instance else "ITSaNAS"
        print("icon " + colour(split_brief(brief)[0]))
        print("\n".join(describe(menu(brief, interval, label), instance)))
        return 0
    try:
        import gi  # pylint: disable=import-outside-toplevel

        gi.require_version("Gtk", "3.0")
        from gi.repository import GLib, Gtk  # pylint: disable=import-outside-toplevel
    except (ImportError, ValueError):
        print(NO_TOOLKIT)
        return 0
    if not Gtk.init_check(sys.argv)[0]:
        print("itsanas-tray: no display to draw on; the web Settings page: itsanas settings")
        return 0
    Tray(Gtk, GLib, instance)
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
