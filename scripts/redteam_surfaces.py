# -*- coding: utf-8 -*-
"""Red-team tests by attack surface (HANDOVER §8 6b).

Every `red_team_*` test belongs to exactly one surface, by the file it lives
in. `check-counts.py` imports this and fails when a file with red-team tests
maps to no surface, or when the per-surface counts in `docs/TESTING.md`
disagree with the source.

Run one surface's red-team tests:

    python3 scripts/redteam_surfaces.py run identity

or print the nextest filter only (`filter identity`), or the table (`table`).

Why files and not a move into `tests/redteam_<surface>.rs`, which is what 6b
first asked for: most red-team tests reach private items of the module they
test, so moving them means widening APIs for tests, and CI is already
selective by crate. A map costs one file and no API; a mixed file goes to the
surface most of its tests attack, and the table says so.
"""

import os
import re
import subprocess
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')

# Order is the order of the table in docs/TESTING.md. A path ending in "/"
# covers every file under it.
SURFACES = [
    ('wire', 'peer and coordinator framing, versions, verb numbering', [
        'itsanas-net/src/protocol.rs',
        'itsanas-coord/src/protocol.rs',
    ]),
    ('tls', 'handshakes, session limits', [
        'itsanas-tls/src/limits.rs',
        'itsanas-tls/src/session.rs',
    ]),
    ('identity', 'keys, accounts, devices, claims, invitations', [
        'itsanas-crypto/src/identity.rs',
        'itsanas-crypto/src/keystore.rs',
        'itsanas-coord/src/claim.rs',
        'itsanas-coord/src/invitation.rs',
        'itsanas-node/src/contact.rs',
        'itsanas-cli/src/copy_to_key/',
        'itsanas-cli/src/drives.rs',
        'itsanas-android/src/devices.rs',
        'itsanas-node/tests/five_devices.rs',
        'itsanas-node/tests/withdrawals.rs',
    ]),
    ('coordinator', 'directory, presences, accounting, the coordinator as an adversary', [
        'itsanas-coord/src/directory.rs',
        'itsanas-coord/src/service.rs',
        'itsanas-coord/src/server.rs',
        'itsanas-coord/src/accounting.rs',
        'itsanas-coord/tests/coordinator.rs',
        'itsanas-coordinator/tests/crash.rs',
        'itsanas-node/src/coordinator.rs',
        'itsanas-node/tests/away_from_home.rs',
    ]),
    ('host-storage', 'what a host stores for whom: pledges, owner caps, eviction', [
        'itsanas-node/src/owners.rs',
        'itsanas-node/src/pledges.rs',
        'itsanas-net/src/service.rs',
        'itsanas-store/src/vault.rs',
        'itsanas-net/tests/eviction.rs',
    ]),
    ('placement-audit', 'where copies go, audits, reliability, release', [
        'itsanas-placement/src/',
        'itsanas-net/src/placed.rs',
        'itsanas-net/src/session.rs',
        'itsanas-net/tests/placement.rs',
        'itsanas-store/src/index/placement.rs',
        'itsanas-store/src/reliability.rs',
        'itsanas-cli/src/placing.rs',
    ]),
    ('reach', 'LAN discovery, UPnP and NAT-PMP, relays, address classes', [
        'itsanas-discover/src/',
        'itsanas-cli/src/discovery.rs',
        'itsanas-cli/src/natpmp.rs',
        'itsanas-cli/src/upnp.rs',
        'itsanas-tls/src/reach.rs',
        'itsanas-net/src/relay.rs',
        'itsanas-net/tests/relay.rs',
    ]),
    ('peers-sync', 'two nodes over sockets, logs and convergence', [
        'itsanas-net/tests/two_nodes.rs',
        'itsanas-sync/tests/convergence.rs',
        'itsanas-store/src/oplog.rs',
    ]),
    ('local-store', 'the index, packs, folders, the virtual drive, crash', [
        'itsanas-store/src/index.rs',
        'itsanas-store/src/pack.rs',
        'itsanas-store/tests/store.rs',
        'itsanas-store/tests/packs.rs',
        'itsanas-store/tests/keyed_chunking.rs',
        'itsanas-folder/tests/',
        'itsanas-drive/src/projfs.rs',
        'itsanas-node/src/node.rs',
        'itsanas-node/src/config.rs',
        'itsanas-node/tests/says_what_is_wrong.rs',
    ]),
    ('local-control', 'setup, secrets, the control socket, the daemon, updates', [
        'itsanas-cli/src/main.rs',
        'itsanas-cli/src/control.rs',
        'itsanas-cli/src/jobs.rs',
        'itsanas-cli/src/daemon.rs',
        'itsanas-cli/src/lifecycle.rs',
        'itsanas-cli/src/portable.rs',
        'itsanas-cli/src/setup/',
        'itsanas-cli/src/update/',
        'itsanas-cli/tests/steering.rs',
        'itsanas-release/tests/release.rs',
        'itsanas-android/src/lib.rs',
    ]),
]

