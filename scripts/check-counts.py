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
* the section headings, which are a partition of each crate and have to add up
  to that crate's real count;
* the totals in the `docs/TESTING.md` header, including how many doctests
  there are;
* how many tests have an entry of their own, and a ceiling on how many do not,
  so that adding a test without one is a decision rather than an accident;
* the counts in `README.md` and `docs/ROADMAP.md`, and the number of CI jobs
  the roadmap claims.

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

# Every top-level section of the catalogue names a crate and says how many of
# that crate's tests it covers. Those numbers have to add up to the crate's real
# count: they are a partition of it, and five of them did not add up. One
# heading carried two numbers ("12 integration, 8 unit"), which made its crate
# look eight short until the split moved into the prose below it.
SECTION = re.compile(r'^# `([a-z0-9-]+)` — .+? \((\d+)')

# How many tests have an entry of their own. `check-catalogue.sh` reads this
# file the other way round -- every name cited must exist -- and nothing counted
# how many exist without being cited. The answer was 522 of 638, while the
# section headings added up to 627, so the file read as a complete catalogue and
# was not one. Stating the real figure is the difference between a gap and a
# claim.
#
# An *entry*, not a mention. The first version counted a name appearing anywhere
# in the file, which would have been satisfied by pasting it into a sentence --
# a debt counter payable in monopoly money. A name counts when it sits in the
# first cell of a table row whose last cell says something, which is what
# CONTRIBUTING.md asks for: every test gets an entry saying what it proves.
#
# Tightening it changed nothing today: all 525 cited names were already real
# entries. That is the moment to tighten a rule -- when it costs nothing and
# closes the door before somebody needs it open.
CITED = re.compile(r'`([a-z][a-z0-9_]+)`')
#
# The cited figure counts distinct *names*, and the total counts test functions;
# two names occur in two crates each, so this understates coverage by at most
# two. Conservative is the right direction for a claim about how complete
# something is.
COVERAGE_CLAIM = re.compile(
    r'(\d+) of the (\d+) tests have an entry of their own'
)

# A ratchet, and the reason for it is the shape of the check above rather than
# anything about the catalogue. Stating "522 of the 638" and checking both
# numbers means that adding an uncatalogued test fails on the *denominator*:
# the check says "the test total it is out of is 638; it is 639", and the
# cheapest way to satisfy it is to write 639 and never touch the 522. A gate
# that counts a debt and asks you politely to increment it is not a gate.
#
# So the number of tests with no entry has a ceiling, and the ceiling lives
# here rather than in prose. It may go down. Raising it means editing the file
# whose purpose is to stop you, which is the amount of friction the decision
# deserves -- not none, and not a refusal.
UNCATALOGUED_CEILING = 116

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



def catalogued(text, known):
    """Test names that have a table entry, not merely a mention."""
    found = set()
    for line in text.split(chr(10)):
        if not line.startswith('|'):
            continue
        cells = [cell.strip() for cell in line.strip().strip('|').split('|')]
        if len(cells) < 2:
            continue
        explanation = cells[-1]
        if not explanation or set(explanation) <= set('- '):
            continue
        for name in CITED.findall(cells[0]):
            if name in known:
                found.add(name)
    return found


def collect(root):
    """Return {(crate, kind): [total, ignored]}, the red-team total, and names."""
    found = {}
    names = set()
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
                names.add(match.group(2))
                if match.group(2).startswith('red_team_'):
                    red_team += 1
    return found, red_team, names


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
    found, red_team, test_names = collect(root)
    doctests = count_doctests(root)
    total = sum(entry[0] for entry in found.values())
    ignored = sum(entry[1] for entry in found.values())
    binaries = len(found)
    problems = []

    def read(name):
        return io.open(os.path.join(root, *name.split('/')), encoding='utf-8').read()

    testing = read('docs/TESTING.md')
    check_table(testing, found, problems)

    # The section headings, added up per crate.
    per_crate = {}
    for key, entry in found.items():
        per_crate[key[0]] = per_crate.get(key[0], 0) + entry[0]
    claimed_per_crate = {}
    for line in testing.split('\n'):
        heading = SECTION.match(line)
        if heading:
            crate = heading.group(1)
            claimed_per_crate[crate] = claimed_per_crate.get(crate, 0) + int(heading.group(2))
    for crate in sorted(set(per_crate) | set(claimed_per_crate)):
        claimed_here = claimed_per_crate.get(crate, 0)
        if not claimed_here:
            continue
        compare(
            'docs/TESTING.md',
            "the section headings for `%s` adding up" % crate,
            claimed_here,
            per_crate.get(crate, 0),
            problems,
        )

    # How many tests are catalogued individually.
    cited = catalogued(testing, test_names)
    claim = COVERAGE_CLAIM.search(' '.join(testing.split()))
    if not claim:
        problems.append(
            'docs/TESTING.md no longer says how many tests have an entry of '
            'their own. That number is what separates a catalogue from a '
            'sample, and it was never stated while the headings added up to '
            '627 out of 638.'
        )
    else:
        compare('docs/TESTING.md', 'the number catalogued individually',
                int(claim.group(1)), len(cited), problems)
        compare('docs/TESTING.md', 'the test total it is out of',
                int(claim.group(2)), total, problems)

    uncatalogued = total - len(cited)
    if uncatalogued > UNCATALOGUED_CEILING:
        problems.append(
            '%d tests have no entry of their own in docs/TESTING.md, and the '
            'ceiling in this script is %d. Write the entry, or raise the '
            'ceiling here on purpose -- but a test added without one is a debt '
            'taken out quietly.' % (uncatalogued, UNCATALOGUED_CEILING)
        )

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
        'red-team, %d catalogued individually; README, ROADMAP and TESTING '
        'all agree, and every section heading adds up'
        % (total, binaries, ignored, doctests, red_team, len(cited))
    )
    return 0


if __name__ == '__main__':
    sys.exit(main())
