# -*- coding: utf-8 -*-
"""Every capability has a row in docs/PARITY.md, and the row is true to the source.

Why this exists
---------------

On 2026-11-01 a novice test on Android found regressions against the PC
version, and Nicolas wrote: "it is a serious problem that the features are not
aligned between the platforms". The answer is one interface -- a feature lives
in the library and a shell draws only what it alone can -- and that answer is
only as good as the list of what each platform has. A list written once and read
never is the failure `check-counts.py` and `check-bargain.py` exist to stop, so
this reads the source and refuses a table that has drifted.

What is checked
---------------

* every variant of the clap `Command` enum (`itsanas <name>`) has a row;
* every `<h1>` and `<h2>` of the setup page has a row that names it;
* every `/api/` route of the page's server has a row that names it, in backticks;
* every field of `Features` has a row `feature:<name>`; its Android cell is `✅`
  exactly when `Features::PHONE` has it, and its three desktop cells exactly
  when `Features::DESKTOP` has it;
* every cell is `✅`, or `⬜` followed by a reason of at least three words.

A new command, heading, route or feature without a row fails with the line to
add; a row about something that no longer exists is not an error, because a
table that is longer than the code is still true.

Run from anywhere: `python scripts/check-parity.py`.
"""

import os
import re
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
CLI = os.path.join(ROOT, 'crates', 'itsanas-cli', 'src')
PLATFORMS = ['Windows', 'macOS', 'Linux', 'Android']


def read(*parts):
    with open(os.path.join(*parts), encoding='utf-8') as handle:
        return handle.read()


def kebab(name):
    return re.sub(r'(?<!^)(?=[A-Z])', '-', name).lower()


def commands(source):
    """The subcommands of the clap `Command` enum, as typed after `itsanas`."""
    start = re.search(r'^(?:pub(?:\(crate\))? )?enum Command \{\n', source, re.M)
    if not start:
        sys.exit('check-parity: the Command enum is not where this script looks (lib.rs)')
    body = source[start.end():]
    body = body[:body.index('\n}\n')]
    found, pending = [], None
    for line in body.splitlines():
        named = re.match(r'    #\[command\(name = "([^"]+)"', line)
        if named:
            pending = named.group(1)
            continue
        variant = re.match(r'    ([A-Z]\w*)\b', line)
        if variant:
            found.append(pending or kebab(variant.group(1)))
            pending = None
    return found


def headings(html):
    found = []
    for text in re.findall(r'<h[12][^>]*>(.*?)</h[12]>', html, re.S):
        text = re.sub(r'<[^>]+>', '', text).strip()
        if text:
            found.append(text)
    return found


def routes(source):
    return sorted(set(re.findall(r'"(/api/[a-z-]+)"', source)))


def features(source):
    """The Features fields, and what DESKTOP and PHONE say of each."""
    fields = re.findall(r'^    pub (\w+): bool,', source, re.M)

    def block(name):
        match = re.search(r'pub const %s: Self = Self \{(.*?)\};' % name, source, re.S)
        if not match:
            sys.exit('check-parity: Features::%s is not where this script looks' % name)
        return dict(re.findall(r'(\w+): (true|false)', match.group(1)))

    as_bool = lambda found: {name: value == 'true' for name, value in found.items()}
    return fields, as_bool(block('DESKTOP')), as_bool(block('PHONE'))


def table(text):
    """Rows of every table: (first cell, [four platform cells], line number)."""
    rows = []
    for number, line in enumerate(text.splitlines(), 1):
        if not line.startswith('|') or set(line.replace('|', '').strip()) <= set('-: '):
            continue
        cells = [cell.strip() for cell in line.strip().strip('|').split('|')]
        if cells[0] in ('Capability', 'Command'):
            continue
        rows.append((cells[0], cells[1:], number))
    return rows


def main():
    # The table holds ✅ and ⬜; a console that is not UTF-8 would crash on them.
    if hasattr(sys.stdout, 'reconfigure'):
        sys.stdout.reconfigure(encoding='utf-8')
    problems = []
    parity = read(ROOT, 'docs', 'PARITY.md')
    rows = table(parity)

    for first, cells, number in rows:
        if len(cells) != len(PLATFORMS):
            problems.append('docs/PARITY.md:%d: "%s" has %d platform cells, not %d'
                            % (number, first, len(cells), len(PLATFORMS)))
            continue
        for platform, cell in zip(PLATFORMS, cells):
            if cell == '✅':
                continue
            if not cell.startswith('⬜'):
                problems.append('docs/PARITY.md:%d: "%s", %s: "%s" is neither ✅ nor ⬜ '
                                'followed by a reason' % (number, first, platform, cell))
            elif len(cell[1:].split()) < 3:
                problems.append('docs/PARITY.md:%d: "%s", %s: ⬜ with no reason. Say why in a '
                                'fact, e.g. "a phone keeps one account"' % (number, first, platform))

    def named(token):
        return any(token in first for first, _, _ in rows)

    for name in commands(read(CLI, 'lib.rs')):
        if not named('`itsanas %s`' % name):
            problems.append('add a row to docs/PARITY.md for the command: | `itsanas %s` | ✅ | ✅ | ✅ | '
                            '✅ or ⬜ and why |' % name)
    for heading in headings(read(CLI, 'setup', 'web', 'index.html')):
        if not named(heading):
            problems.append('add a row to docs/PARITY.md that names the page heading "%s"' % heading)
    for route in routes(read(CLI, 'setup', 'web', 'mod.rs')):
        if not named('`%s`' % route):
            problems.append('add a row to docs/PARITY.md that names the route `%s`' % route)

    fields, desktop, phone = features(read(CLI, 'setup', 'web', 'features.rs'))
    for field in fields:
        row = [(cells, number) for first, cells, number in rows if '`feature:%s`' % field in first]
        if not row:
            problems.append('add a row to docs/PARITY.md: | `feature:%s` | ... |' % field)
            continue
        cells, number = row[0]
        if len(cells) != len(PLATFORMS):
            continue
        expected = [desktop[field]] * 3 + [phone[field]]
        for platform, cell, has in zip(PLATFORMS, cells, expected):
            if cell.startswith('✅') != has:
                problems.append(
                    'docs/PARITY.md:%d: `feature:%s` says %s for %s, and Features::%s says %s'
                    % (number, field, 'present' if cell.startswith('✅') else 'absent', platform,
                       'PHONE' if platform == 'Android' else 'DESKTOP',
                       'present' if has else 'absent'))

    if problems:
        print('docs/PARITY.md does not say what the source does:')
        for problem in problems:
            print('  ' + problem)
        print()
        print('This table is what the owner reads to know what each platform has. Fix the')
        print('table, or the code that made it untrue; do not silence the check.')
        return 1
    print('check-parity: %d rows; every command, page heading, route and feature has one, '
          'and the features agree with Features::DESKTOP and PHONE' % len(rows))
    return 0


if __name__ == '__main__':
    sys.exit(main())
