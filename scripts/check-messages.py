# -*- coding: utf-8 -*-
"""Find messages that `cargo fmt` has collapsed a line continuation into.

Why this exists
---------------

Rust lets a string literal be continued across lines with a trailing backslash,
which skips the newline *and* the next line's indentation. It is the natural way
to write a long message inside indented code, and this project used it
everywhere.

`cargo fmt` removes the backslash and joins the lines, keeping the indentation
as literal spaces. The message still compiles, the tests still pass, `clippy`
says nothing, and what the user sees is:

    9a1... is no longer being sent new data. It still receives          the log

Every backslash continuation in this repository had been eaten that way before
anybody looked at the output. One of them was the only line an operator sees at
the moment a peer is sanctioned. Nothing else in the toolchain reads the inside
of a string, so this does.

The rule
--------

A run of eight or more spaces **between two lowercase letters**, in a literal
longer than ninety characters. Both halves matter. The space run is the
signature of the damage; the length is what separates it from a status line
that pads its columns by hand, because fmt only ever joins lines that did not
fit, so what it produces is always long.

It deliberately does not fire on hand-aligned output ("  peers           none
configured") or on the box-drawn banners in `login`, where the padding runs up
against a border character rather than a letter. Those are formatting somebody
chose.

The fix is `concat!("...", "...")`, which fmt leaves alone.
"""

import io
import os
import re
import sys

LITERAL = re.compile('"(?:[^"' + chr(92) * 2 + ']|' + chr(92) * 2 + '.)*"')
# A run of spaces mid-sentence. The first version required a *lowercase*
# letter before the run, which was the shape of the two examples it was
# written from. A continuation is cut where a sentence breathes, so the
# character before it is as often a semicolon or a comma — and
# `server.rs` carried "requests;<eighteen spaces>open another" in the repo
# while this file reported everything clean. Calibrating a rule on the
# examples in front of you is the same mistake as a test that passes for
# the wrong reason.
COLLAPSED = re.compile('[^ ] {8,}[a-z]')

# Residue from the scripts that write these edits. They splice an em dash in
# by string concatenation, and a splice that lands inside a triple-quoted
# block is copied through literally. Six files carried it in their comments
# and one was committed that way: it compiles, it sits inside a comment, and
# nothing in the toolchain reads prose.
RESIDUE = re.compile(r'\"{3}\s*\+|\+\s*\"{3}|\{\}\s*\.format\(|%\(\w+\)s')

# Where a continuation gets eaten outside Rust.
#
# `cargo fmt` is not the only thing here that removes a trailing backslash. The
# scripts that write large edits splice text through Python strings, and a
# backslash before a newline is a line continuation there too: it and the
# newline vanish, leaving the next line's indentation behind. That is how
# `.github/workflows/ci.yml` came to hold
#
#     cargo check --target aarch64-linux-android             -p itsanas-crypto
#
# from the day it was written. The shell collapses the run of spaces, so the
# command still did the right thing and nothing ever complained. It was found
# by reading the file, which is not a method. The same accident one argument to
# the left, or inside a quoted string, would not have been survivable.
#
# Commands get their own rule rather than reusing COLLAPSED. That one wants a
# letter after the run of spaces, because prose is what it reads, and it does
# not match the case this section was written for: what followed the eaten
# backslash in ci.yml was `-p`, a flag. Reusing it would have produced a gate
# that passed the very line that motivated it — and it did, once, before this
# was run against the damage instead of only against the clean tree.
#
# So: any two non-spaces separated by eight or more spaces. No length floor
# either, because a command is not padded to fit. Comments are skipped (one of
# them quotes the damage on purpose), and so are heredoc bodies and PowerShell
# here-strings, which is where every hand-aligned column in this repository
# lives. Run against the tree as it stands, that leaves no false positives and
# three real hits nothing had ever looked for.
COLLAPSED_COMMAND = re.compile('[^ ] {8,}[^ ]')
COMMAND_DIRS = ('.github/workflows', 'install', 'scripts')
COMMAND_SUFFIXES = ('.yml', '.yaml', '.sh', '.ps1')
HEREDOC = re.compile('<<-?[ ]*[' + chr(34) + chr(39) + ']?([A-Za-z_][A-Za-z0-9_]*)')
HERESTRING_OPEN = re.compile('@[' + chr(34) + chr(39) + ']')
HERESTRING_CLOSE = re.compile('^[' + chr(34) + chr(39) + ']@')


