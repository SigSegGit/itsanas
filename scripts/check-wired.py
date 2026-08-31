# -*- coding: utf-8 -*-
"""Find public methods that nothing in the workspace calls.

Why this exists
---------------

Four times in one session a mechanism was designed, implemented, given tests,
documented, and wired to nothing: `itsanas-policy` sat unused for weeks holding
the sync behaviour the Android shell was meant to inherit; `unreliable_devices`
and `Reliability::complaint` were written to report failing peers and never
reached a status line; `PushReport::push_bytes` was an accessor for a public
field. Each was found by a human reading the code and noticing, which is not a
process.

It matters more here than in a library. ITSaNAS has no external consumers: the
workspace is the only caller there is, so "no call site" and "dead" are the same
statement. A `pub fn` nobody calls is either work that was never finished or
work that was finished and forgotten, and the two are indistinguishable from
outside.

Three things it deliberately gets right
---------------------------------------

**Comments do not count.** A doc link like ``[`Self::plan`]`` reads exactly like
a call site to a regex, and `itsanas-policy::plan` spent weeks unused with its
own documentation pointing at it.

**A wrapper does not vouch for what it wraps.** This codebase delegates in
layers: `Index::x`, wrapped by `Store::x`, called by a CLI command. Counting
`Store::x`'s call to `Index::x` lets a dead chain certify itself. Deleting the
only real caller of `unreliable_devices` left the two wrappers pointing at each
other and the first version of this check reported the wiring as fine. So the
body of a function is not a call site for a function of the same name.

**Two types may share a method name.** `moves_content` exists on two unrelated
enums, `observe` on a census and on a version vector. Matching by bare name and
then excluding every file that defines any of them flags all of them. Only the
same-named *body* is skipped, never the whole file.

What it does not catch: a function called only from a path that never runs.
`itsanas-policy::plan` would have passed this check the day it was written,
because its tests call it. Tests are counted deliberately anyway — a mechanism
with tests and no production caller is still evidence of intent, and excluding
them would flag every property this project pins down.

The allowlist is for work deferred by decision. Each entry says what would wire
it, so the list reads as unfinished work rather than as excuses.
"""

import io
import os
import re
import sys

ALLOWED = {
    # Kept for file sharing, which ECONOMICS.md §9 defers explicitly. Removing
    # and re-adding cryptographic code is how mistakes enter.
    'agree': 'key agreement, kept for the sharing ECONOMICS.md defers',
    'serialize': 'identity serialisation, kept with the sharing primitives',

    # The coordinator's entitlement and anchor bookkeeping: specified and tested,
    # inert because nothing writes usage and nothing chooses anchors.
    # ECONOMICS.md §3 records it as "partly built"; DESIGN.md records anchors as
    # "decided, not built".
    'report_usage': 'needs a Usage message in coord::protocol, reported each round',
    'set_over_since': 'needs report_usage first; nothing exceeds an unrecorded quota',
    'is_anchor': 'needs the anchor placement rule, which DESIGN.md defers',
    'needs_attention': 'needs a coordinator-to-member notice channel, which does not exist',

    # Tombstones are never pruned, so the table grows for the life of the
    # account: one small record per file ever deleted. This is the method that
    # would prune them and it cannot be called safely yet, because "every device
    # has seen the delete" needs a membership list this design deliberately does
    # not have. Recorded in ROADMAP.md so the gap lives somewhere a reader will
    # find it, not only in this file.
    'forget_tombstone': 'needs proof every device saw the delete; see ROADMAP.md',
}

# Methods (indented inside an `impl`) and free functions (at module level).
# The first version matched only the indented form, which left the whole of
# `session.rs` outside the gate: `push`, `pull`, `round`, `repair` and
# `drain_vault` are free functions, and so is `placement::repair::plan`, the
# canonical example of the problem this gate exists for. It reported that
# every public method had a call site while covering half the surface.
DEF = re.compile(r'\n(?:    )?pub (?:const )?(?:async )?fn (\w+)')


# A `#[cfg(test)]` at module indentation opens the test module. One indented
# inside an `impl` is a test-only helper method, and cutting there would hide
# every method defined after it. `index.rs` has such a helper a quarter of the
# way down, so the first version of this check silently skipped three quarters
# of the file — including the method whose deletion it was supposed to catch.
TEST_MODULE = re.compile('\\n#\\[cfg\\(test\\)\\]')


def production_half(text):
    """Everything before the test module, which is not the same as before the
    first `#[cfg(test)]`."""
    match = TEST_MODULE.search(text)
    return text[: match.start()] if match else text


def strip_comments(text):
    """Blank `//`, `///` and `//!` lines, keeping the line count intact."""
    return '\n'.join(
        '' if line.lstrip().startswith('//') else line for line in text.split('\n')
    )


def without_own_body(text, name):
    """Blank the body of any `pub fn <name>`, so a wrapper cannot vouch for itself.

    A body runs from its signature to the first line that closes at method
    indentation. Crude, and adequate: the wrappers this exists for are one
    expression long.
    """
    lines = text.split('\n')
    signature = re.compile(r'^\s*pub (?:const )?(?:async )?fn ' + re.escape(name) + r'\b')
    inside = False
    for i, line in enumerate(lines):
        if not inside:
            if signature.match(line):
                inside = True
                lines[i] = ''
            continue
        closing = line in ('    }', '}')
        lines[i] = ''
        if closing:
            inside = False
    return '\n'.join(lines)


def rust_files(root):
    for dirpath, dirnames, filenames in os.walk(os.path.join(root, 'crates')):
        dirnames[:] = [d for d in dirnames if d != 'target']
        for name in sorted(filenames):
            if name.endswith('.rs'):
                path = os.path.join(dirpath, name)
                yield os.path.relpath(path, root).replace(os.sep, '/'), path


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')

    code = {}
    definitions = {}
    for rel, path in rust_files(root):
        text = io.open(path, encoding='utf-8', errors='replace').read()
        code[rel] = strip_comments(text)
        # Definitions from the non-test half of the file; call sites from all of
        # it, because a test is a caller.
        for match in DEF.finditer(production_half(text)):
            definitions.setdefault(match.group(1), set()).add(rel)

    unwired = []
    for name, homes in sorted(definitions.items()):
        if name in ALLOWED:
            continue
        # A call, a path used as a function value (`Cloud::method`, the form
        # clippy insists on), or a method reference.
        pattern = re.compile(
            r'\b' + re.escape(name) + r'\s*\(|::\s*' + re.escape(name)
            + r'\b|\.\s*' + re.escape(name) + r'\b'
        )
        calls = 0
        for rel, text in code.items():
            searchable = without_own_body(text, name) if rel in homes else text
            calls += len(pattern.findall(searchable))
        if calls == 0:
            unwired.append((name, ', '.join(sorted(homes))))

    if unwired:
        print('public functions no code in this workspace calls:')
        for name, homes in unwired:
            print('  %-40s %s' % (name, homes))
        print('')
        print('This workspace is the only consumer there is, so a `pub fn` with')
        print('no call site is either unfinished work or forgotten work, and the')
        print('two look identical from outside. Wire it, delete it, or add it to')
        print('ALLOWED in this script with the reason it is deliberately kept.')
        return 1

    print('wiring: every public function and method has a call site')
    return 0


if __name__ == '__main__':
    sys.exit(main())
