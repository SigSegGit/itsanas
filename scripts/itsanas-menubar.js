// A menu-bar item for one ITSaNAS node on macOS.
//
//   osascript -l JavaScript scripts/itsanas-menubar.js [NAME]
//
// install/macos.sh copies this to ~/.local/share/itsanas/ and starts it at each
// login through ~/Library/LaunchAgents/net.itsanas.menubar[.NAME].plist;
// install/clean.sh removes it. Its siblings draw the same menu:
// scripts/itsanas-tray.ps1 (Windows) and scripts/itsanas-tray.py (Linux).
//
// JavaScript for Automation with the Objective-C bridge, run by osascript,
// which every Mac has: an NSStatusItem whose title is a coloured dot, the
// menu, and NSApplication's run loop to keep it alive. Measured against the
// alternative HANDOVER §8 0w (5) named -- a small Swift app built by
// macos.sh -- this needs no Xcode, no build step and no signing, and the
// osascript binary that runs it is Apple's. The cost: no unit tests on a
// Mac, so the menu itself is built by plain functions that run under node too.
// `node scripts/itsanas-menubar.js --describe 'paused 60'` prints the menu
// without a Mac, and scripts/check-installers.sh compares that text with the
// two other trays'.
//
// It draws what `itsanas status --brief` says (every 30 s) and runs `itsanas
// [--instance NAME] ...` for every entry: no logic of its own beyond drawing.
// Never wait on a long command here -- the menu bar would freeze: `settings`
// serves a page for as long as it is open, so it is started in the
// background; `signout` too, and a timer reports how it ended; `signin` asks
// for the passphrase, so it opens in Terminal.

'use strict';

// The words of the two dialogs, the same on every platform.
var PAUSE_WORDS = 'Nothing is lost: what you change here waits until syncing resumes, and what ' +
    'your other machines change waits for you. This machine keeps hosting for the others.';
var SIGNOUT_WORDS = 'This stops ITSaNAS on this machine and forgets the passphrase here, so it no ' +
    'longer starts by itself.\n\nYour files, and the data this machine keeps for other people, ' +
    'stay on this disk. While signed out it does not sync, and the others cannot check what it ' +
    'keeps for them.\n\nTo come back, choose Sign in... and type your passphrase.';

var COLOURS = {
    green: [46, 160, 67],
    blue: [47, 111, 235],
    orange: [230, 140, 20],
    red: [210, 45, 45],
    grey: [140, 140, 140]
};

var EVERY = [
    ['1 min', '1m', '1 min'],
    ['5 min', '5m', '5 min'],
    ['15 min', '15m', '15 min'],
    ['1 hour', '1h', '1 h'],
    ['Automatic', 'auto', 'auto']
];

var PAUSES = [
    ['For 1 hour', ['pause', '--for', '1h'], 'for 1 hour'],
    ['For 8 hours', ['pause', '--for', '8h'], 'for 8 hours'],
    ['Until I resume', ['pause'], 'until you resume it']
];

// ------------------------------------------------------------------ the model

function formatAge(seconds) {
    if (seconds === null) { return ''; }
    if (seconds < 120) { return ' (' + seconds + ' s ago)'; }
    if (seconds < 7200) { return ' (' + Math.floor(seconds / 60) + ' min ago)'; }
    return ' (' + Math.floor(seconds / 3600) + ' h ago)';
}

// `status --brief` is "WORD [AGE]"; anything else is unknown.
function splitBrief(brief) {
    var parts = String(brief || '').trim().split(/\s+/);
    if (!parts[0]) { return ['unknown', null]; }
    var age = parts.length > 1 && /^\d+$/.test(parts[1]) ? parseInt(parts[1], 10) : null;
    return [parts[0], age];
}

function colour(state) {
    var known = { healthy: 'green', paused: 'blue', stopped: 'red', departed: 'red', 'signed-out': 'grey' };
    return Object.prototype.hasOwnProperty.call(known, state) ? known[state] : 'orange';
}

