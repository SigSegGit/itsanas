# -*- coding: utf-8 -*-
"""Check every test count the documentation states against the source.

Why this exists
---------------

Three files state how many tests this project has, and on the day this was
written all three were wrong and none agreed with the others:

    README.md      637 tests, 29 of them red-team
    docs/ROADMAP.md  589 test functions, sixteen of them red-team
    docs/TESTING.md  637 test functions across 21 binaries, twenty-nine red-team

The tree held 638 tests and 30 red-team ones. `docs/TESTING.md` also carries a
table of twenty-one per-binary counts, one of which (`itsanas-folder` unit) said
31 against a real 32 — which is exactly where the missing test in the totals had
gone. Nothing read any of it.

That matters more than the arithmetic. The numbers are the first thing somebody
sees, and "638 tests, 30 of them red-team" is the sentence the project offers in
place of the reader auditing it themselves. A number nobody checks is not
evidence; it is a claim that happens to be typeset like one. `docs/TESTING.md`
went as far as saying "three of these counts are checked mechanically by CI"
while naming three checks that verify names, messages and wiring, and no count
at all.

What is checked
---------------

* the per-binary table in `docs/TESTING.md`, row by row, including how many of
  each row's tests are `#[ignore]`d;
* that the table has a row for every binary that has tests, and no row for a
  binary that does not;
* the totals in the `docs/TESTING.md` header, including how many doctests
  there are;
* the counts in `README.md` and `docs/ROADMAP.md`.

The source is the authority: a test is a function carrying `#[test]` or
`#[tokio::test]` under `crates/`, and a red-team test is one whose name starts
with `red_team_`. Counting from the source rather than from `cargo test` output
keeps the answer the same on every platform, which matters because one test is
`#[cfg(unix)]` and Windows therefore runs one fewer than Linux does.
"""

import io
import os
import re
import sys

TEST = re.compile(
    r'#\[(?:tokio::)?test\][^\n]*\n((?:\s*#\[[^\]]*\][^\n]*\n)*)\s*(?:async\s+)?fn\s+([a-z_0-9]+)'
)

# A row is `crate` then a human label, then a cell that is a number and
# optionally how many of them are ignored. The label carries the source file in
# backticks for anything that is not the crate's own unit tests, which is what
# ties a row to a directory.
ROW = re.compile(r'^\|\s*`([a-z0-9-]+)`\s*([^|]*?)\s*\|\s*([^|]+?)\s*\|\s*$')
ROW_FILE = re.compile(r'`(tests/[a-z0-9_/]+\.rs)`')
CELL = re.compile(r'^(\d+)(?:\s+\((\d+)\s+`#\[ignore\]`d\))?$')

HEADER = re.compile(
    r'(\d+) test functions across (\d+) binaries, (\d+) of them '
    r'`#\[ignore\]`d, plus (\d+) doctests\. (\d+) are red-team tests'
)

# A fenced block in a doc comment is compiled and run unless its info string
# says otherwise, so counting them gives the same answer `cargo test` does
# without building anything.
DOC_LINE = re.compile(r'^\s*//[/!]')
RUNNABLE_INFO = ('', 'rust', 'no_run', 'should_panic')
README_CLAIM = re.compile(r'(\d+) tests, (\d+) of them red-team')

# The roadmap says how many jobs the workflow has. It said seven against a real
# eight, which is the same failure as the test counts in a different file, so it
# is read back the same way. A job is a two-space-indented key under `jobs:`.
JOB = re.compile(r'^  ([a-z][a-z0-9-]*):\s*$')
JOBS_CLAIM = re.compile(r'CI with (\w+) jobs')
WORDS = {
    'four': 4, 'five': 5, 'six': 6, 'seven': 7, 'eight': 8, 'nine': 9,
    'ten': 10, 'eleven': 11, 'twelve': 12,
}
ROADMAP_CLAIM = re.compile(
    r'(\d+) test functions, (\d+) of them `#\[ignore\]`d into the slow job, '
    r'and (\d+) of\s+them red-team'
)


def collect(root):
    """Return {(crate, kind): [total, ignored]} and the red-team total."""
    found = {}
    red_team = 0
    crates = os.path.join(root, 'crates')
    for dirpath, dirnames, filenames in os.walk(crates):
        dirnames[:] = [d for d in dirnames if d != 'target']
        for name in sorted(filenames):
            if not name.endswith('.rs'):
                continue
            path = os.path.join(dirpath, name)
            parts = os.path.relpath(path, crates).replace(os.sep, '/').split('/')
            crate = parts[0]
            if parts[1] == 'src':
                kind = 'unit'
            else:
                kind = '/'.join(parts[1:])
            text = io.open(path, encoding='utf-8', errors='replace').read()
            for match in TEST.finditer(text):
                entry = found.setdefault((crate, kind), [0, 0])
                entry[0] += 1
                if '#[ignore' in match.group(1):
                    entry[1] += 1
                if match.group(2).startswith('red_team_'):
                    red_team += 1
    return found, red_team


def count_doctests(root):
    total = 0
    crates = os.path.join(root, 'crates')
    for dirpath, dirnames, filenames in os.walk(crates):
        dirnames[:] = [d for d in dirnames if d != 'target']
        for name in sorted(filenames):
            if not name.endswith('.rs'):
                continue
            path = os.path.join(dirpath, name)
            inside = False
            for line in io.open(path, encoding='utf-8', errors='replace'):
                if not DOC_LINE.match(line):
                    continue
                body = line.strip()[3:].strip()
                if not body.startswith('```'):
                    continue
                if inside:
                    inside = False
                    continue
                inside = True
                if body[3:].strip() in RUNNABLE_INFO:
                    total += 1
    return total


