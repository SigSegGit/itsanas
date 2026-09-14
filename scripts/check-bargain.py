# -*- coding: utf-8 -*-
"""The split in the code is the split every document states.

Why this exists
---------------

On 2026-09-14 the contribution ratio stopped being `CONTRIBUTION_RATIO = 3` and
became `Split::DEFAULT`, 30/70. The code changed, the tests changed, and
`ECONOMICS.md` §1 changed. **Nine other places did not**, and they were not
obscure ones:

    FIRST-STEPS.md          "Keeping a byte of your own costs three pledged"
    docs/QUICKSTART.md      the same sentence
    install/README.md       the same sentence, in bold
    install/provision.sh    the same sentence, in a comment
    install/provision.ps1   the same sentence, in a comment
    docs/ECONOMICS.md §3    "the 3x ratio appears on its own"
    docs/DESIGN.md          the same bullet, and one more reference

`FIRST-STEPS.md` is the file a new member opens first. For the length of one
commit, the first paragraph anybody read about the bargain stated a number the
software no longer applied — and the commit that did that also updated three
documents, ran nine gates, and was green.

Two of those nine were worse than stale. "Wanting three replicas of 100 GB means
finding three counterparties and giving each of them 100 GB back" reproduces the
exact arithmetic error §1 had just been corrected for: the third copy is the
one already on the owner's own disk, so it is **two** counterparties, not three.
The wrong derivation had been sitting in the bilateral-model section of two
documents the whole time, agreeing with §1 and therefore invisible.

That is the failure this file exists to stop, and it is the same one
`check-counts.py` stops for test counts: a number that lives in the code and is
restated in prose in nine places, with nothing comparing them.

What is checked
---------------

* the split the code actually ships, read out of `accounting.rs`;
* that every document which tells a reader what the bargain *is* states that
  split, written as `own/network`;
* that no document states a *different* split next to the word "split";
* that the sentences which were wrong on 2026-09-14 have not come back.

The third rule is the general one and the first two are the cheap ones. The
fourth is a list of specific sentences, which is a weaker kind of rule — it
catches a regression, not an invention. It is here because those exact sentences
were copied between five files once already, and a rule that only catches what
has actually happened is still worth more than the comment nobody read.

Historical mentions are allowed where a document explains what the number used
to be, which several deliberately do. They are listed by file and phrase below,
so that adding one is a decision rather than an accident.
"""

import io
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')

# `pub const DEFAULT: Self = Self { own: 30, network: 70 };`
DEFAULT = re.compile(
    r'pub const DEFAULT: Self = Self \{\s*own: (\d+),\s*network: (\d+),?\s*\}'
)

# Files that tell a reader what the bargain is, rather than mentioning it in
# passing. Each has to state the current split. `README.md` and `FIRST-STEPS.md`
# are here because they are what somebody reads before installing anything.
STATES_THE_BARGAIN = (
    'README.md',
    'FIRST-STEPS.md',
    'docs/QUICKSTART.md',
    'docs/ECONOMICS.md',
    'install/README.md',
)

# A split written next to the word "split" is a claim about this constant.
A_SPLIT = re.compile(r'(\d{1,3})/(\d{1,3})(?=[^\d/]{0,40}?split)|split[^\d\n]{0,40}?(\d{1,3})/(\d{1,3})')

# The sentences that were wrong, kept so they cannot return.
STALE = (
    'costs three pledged',
    'a byte of your own costs three',
    '3-for-1',
    '3x ratio',
    '3x contribution ratio',
    'pledge three times what you store',
    'pledged for each byte you keep',
)

# Worked examples. The ratios above were all corrected on 2026-09-14 and the
# figures beside them were not: "offering 90 GiB earns 30 GiB, and 31 GiB is
# refused" survived in two documents, with a sample refusal quoting 93 GiB, and
# ECONOMICS.md still wrote entitlement as "÷ 3" over examples of 333 and 83 GB.
# A sentence can state 30/70 and then do 25/75 arithmetic, and the check above
# reads only the first half. These recompute the second.
GIB = 1024 ** 3
NEEDS = re.compile(r'keeping (\d+)(?:\.(\d))? GiB needs (\S+) pledged')
OFFERING = re.compile(r'offering (\d+) GiB earns (\d+) GiB')
EARNS_YOU = re.compile(r'you offer\s+(\d+)\.0 GiB\s*\n\s*that earns you\s+(\d+\.\d) GiB')
CONTRIBUTES = re.compile(r'contributes (\d+) (GB|TB) and earns (\d+) GB')
DIVIDED = re.compile(r'effective contribution ÷ \d+\s*$', re.M)

# Code comments that quote the refusal, alongside the documents.
EXAMPLES_IN_CODE = ('crates/itsanas-coord/src/accounting.rs',)


def price(keep, own, network):
    """`Split::pledge_needed_for`: rounded up."""
    needed = keep * network
    return needed // own + (needed % own != 0)