// `itsanas interval` says "every 5 min (set with ...)" or "auto: ...".
function currentEvery(line) {
    var found = /^every (.+?) \(/.exec(line || '');
    if (found) { return found[1]; }
    return String(line || '').indexOf('auto') === 0 ? 'auto' : '';
}

function entry(kind, text, action, flags) {
    flags = flags || {};
    return {
        kind: kind,
        text: text,
        action: action === undefined ? null : action,
        confirm: !!flags.confirm,
        checked: !!flags.checked,
        children: flags.children || []
    };
}

function menuModel(brief, intervalLine, label) {
    var split = splitBrief(brief);
    var state = split[0];
    var items = [
        entry('item', 'Open the synced folder', 'open-folder'),
        entry('status', label + ': ' + state + formatAge(split[1])),
        entry('sep', '')
    ];
    if (state === 'signed-out') {
        items.push(entry('item', 'Sign in...', ['signin']));
    } else {
        if (state === 'paused') {
            items.push(entry('item', 'Resume syncing', ['resume']));
        } else {
            items.push(entry('menu', 'Pause syncing', null, {
                children: PAUSES.map(function (pause) {
                    return entry('item', pause[0], pause[1], { confirm: true });
                })
            }));
        }
        items.push(entry('item', 'Sync now', ['sync-now']));
        var now = currentEvery(intervalLine);
        items.push(entry('menu', 'Sync every', null, {
            children: EVERY.map(function (every) {
                return entry('item', every[0], ['interval', every[1]], { checked: now === every[2] });
            })
        }));
        items.push(entry('sep', ''));
        items.push(entry('item', 'Settings...', ['settings']));
        items.push(entry('item', 'Sign out...', ['signout'], { confirm: true }));
    }
    items.push(entry('sep', ''));
    items.push(entry('item', 'Open the log', 'open-log'));
    items.push(entry('item', 'Restart', 'restart'));
    items.push(entry('item', 'Quit the icon', 'quit'));
    return items;
}

function cliArgs(instance, argv) {
    return (instance ? ['--instance', instance] : []).concat(argv);
}

// One line per entry, in the format the other trays print.
function describe(items, instance, indent) {
    indent = indent || '';
    var lines = [];
    items.forEach(function (item) {
        if (item.kind === 'sep') {
            lines.push(indent + '---');
        } else if (item.kind === 'status') {
            lines.push(indent + 'status ' + item.text);
        } else if (item.kind === 'menu') {
            lines.push(indent + 'menu ' + item.text);
            lines = lines.concat(describe(item.children, instance, indent + '  '));
        } else {
            var action = Array.isArray(item.action)
                ? 'itsanas ' + cliArgs(instance, item.action).join(' ')
                : item.action;
            lines.push(indent + 'item ' + item.text + ' -> ' + action +
                (item.checked ? ' [checked]' : '') + (item.confirm ? ' [confirm]' : ''));
        }
    });
    return lines;
}

function labelFor(instance) {
    return instance ? 'ITSaNAS ' + instance : 'ITSaNAS';
}

function parseArguments(argv) {
    var parsed = { instance: '', brief: null, interval: '', describing: false };
    for (var i = 0; i < argv.length; i++) {
        if (argv[i] === '--describe' && i + 1 < argv.length) {
            parsed.brief = argv[++i];
            parsed.describing = true;
        } else if (argv[i] === '--interval' && i + 1 < argv.length) {
            parsed.interval = argv[++i];
        } else {
            parsed.instance = argv[i];
        }
    }
    return parsed;
}

function describeMain(argv) {
    var parsed = parseArguments(argv);
    var lines = ['icon ' + colour(splitBrief(parsed.brief)[0])]
        .concat(describe(menuModel(parsed.brief, parsed.interval, labelFor(parsed.instance)), parsed.instance));
    return lines.join('\n');
}

// ----------------------------------------------------------- the menu bar

// Single quotes for sh, with any quote inside closed, escaped and reopened.
function quote(text) {
    return "'" + String(text).replace(/'/g, "'\\''") + "'";
}

// osascript calls this with the arguments after the script's path.
// eslint-disable-next-line no-unused-vars
function run(argv) {
    ObjC.import('Cocoa');
    var parsed = parseArguments(argv);
    if (parsed.describing) { return describeMain(argv); }
    var instance = parsed.instance;
    var label = labelFor(instance);
    var shell = Application.currentApplication();
    shell.includeStandardAdditions = true;
    var home = ObjC.unwrap($.NSHomeDirectory());
    var files = $.NSFileManager.defaultManager;

    function sh(command) {
        try {
            return { ok: true, out: shell.doShellScript(command, { alteringLineEndings: false }) };
        } catch (error) {
            return { ok: false, out: String(error.message || error) };
        }
    }

    // ITSANAS_BIN, else where macos.sh puts it, else Homebrew's places: a
    // LaunchAgent's PATH is /usr/bin:/bin:/usr/sbin:/sbin and nothing more.
    var exe = ObjC.unwrap($.NSProcessInfo.processInfo.environment.objectForKey('ITSANAS_BIN')) || '';
    [home + '/.local/bin/itsanas', '/opt/homebrew/bin/itsanas', '/usr/local/bin/itsanas'].forEach(function (path) {
        if (!exe && files.isExecutableFileAtPath(path)) { exe = path; }
    });
    exe = exe || 'itsanas';

    function cli(argv) {
        return [exe].concat(cliArgs(instance, argv)).map(quote).join(' ');
    }

    function itsanas(argv) {
        var done = sh(cli(argv) + ' </dev/null 2>/dev/null');
        return done.ok ? done.out : null;
    }

    var app = $.NSApplication.sharedApplication;
    // An accessory (1, NSApplicationActivationPolicyAccessory): a menu-bar
    // item, no Dock icon, no menu of its own.
    app.setActivationPolicy(1);

    function alert(text, withCancel) {
        app.activateIgnoringOtherApps(true);
        var box = $.NSAlert.alloc.init;
        box.messageText = label;
        box.informativeText = text;
        box.addButtonWithTitle('OK');
        if (withCancel) { box.addButtonWithTitle('Cancel'); }
        // 1000 is NSAlertFirstButtonReturn, written out: an enum the bridge
        // does not know reads as undefined, and every answer as Cancel.
        return box.runModal === 1000;
    }

    // -1 is NSVariableStatusItemLength: as wide as the dot.
    var statusItem = $.NSStatusBar.systemStatusBar.statusItemWithLength(-1);
    var actions = [];
    var pending = null;

    function dot(name) {
        var rgb = COLOURS[name];
        var paint = $.NSColor.colorWithSRGBRedGreenBlueAlpha(rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, 1);
        var attributes = $.NSDictionary.dictionaryWithObjectForKey(paint, $.NSForegroundColorAttributeName);
        return $.NSAttributedString.alloc.initWithStringAttributes('●', attributes);
    }

    function addEntries(menu, items, target) {
        items.forEach(function (item) {
            if (item.kind === 'sep') {
                menu.addItem($.NSMenuItem.separatorItem);
                return;
            }
            // An item with no action is drawn disabled: the status line.
            var shown = $.NSMenuItem.alloc.initWithTitleActionKeyEquivalent(
                item.text, item.kind === 'item' ? 'choose:' : null, '');
            if (item.kind === 'menu') {
                var sub = $.NSMenu.alloc.initWithTitle(item.text);
                addEntries(sub, item.children, target);
                shown.submenu = sub;
            } else if (item.kind === 'item') {
                shown.target = target;
                shown.tag = actions.length;
                actions.push(item);
                if (item.checked) { shown.state = 1; }
            }
            menu.addItem(shown);
        });
    }

    var target;

    function update() {
        var brief = (itsanas(['status', '--brief']) || '').trim();
        var interval = (itsanas(['interval']) || '').trim();
        var split = splitBrief(brief);
        statusItem.button.attributedTitle = dot(colour(split[0]));
        statusItem.button.toolTip = label + ': ' + split[0] + formatAge(split[1]);
        actions = [];
        var menu = $.NSMenu.alloc.initWithTitle(label);
        addEntries(menu, menuModel(brief, interval, label), target);
        statusItem.menu = menu;
    }

    function openFolder() {
        var name = instance || '(unnamed)';
        var lines = (itsanas(['instances']) || '').split('\n');
        for (var i = 0; i < lines.length; i++) {
            if (lines[i].indexOf(name + ':') === 0) {
                var found = /, folder (.+?) (reachable|UNREACHABLE),/.exec(lines[i]);
                if (found && files.fileExistsAtPath(found[1])) {
                    sh('open ' + quote(found[1]));
                    return;
                }
            }
        }
        alert('No synced folder is set for ' + label + ', or it is not reachable. Set one in Settings...', false);
    }

    // The daemon's LaunchAgent, as macos.sh and testbed.sh name it.
    var agent = instance ? 'net.itsanas.' + instance : 'net.itsanas.daemon';
    var log = instance ? home + '/.itsanas-' + instance + '/daemon.log' : home + '/Library/Logs/itsanas.log';

    // Started in the background with its output kept; `watch:` reports it.
    function startWatched(argv, what) {
        var stem = ObjC.unwrap($.NSTemporaryDirectory()) + 'itsanas-' + Date.now();
        sh('(' + cli(argv) + ' </dev/null >' + quote(stem + '.out') + ' 2>&1; echo $? >' +
            quote(stem + '.done') + ') >/dev/null 2>&1 &');
        pending = { stem: stem, what: what };
    }

    function checkWatched() {
        if (!pending || !files.fileExistsAtPath(pending.stem + '.done')) { return; }
        var job = pending;
        pending = null;
        var code = (sh('cat ' + quote(job.stem + '.done')).out || '').trim();
        var said = (sh('cat ' + quote(job.stem + '.out')).out || '').trim();
        sh('rm -f ' + quote(job.stem + '.out') + ' ' + quote(job.stem + '.done'));
        alert(job.what + (code === '0' ? ' done.' : ' did not work.') + '\n\n' + said, false);
        update();
    }

    // Sign in asks for the passphrase, which only a terminal may receive: a
    // .command file opens in Terminal without asking for Automation rights.
    function inTerminal(command) {
        var path = ObjC.unwrap($.NSTemporaryDirectory()) + 'itsanas-' + Date.now() + '.command';
        var text = '#!/bin/sh\n' + command + '\necho\nprintf "Press Enter to close. "\nread _\n';
        $(text).writeToFileAtomicallyEncodingError(path, true, $.NSUTF8StringEncoding, null);
        sh('chmod +x ' + quote(path) + ' && open ' + quote(path));
    }

    function confirmed(item) {
        if (!item.confirm) { return true; }
        if (item.action[0] === 'signout') {
            return alert('Sign out of ' + label + ' on this machine?\n\n' + SIGNOUT_WORDS, true);
        }
        var lasts = PAUSES.filter(function (pause) { return pause[0] === item.text; })[0][2];
        return alert('Pause syncing on this machine ' + lasts + '?\n\n' + PAUSE_WORDS, true);
    }

    function command(argv) {
        if (argv[0] === 'settings') {
            sh(cli(argv) + ' </dev/null >/dev/null 2>&1 &');
        } else if (argv[0] === 'signin') {
            inTerminal(cli(argv));
        } else if (argv[0] === 'signout') {
            startWatched(argv, 'Sign out');
        } else if (itsanas(argv) === null && argv[0] === 'sync-now') {
            alert('Not asked: syncing is paused, or no daemon is running for ' + label + '.', false);
        }
    }

    function choose(tag) {
        var item = actions[tag];
        if (!item || !confirmed(item)) { return; }
        if (Array.isArray(item.action)) {
            command(item.action);
        } else if (item.action === 'open-folder') {
            openFolder();
        } else if (item.action === 'open-log') {
            if (files.fileExistsAtPath(log)) { sh('open -a Console ' + quote(log)); } else { alert('No log yet at ' + log, false); }
        } else if (item.action === 'restart') {
            var restarted = sh('launchctl kickstart -k "gui/$(id -u)/' + agent + '"');
            if (!restarted.ok) { alert('No LaunchAgent ' + agent + ' to restart.', false); }
        } else if (item.action === 'quit') {
            app.terminate(null);
            return;
        }
        update();
    }

    // A JavaScript error escaping into Cocoa would end the process silently.
    function safely(work) {
        try { work(); } catch (error) { sh('logger -t itsanas-menubar ' + quote(String(error))); }
    }

    ObjC.registerSubclass({
        name: 'ItsanasMenubarTarget',
        superclass: 'NSObject',
        methods: {
            'choose:': { types: ['void', ['id']], implementation: function (sender) { safely(function () { choose(sender.tag); }); } },
            'tick:': { types: ['void', ['id']], implementation: function () { safely(update); } },
            'watch:': { types: ['void', ['id']], implementation: function () { safely(checkWatched); } }
        }
    });
    target = $.ItsanasMenubarTarget.alloc.init;
    update();
    $.NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats(30, target, 'tick:', null, true);
    $.NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats(1, target, 'watch:', null, true);
    app.run;
    return '';
}

// Under node (no ObjC bridge): print the menu for check-installers.sh.
if (typeof ObjC === 'undefined' && typeof process !== 'undefined') {
    process.stdout.write(describeMain(process.argv.slice(2)) + '\n');
}