def check_table(text, found, problems):
    """Compare the per-binary table against the source, both ways."""
    seen = set()
    for line in text.split('\n'):
        match = ROW.match(line)
        if not match:
            continue
        crate, label, cell = match.group(1), match.group(2), match.group(3)
        if not crate.startswith('itsanas-'):
            continue
        found_file = ROW_FILE.search(label)
        kind = found_file.group(1) if found_file else 'unit'
        numbers = CELL.match(cell)
        if not numbers:
            problems.append(
                'docs/TESTING.md: cannot read the count for `%s` %s: "%s"'
                % (crate, kind, cell)
            )
            continue
        claimed = int(numbers.group(1))
        claimed_ignored = int(numbers.group(2) or 0)
        seen.add((crate, kind))
        real = found.get((crate, kind))
        if real is None:
            problems.append(
                'docs/TESTING.md has a row for `%s` %s, which has no tests'
                % (crate, kind)
            )
            continue
        if claimed != real[0]:
            problems.append(
                'docs/TESTING.md says `%s` %s has %d tests; it has %d'
                % (crate, kind, claimed, real[0])
            )
        if claimed_ignored != real[1]:
            problems.append(
                'docs/TESTING.md says `%s` %s has %d `#[ignore]`d; it has %d'
                % (crate, kind, claimed_ignored, real[1])
            )
    for key in sorted(found):
        if key not in seen:
            problems.append(
                'docs/TESTING.md has no row for `%s` %s, which has %d tests'
                % (key[0], key[1], found[key][0])
            )


def compare(where, what, claimed, real, problems):
    if claimed != real:
        problems.append('%s says %s is %d; it is %d' % (where, what, claimed, real))


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
    found, red_team = collect(root)
    doctests = count_doctests(root)
    total = sum(entry[0] for entry in found.values())
    ignored = sum(entry[1] for entry in found.values())
    binaries = len(found)
    problems = []

    def read(name):
        return io.open(os.path.join(root, *name.split('/')), encoding='utf-8').read()

    testing = read('docs/TESTING.md')
    check_table(testing, found, problems)

    flat = ' '.join(testing.split())
    header = HEADER.search(flat)
    if not header:
        problems.append(
            'docs/TESTING.md: the header no longer states its counts in the '
            'form this check reads. Restore it or teach HEADER the new one.'
        )
    else:
        compare('docs/TESTING.md', 'the test total', int(header.group(1)), total, problems)
        compare('docs/TESTING.md', 'the binary count', int(header.group(2)), binaries, problems)
        compare('docs/TESTING.md', 'the ignored count', int(header.group(3)), ignored, problems)
        compare('docs/TESTING.md', 'the doctest count', int(header.group(4)), doctests, problems)
        compare('docs/TESTING.md', 'the red-team count', int(header.group(5)), red_team, problems)

    claim = README_CLAIM.search(' '.join(read('README.md').split()))
    if not claim:
        problems.append('README.md no longer states "N tests, M of them red-team"')
    else:
        compare('README.md', 'the test total', int(claim.group(1)), total, problems)
        compare('README.md', 'the red-team count', int(claim.group(2)), red_team, problems)

    workflow = read('.github/workflows/ci.yml')
    inside_jobs = False
    jobs = 0
    for line in workflow.split('\n'):
        if line.startswith('jobs:'):
            inside_jobs = True
            continue
        if inside_jobs:
            if line and not line.startswith(' '):
                inside_jobs = False
            elif JOB.match(line):
                jobs += 1

    roadmap = ' '.join(read('docs/ROADMAP.md').split())
    claim = JOBS_CLAIM.search(roadmap)
    if not claim:
        problems.append('docs/ROADMAP.md no longer says how many CI jobs there are')
    else:
        word = claim.group(1)
        stated = WORDS.get(word, int(word) if word.isdigit() else None)
        if stated is None:
            problems.append(
                'docs/ROADMAP.md says "CI with %s jobs", which is not a number '
                'this check knows' % word
            )
        else:
            compare('docs/ROADMAP.md', 'the CI job count', stated, jobs, problems)

    claim = ROADMAP_CLAIM.search(roadmap)
    if not claim:
        problems.append('docs/ROADMAP.md no longer states its test counts')
    else:
        compare('docs/ROADMAP.md', 'the test total', int(claim.group(1)), total, problems)
        compare('docs/ROADMAP.md', 'the ignored count', int(claim.group(2)), ignored, problems)
        compare('docs/ROADMAP.md', 'the red-team count', int(claim.group(3)), red_team, problems)

    if problems:
        print('the documentation states test counts the source does not support:')
        for problem in problems:
            print('  %s' % problem)
        print('')
        print('The source is the authority. Numbers written by hand drift, and')
        print('these three files had drifted apart from each other as well as')
        print('from the tree: a count nobody checks reads as evidence and is not.')
        return 1

    print(
        'counts: %d tests in %d binaries, %d ignored, %d doctests, %d '
        'red-team; README, ROADMAP and TESTING all agree'
        % (total, binaries, ignored, doctests, red_team)
    )
    return 0


if __name__ == '__main__':
    sys.exit(main())
