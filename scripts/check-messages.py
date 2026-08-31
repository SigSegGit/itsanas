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
COLLAPSED = re.compile('[a-z] {8,}[a-z]')

# Residue from the scripts that write these edits. They splice an em dash in
# by string concatenation, and a splice that lands inside a triple-quoted
# block is copied through literally. Six files carried it in their comments
# and one was committed that way: it compiles, it sits inside a comment, and
# nothing in the toolchain reads prose.
RESIDUE = re.compile(r'\"{3}\s*\+|\+\s*\"{3}|\{\}\s*\.format\(|%\(\w+\)s')


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

    print('messages: no collapsed continuations, no editing residue')
    return 0


if __name__ == '__main__':
    sys.exit(main())