def command_lines(path):
    """Yield (number, line) for lines that are actually commands.

    Everything a human aligned by hand in this repository sits inside a heredoc
    or a here-string, so skipping those bodies is what makes the rule usable.
    """
    terminator = None
    in_herestring = False
    text = io.open(path, encoding='utf-8', errors='replace').read()
    for number, line in enumerate(text.split('\n'), 1):
        stripped = line.strip()
        if terminator is not None:
            if stripped == terminator:
                terminator = None
            continue
        if in_herestring:
            if HERESTRING_CLOSE.match(stripped):
                in_herestring = False
            continue
        if not stripped.startswith('#'):
            yield number, line
        found = HEREDOC.search(line)
        if found:
            terminator = found.group(1)
        elif HERESTRING_OPEN.search(line):
            in_herestring = True


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
    hits = []
    residue = []
    for dirpath, dirnames, filenames in os.walk(os.path.join(root, 'crates')):
        dirnames[:] = [d for d in dirnames if d != 'target']
        for name in sorted(filenames):
            if not name.endswith('.rs'):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, root).replace(os.sep, '/')
            text = io.open(path, encoding='utf-8', errors='replace').read()
            for number, line in enumerate(text.split('\n'), 1):
                if RESIDUE.search(line):
                    residue.append((rel, number, line.strip()))
                for match in LITERAL.finditer(line):
                    if len(match.group(0)) <= 90:
                        continue
                    if COLLAPSED.search(match.group(0)[1:-1]):
                        hits.append((rel, number, match.group(0)))

    commands = []
    for relative_dir in COMMAND_DIRS:
        base = os.path.join(root, *relative_dir.split('/'))
        for dirpath, dirnames, filenames in os.walk(base):
            for name in sorted(filenames):
                if not name.endswith(COMMAND_SUFFIXES):
                    continue
                path = os.path.join(dirpath, name)
                rel = os.path.relpath(path, root).replace(os.sep, '/')
                for number, line in command_lines(path):
                    if COLLAPSED_COMMAND.search(line):
                        commands.append((rel, number, line.strip()))

    if residue:
        print('source carrying residue from the scripts that edited it:')
        for rel, number, line in residue:
            print('  %s:%d' % (rel, number))
            print('    %s' % line[:120].encode('ascii', 'replace').decode())
        print('')
        print('A splice like \'""" + D + """\' inside a triple-quoted block is')
        print('copied through literally. It compiles, it sits in a comment, and')
        print('nothing else in the toolchain reads prose.')
        return 1

    if hits:
        print('messages with a line continuation collapsed into them:')
        for rel, number, text in hits:
            print('  %s:%d' % (rel, number))
            print('    %s' % text[:140].encode('ascii', 'replace').decode())
        print('')
        print('That is what `cargo fmt` does to a trailing backslash: it removes')
        print('the backslash and keeps the indentation as literal spaces. The')
        print('code compiles, clippy is silent, and the user reads the spaces.')
        print('Use concat!("...", "...") instead, which fmt leaves alone.')
        return 1

    if commands:
        print('commands with a line continuation collapsed into them:')
        for rel, number, line in commands:
            print('  %s:%d' % (rel, number))
            print('    %s' % line[:140].encode('ascii', 'replace').decode())
        print('')
        print('A trailing backslash is a line continuation in Python too, so a')
        print('script that splices this file through one eats it. The shell then')
        print('collapses the spaces and the command still runs, which is why this')
        print('survived unnoticed in ci.yml from the day it was written.')
        print('')
        print('In YAML, use a folded scalar (run: >-) and drop the backslashes')
        print('entirely: continuations that do not exist cannot be eaten.')
        return 1

    print('messages: no collapsed continuations, no editing residue')
    return 0


if __name__ == '__main__':
    sys.exit(main())