RED_TEAM = re.compile(r'#\[(?:tokio::)?test\][^\n]*\n(?:\s*#\[[^\]]*\][^\n]*\n)*\s*(?:async\s+)?fn\s+(red_team_[a-z_0-9]+)')


def red_team_by_file():
    """{path under crates/: [test names]} for every file with red-team tests."""
    found = {}
    crates = os.path.join(ROOT, 'crates')
    for directory, _, files in os.walk(crates):
        for name in files:
            if not name.endswith('.rs'):
                continue
            path = os.path.join(directory, name)
            with open(path, encoding='utf-8') as source:
                names = RED_TEAM.findall(source.read())
            if names:
                found[os.path.relpath(path, crates).replace(os.sep, '/')] = names
    return found


def surface_of(path):
    """The surface a file belongs to, or None. Exactly one matches, or the
    table is ambiguous and `problems` says so."""
    matches = [key for key, _, paths in SURFACES
               if any(path == p or (p.endswith('/') and path.startswith(p)) for p in paths)]
    return matches[0] if len(matches) == 1 else None


def problems():
    """What is wrong with the map, as sentences; empty when it is right."""
    out = []
    for path in sorted(red_team_by_file()):
        matches = [key for key, _, paths in SURFACES
                   if any(path == p or (p.endswith('/') and path.startswith(p)) for p in paths)]
        if not matches:
            out.append(f'{path} has red-team tests and no attack surface in '
                       f'scripts/redteam_surfaces.py: add it to one')
        elif len(matches) > 1:
            out.append(f'{path} maps to {len(matches)} surfaces ({", ".join(matches)}): one only')
    return out


def counts():
    """{surface: number of red-team tests}."""
    totals = {key: 0 for key, _, _ in SURFACES}
    for path, names in red_team_by_file().items():
        key = surface_of(path)
        if key:
            totals[key] += len(names)
    return totals


def filter_for(key):
    """A nextest filterset for one surface's red-team tests."""
    parts = []
    for path, _ in sorted(red_team_by_file().items()):
        if surface_of(path) != key:
            continue
        crate, rest = path.split('/', 1)
        if rest.startswith('tests/'):
            parts.append(f'binary_id({crate}::{rest[6:-3]})')
            continue
        module = rest[4:-3]
        if module.endswith('/mod'):
            module = module[:-4]
        if module in ('lib', 'main'):
            prefix = 'tests::'
        else:
            prefix = module.replace('/', '::') + '::'
        parts.append(f'(package({crate}) & test(/^{re.escape(prefix)}/))')
    if not parts:
        raise SystemExit(f'no red-team tests on surface {key!r}')
    return '(' + ' | '.join(parts) + ') & test(/red_team_/)'


def main(argv):
    keys = [key for key, _, _ in SURFACES]
    if len(argv) >= 2 and argv[0] in ('run', 'filter') and argv[1] in keys:
        expression = filter_for(argv[1])
        if argv[0] == 'filter':
            print(expression)
            return 0
        return subprocess.call(['cargo', 'nextest', 'run', '--profile', 'ci',
                                '--workspace', '--all-features', '--run-ignored', 'all', '-E', expression], cwd=ROOT)
    if argv[:1] == ['table']:
        for key, what, _ in SURFACES:
            print(f'| `{key}` | {what} | {counts()[key]} |')
        return 0
    print('usage: redteam_surfaces.py run|filter <surface> | table\nsurfaces: ' + ', '.join(keys))
    return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