def argument(size):
    """`itsanas_node::config::size_argument`: a whole unit, rounded up."""
    if size >= 1024 ** 4 and size % 1024 ** 4 == 0:
        return '%dT' % (size // 1024 ** 4)
    for suffix, unit in (('G', GIB), ('M', 1024 ** 2), ('K', 1024)):
        if size >= unit:
            return '%d%s' % (-(-size // unit), suffix)
    return str(size)


def check_examples(name, text, own, network, problems):
    """Recompute every worked example in `text`; return how many were checked."""
    checked = 0

    def line(match):
        return text.count('\n', 0, match.start()) + 1

    for match in NEEDS.finditer(text):
        if match.group(2) not in (None, '0'):
            continue  # a floored tenth cannot be read back to the bytes asked for
        checked += 1
        expected = argument(price(int(match.group(1)) * GIB, own, network))
        if match.group(3) != expected:
            problems.append(
                '%s:%d quotes %s as the price of %s GiB; at %d/%d it is %s'
                % (name, line(match), match.group(3), match.group(1), own, network, expected)
            )
    for match in OFFERING.finditer(text):
        checked += 1
        earned = int(match.group(1)) * own // network
        if int(match.group(2)) != earned:
            problems.append(
                '%s:%d says %s GiB earns %s GiB; at %d/%d it earns %d'
                % (name, line(match), match.group(1), match.group(2), own, network, earned)
            )
    for match in EARNS_YOU.finditer(text):
        checked += 1
        earned = int(match.group(1)) * GIB * own // network
        shown = '%d.%d' % (earned // GIB, (earned % GIB) * 10 // GIB)
        if match.group(2) != shown:
            problems.append(
                '%s:%d shows %s GiB offered earning %s GiB; the CLI prints %s'
                % (name, line(match), match.group(1), match.group(2), shown)
            )
    for match in CONTRIBUTES.finditer(text):
        checked += 1
        contributed = int(match.group(1)) * (1000 if match.group(2) == 'TB' else 1)
        earned = contributed * own // network
        if int(match.group(3)) != earned:
            problems.append(
                '%s:%d says %s %s earns %s GB; at %d/%d it earns %d'
                % (name, line(match), match.group(1), match.group(2), match.group(3),
                   own, network, earned)
            )
    for match in DIVIDED.finditer(text):
        problems.append(
            '%s:%d writes entitlement as a division by a single ratio; the code '
            'multiplies by %d and divides by %d' % (name, line(match), own, network)
        )
    return checked

# Where a stale phrase is deliberate, because the document is explaining what
# the number used to be. File, then the phrase it is allowed to contain.
HISTORY = {
    ('docs/ECONOMICS.md', 'pledge three times what you store'),
}

# Where an old split may be named, for the same reason.
OLD_SPLITS_ALLOWED = (
    'docs/ECONOMICS.md',
    'docs/HANDOVER.md',
    'docs/TESTING.md',
    'crates/itsanas-coord/src/accounting.rs',
    'crates/itsanas-node/src/config.rs',
)


def read(name):
    with io.open(os.path.join(ROOT, *name.split('/')), encoding='utf-8') as handle:
        return handle.read()


def documents():
    """Every document and installer, discovered rather than listed."""
    for folder in ('.', 'docs', 'install', 'scripts'):
        base = os.path.join(ROOT, folder)
        if not os.path.isdir(base):
            continue
        for name in sorted(os.listdir(base)):
            if name.endswith(('.md', '.sh', '.ps1')):
                path = folder + '/' + name if folder != '.' else name
                yield path, read(path)


def main():
    source = read('crates/itsanas-coord/src/accounting.rs')
    found = DEFAULT.search(source)
    if not found:
        print('cannot find `Split::DEFAULT` in accounting.rs, so nothing below')
        print('is checking anything. Fix the pattern in this script.')
        return 1

    own, network = found.group(1), found.group(2)
    split = '%s/%s' % (own, network)
    problems = []

    for name in STATES_THE_BARGAIN:
        if split not in read(name):
            problems.append(
                '%s tells a reader what the bargain is and never says %s'
                % (name, split)
            )

    examples = 0
    for name in EXAMPLES_IN_CODE:
        examples += check_examples(name, read(name), int(own), int(network), problems)

    for name, text in documents():
        examples += check_examples(name, text, int(own), int(network), problems)
        for number, line in enumerate(text.split('\n'), 1):
            for phrase in STALE:
                if phrase in line and (name, phrase) not in HISTORY:
                    problems.append(
                        '%s:%d states the old bargain: %r' % (name, number, phrase)
                    )
            if name in OLD_SPLITS_ALLOWED:
                continue
            for match in A_SPLIT.finditer(line):
                left = match.group(1) or match.group(3)
                right = match.group(2) or match.group(4)
                if (left, right) != (own, network):
                    problems.append(
                        '%s:%d names a %s/%s split; the code ships %s'
                        % (name, number, left, right, split)
                    )

    # The documents carry worked examples today. If none match, a pattern has
    # drifted from the prose and everything above is checking nothing.
    if examples == 0:
        problems.append(
            'no worked example matched any pattern in this script, so none was '
            'checked; the prose moved and the patterns did not'
        )

    if problems:
        print('the documentation states a bargain the code does not:')
        for problem in sorted(set(problems)):
            print('  %s' % problem)
        print()
        print('`Split::DEFAULT` in crates/itsanas-coord/src/accounting.rs is the')
        print('authority. Nine files disagreed with it once, and the first one a')
        print('new member reads was among them.')
        return 1

    print(
        'bargain: the code ships a %s split, all %d documents that state it '
        'agree, and %d worked examples recompute' % (split, len(STATES_THE_BARGAIN), examples)
    )
    return 0


if __name__ == '__main__':
    sys.exit(main())
