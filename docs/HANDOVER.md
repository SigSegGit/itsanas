# Handover

Everything needed to pick this project up cold. Read this, then
[ROADMAP.md](ROADMAP.md) for status and [ECONOMICS.md](ECONOMICS.md) for the
contract.

---

## 0. Resume here after `/clear`

<!-- ITSANAS-STATE
NEXT: 8.0c
TITLE: Nicolas's hand test, with a partner's Mac, on 0.1.20 (§8 0c)
WRITTEN-AT: 2026-11-27
BASE: 3e9c629
-->

Read this section, then §8. Nothing else is needed to continue. The block
above names the next step and `scripts/check-handover.py` keeps it honest;
whether CI is green and whether a PR is open are facts for `git` and `gh`,
never for this file.

**State now (2026-11-27).** The dated entries that sat here (2026-10-04 to 2026-11-26), the
done §8 items and steps and the resolved §9 and §10 entries are in
[HANDOVER-ARCHIVE.md](HANDOVER-ARCHIVE.md), "Moved from HANDOVER.md on 2026-11-27", verbatim:
history, not instructions.

- **Last done (2026-11-27):** the clean base (#470), then one PR with the 37 fixes of the
  whole-repository red team (each with a test that fails without it, catalogued) and every
  source file under 1,000 lines (`scripts/check-file-size.py`, its ceiling list now empty: the
  old 37 files are modules of one concern each, pure moves, 1,780 tests passing before and
  after). Dead code removed: one protocol constant below the accepted window, three unused
  dependencies. Before: the iPhone's rename and versions (§8 15, #468).
- **Last passes:** a whole-repository red team (eight lenses, each finding checked by a
  skeptic: 43 confirmed) and a Rodin consultation, 2026-11-27; their fixes are the PRs after #468.
  The `itsanas` skill (`.claude/skills/itsanas/SKILL.md` §3) says when the next ones run.
- **In flight:** NEXT, Nicolas's hand test with a partner's Mac (8.0c). `scripts/kit-day.sh`,
  started on every machine of the day at once (`--away` on the one away from home), runs B, O and
  F between all of them through the account itself and ends in `PASS kit-day`; the partner's page
  (TEST-PARTENAIRE.md) leads with it. The whole run, every command in order with its expected
  result, is in the owner's private notes (`RECETTE-0.1.20.md`).
- **Next:** what §8 still holds needs a person or hardware: the Mac day (8.0c), the phone on adb
  (`android-e2e.sh` on it waits for it to be plugged in), Windows Hello's own prompt (§8 10d),
  Android's scheduler over hours (§8 10c), two real networks (§8 10a). §8 00 (ii) waits for an
  application account on `measured`. One open question is Nicolas's: people behind one address
  (carrier NAT, common in China) share the name-question budget; a proof of work past it would let
  them through but would let a GPU list names cheaply (ROADMAP "Names a stranger can list").
- **Traps that still bite:**
  - **Read the fleet before anything** (`gh run list --workflow fleet.yml --limit 3`, and issue
    "fleet: red"): it was red seven days with an issue nobody read.
  - The fleet agents rebuild on a release tag only: a fix reaches the VM's coordinator and the
    containers with a release, or by a rebuild by hand from `main`.
  - A light round obeys the `MIN_INTERVAL` floor (30 s after the last round), so a file goes out 2
    to 32 s after it is written, never sooner.
  - `FileEditsTest` sorts before `FreeTest`, so it is the scenario that logs in from an empty phone.
  - `acceptance-local.sh` cannot run on Windows (a debug `itsanas init` overflows the main
    thread's 1 MB stack); run it in WSL, where the three LAN-discovery checks fail on WSL's
    network and nothing else does. Git Bash rewrites `/mnt/c/...` given to `wsl`; use
    `MSYS_NO_PATHCONV=1 wsl -e bash -l script`, `-l` for cargo on the PATH.
  - Git Bash heredocs ate `\n` in Python edit scripts twice: write them with the Write tool.
  - `scripts/check-counts.py` sums only the top `#` headings of TESTING.md, not the `##` under
    them; a new red-team file needs its surface in `scripts/redteam_surfaces.py`.
  - #435 changed the presence bytes without raising `COORD_VERSION`; message bytes are pinned now
    (`red_team_a_message_that_changes_its_bytes_changes_the_version`), disk bytes too (§8 17).
  - Merging one of a batch of PRs puts the next in conflict on the counts: keep main's side, run
    `check-counts.py` and `check-wiki.py --write`.
  - Flaky: `the_space_question_is_answered_by_the_one_rule_every_platform_shares` (real free space).
  - Worktrees building at once into the one target dir overwrite each other's crate artefacts
    (same hash, different source): a false "no method" or a stale test binary. `touch` the edited
    crate's `lib.rs` before trusting a result.
  - A worktree made by the agent tooling starts from `main`, not from the current branch:
    `git checkout -B <branch> <base>` first, or the work is cut from the wrong commit.

**Rule (Nicolas, 2026-10-27 and again 2026-10-30, "can't you change this
permanently?"): every step that touches a machine, the fleet, a release or
a secret is written with its exact commands, one fenced block each, hosts,
ports and paths filled in -- in every reply, every plan and every status
line, also when the session runs them itself.** "Rebuild the VM" without
its block is the bug he keeps having to point out. A session may and should
run them itself (SSH, the fleet, merges, Dependabot): only the release
signature needs him.

**Where the truth is:** `docs/ROADMAP.md` "Known ceilings" and "What an
adversarial sweep found" (open findings, with arithmetic); `docs/ECONOMICS.md`
§1 (the 30/70 bargain: each host enforces it since §8 1c, limits named there); `docs/DESIGN.md` §6.5–6.7
(verification cost against the 100 MB/day budget).

**Waiting on Nicolas, in this order, as commands to paste** (2026-10-29; a
session cannot do any of them, and gives him these commands first, every
time -- he does not want to remember them). The Pi and the VM run their
accounts in **Docker** (`scripts/fleet-docker.sh`): their version is the
image's, rebuilt from `~/itsanas`, and `itsanas update` does not apply there
(`install/docker/README.md`). That is why V says 0.1.12; an earlier version
of this list said "release, then `itsanas update`", which was wrong. Access:
`~/.ssh/itsanas_session`, user `itsomeone`, `ngas.fr` port 22010 (Pi) and
22011 (VM), from his private notes, checked 2026-09-17.

0a. **Release 0.1.20, everywhere, then the hand test with a partner's Mac** (§8 0c). 0.1.20
   carries what 0.1.19 lacks: the trash on every machine and its rules (§8 13), files freed
   from a machine (§8 11), Start at login per account (§8 12), one system unlock for every
   account at sign-in (§8 10d), sharing both ways on a phone, several files at once on
   Android (§8 14), and a phone that hosts said hosting (#416). The whole run, every command
   in order with its expected result and the report to send back, is in the owner's private
   notes (`RECETTE-0.1.20.md`). It starts here: the release (it waits for the version PR's
   merge), the laptop, then the fleet run, which brings the Pi and the VM to the release
   (run `scripts/fleet/setup.sh` on each first if the agent predates the trash):

       cd D:\GitHub\itsanas; git switch main; git pull; .\scripts\release.cmd

       itsanas update

       gh workflow run fleet.yml --repo SigSegGit/itsanas; gh run watch --repo SigSegGit/itsanas $(gh run list --repo SigSegGit/itsanas --workflow fleet.yml --limit 1 --json databaseId -q '.[0].databaseId')

   The iPhone's steps are `docs/IOS.md`, "The hand test", 0 to 10; its build:

       gh run download --repo SigSegGit/itsanas --name ITSaNAS-unsigned-ipa --dir ios-ipa $(gh run list --repo SigSegGit/itsanas --branch main --workflow CI --status success --limit 1 --json databaseId -q '.[0].databaseId')

3. Publish the wiki (once after the wiki-and-board PR merges, then after any
   change to ROADMAP.md or MVP.md). If it says it cannot clone, open the
   Wiki tab, press "Create the first page", save anything, run it again:

       git switch main; git pull; python scripts/wiki-publish.py

4. Create / refresh the public Projects board (needs the `project` scope once):

       gh auth refresh -s project
       python scripts/project-sync.py

   Then, in the browser, add a Board view grouped by Status and a Roadmap
   view: `gh` cannot create views.

---

## 1. Where things are

```
D:\GitHub\itsanas
remote: https://github.com/SigSegGit/itsanas   (public, AGPL-3.0-or-later)
branch: main
CI:     .github/workflows/ci.yml — Linux, Windows, macOS, ARM, Android core,
        cargo deny, installers run on each OS
```

The fleet: this Windows laptop (a scheduled task named `ITSaNAS` runs the
daemon; its passphrase file is under `%LOCALAPPDATA%\itsanas`), a Raspberry Pi
4B, and an aarch64 VM on a Freebox Delta. **The coordinator belongs on the VM**
(Nicolas, 2026-09-17; `docs/MVP.md` §2 row 4). The Pi and the VM run their
accounts in Docker (`scripts/fleet-docker.sh`).

**Reaching the fleet from an agent session:** `ssh -i ~/.ssh/itsanas_session -p
22010 itsomeone@ngas.fr` is the Pi and `-p 22011` the VM (the LAN addresses
refuse SSH from the laptop; the public name works from inside, so the Freebox
does hairpin NAT for SSH). Since 2026-09-17 both machines carry
`install/sudoers-itsanas` in `/etc/sudoers.d/itsanas`: the coordinator can be
stopped, upgraded, started and read **without a password**, in exactly the
forms that file lists (`install/README.md`, "Operating the coordinator without a
password"). Nothing else runs as root from an agent: re-running
`coordinator.sh`, dropping `--admit-first` and touching
`/var/lib/itsanas-coordinator` are Nicolas's. An agent never types a password,
even one given in chat.
Accounts and addresses are in `install/README.md` and `docs/MVP.md`. **No secret belongs in
this repository**, including test machines' passphrases.

## 2. The invariant that keeps this honest

`docs/ROADMAP.md`, `docs/TESTING.md` and `docs/ECONOMICS.md` are updated **in
the same commit as the code they describe**. If a document disagrees with the
code, the code is right and the document is a bug. Keep doing this.

**Tense discipline, which is the rule that stops the drift.** Present indicative
means *it runs today and a named test proves it*. Anything else carries a
visible marker — ECONOMICS.md has a legend at the top and every section is
tagged ✅ / 🟨 / ⬜. This exists because the failure mode is not lying, it is
elegance: a mechanism reads better in the present tense, so a document written
to decide what to build slides into describing it as built. That happened here —
`itsanas evict`, the anchor placement rule and challenge-based reputation were
all described as working before any of them existed, and one of them then
propagated into three other documents.

The practical test before committing a document: **for every present-tense
sentence, can you name the test or the function?** If not, mark it or cut it.

Documents organised by *state* (ROADMAP) do not drift, because the form has an
obvious place to write "not done". Documents organised by *mechanism*
(ARCHITECTURE, DESIGN, ECONOMICS) drift, because they do not. That is a property
of the plan, not of anybody's attentiveness — which is why the markers are
mandatory rather than encouraged.

`scripts/check-catalogue.sh` fails if TESTING.md names a test that does not
exist, which has happened. It does not check the reverse — some crates are
catalogued by property rather than test by test — so a new test still has to be
written up by hand.

Test counts in TESTING.md are mechanical, and the tool is the gate itself:

```bash
python scripts/check-counts.py
```

It counts `#[test]` and `#[tokio::test]` functions under `crates/` and checks
every figure README, ROADMAP and TESTING state against them; today's figures
are in the header of `docs/TESTING.md`. It counts the source rather than
`cargo test -- --list`
because that command's answer depends on the machine running it: one test is
`#[cfg(unix)]` and the ProjFS ones are `#[cfg(windows)]`, so the same tree lists
a different number on Linux and on Windows, and a count that moves with the
reader is not a count.

## 3. Verify a clean tree in one go

Every gate CI runs, runnable locally:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo nextest run --profile ci --workspace --all-features   # 1 min per test, enforced
# CI runs only the affected crates on a pull request: see what it chose with
#   EVENT=pull_request python scripts/ci_scope.py plan
cargo test --doc --workspace --all-features                 # nextest skips doctests
cargo nextest run --release --workspace --all-features --run-ignored ignored-only
cargo +1.88.0 check --workspace --all-features          # MSRV
cargo deny --all-features check
bash scripts/check-rust.sh                              # fmt, clippy and rustdoc
python scripts/check-test-budget.py                     # the timeout is still enforced
bash scripts/check-catalogue.sh                         # docs/TESTING.md names real tests
```

All of these pass as of the last commit. **MSRV is 1.88** (let-chains), not the
1.85 that edition 2024 alone would need.

## 4. Crate layout and what each is for

```
crypto     identity, key schedule, sealing, blinded addressing, keystore
testkit    Alice/Bob/Carol — published test users, generated corpus, canaries
wire       length-prefixed framing + a generic Connection<S: Read + Write>
discover   signed UDP announcements on the local network; no server involved
policy     when and how much to sync — decided, tested, and used by the daemon
           yet: its consumer is the Android shell. `--metadata-only` reaches
           the mode by hand.
tls        anonymous TLS + device authentication bound to the channel
store      chunking, blob store, index, operation log, vault, version vectors
sync       version-vector merge, conflict resolution, convergence simulation
net        peer protocol, TLS transport, push/pull sessions
placement  rendezvous hashing (integer, no floats), repair planning
coord      device certificates, accounting, directory, protocol, server, client
coordinator  the `itsanas-coordinator` binary: address book and escrow locker
folder     a real directory mirrored into the store and back, with a watcher
cli        `itsanas`: the commands, the daemon and the setup page as a library (`src/lib.rs`), with a thin binary (`src/main.rs`)
```

Dependency direction is strict: `crypto → store → sync → net → cli`, with
`wire`/`tls` beside them and `coord` deliberately unable to reach `store` or
`sync`.

## 4b. Four rules against the way this goes wrong

Added 2026-09-21, after Nicolas said the sessions contain too many detours and
false discoveries. He is right, and the four below each come from a specific
failure in one day's work rather than from a principle. Each names its own
counter-example, because a rule without one is advice.

**A claim about code you have not read is a hypothesis, and it does not get
written as a fact.** Before writing that a mechanism exists, is called, or
imposes a constraint: look, and cite what establishes it. *The failure*: a whole
step was held up for three exchanges on the claim that the per-round announce
feeds entitlement, so a decision was needed from Nicolas. Nothing feeds
entitlement -- `tick`, `contributions` and `assess` are called only by their own
tests, and ECONOMICS.md had already said so in writing. One `grep` at the start
would have cost a minute. The same failure, smaller, twice more the same day: a
comment asserting that `socket2` sets `SO_EXCLUSIVEADDRUSE` by default (it does
not, and the listener was hijackable until a test said so), and `tick`'s own
doc comment claiming it is "called on a timer" when no timer has ever called it.

**Measure before you qualify.** A number nobody took is not an argument, in
either direction. *The failure*: the full-table scan in `peers_of` was announced
as "the wall at three thousand machines" before anything was timed. Timed, it
was 2.6 % of one core -- real, worth fixing, not a wall. Dramatising an unmeasured
cost is the same error as dismissing one, and it wastes the same attention.

**What can be done now does not go into the next pass.** *The failure*:
`itsanas doctor` opened the store, the daemon held the store, so the command a
person runs *because nothing is syncing* refused to run on a machine that is
syncing. It was written down as something to fix later. It took under an hour
when Nicolas pushed back. "Noted for the next session" is the right answer only
when the work is genuinely blocked on somebody else or genuinely belongs to
another step -- and saying which of the two it is, out loud, is part of the
answer.

**A restoration is a copy, never an edit.** Undoing a sabotage by replacing text
is a second edit, and a second edit can fail on its own. *The failure*: three
sabotages applied to `directory.rs` by text replacement; breaking the first made
another anchor ambiguous, the restore matched nothing, and the half-restored
file looked like a working one while three unrelated tests failed for a reason
that was not in the code. The afternoon had to be reapplied on a file taken back
from `main`. `scripts/sabotage.py` now does it from a byte-for-byte copy and
restores on any exit, including a crash.

Two of these are now mechanical rather than remembered, which is the only kind
that survives:

| | |
| --- | --- |
| `scripts/sabotage.py` | breaks one defence at a time, runs the tests, restores from a copy, and **reports a defence whose sabotage turned nothing red** as the finding it is |
| `scripts/merge-when-green.sh` | merges only when passing checks equal total checks and there are enough of them to be the real suite. **Since 2026-10-06 one exception, and only one: a skipped check is green when `No warnings anywhere in this run` passed**, because that job runs `ci_scope.py verify`, which fails on any skip the `changes` job did not decide and explain. A skip without that is not green, as before. (The `itsanas` skill's own text, outside this repository, still says "skipped is not green"; this rule supersedes it.) It exists because `gh pr checks \| grep -civ pass && gh pr merge` merged a pull request with two checks still running: `grep -c` exits 0 when it *finds* something, so finding three failures ran the merge. A gate must say "all of them passed", never "I saw no failure" |

---

## 5. Reading the code: a route, not a tour

Nobody reads 22 000 lines. This is the shortest path to the point where the rest
of the code stops surprising you — roughly two hours, in this order. Every file
listed has a module doc comment stating what it is for and what it refuses to
do; read those first and the bodies second.

| # | File | Lines | Why this one |
| --- | --- | --- | --- |
| 1 | `crypto/src/kdf.rs` | 158 | The key schedule. Everything else derives from here, so it is the shortest file that changes how you read all the others. |
| 2 | `crypto/src/seal.rs` | 561 | Deterministic vs randomised sealing, and what goes into the associated data. The single most consequential design decision in the project — dedup, remote audit and blinded addressing all fall out of it. |
| 3 | `store/src/version.rs` | 310 | Version vectors and the dominance test. Every convergence property in the system is this file being right. |
| 4 | `folder/src/decision.rs` | 238 | `decide(on_disk, in_store, ledger)`, a pure function over three hashes with an exhaustive 27-case test. This is where "the folder syncs" actually happens, and it is small enough to hold in your head entirely. |
| 5 | `tls/src/auth.rs` | 182 | Why the device identity is not in the certificate. Short, and it is the whole transport security argument. |
| 6 | `store/src/oplog.rs` | 714 | Segments, chaining, and the tail-truncation gap documented at the top. Read the module comment even if you skip the body. |

After those six, the shape of everything else is predictable. `chunker.rs`,
`nodeset.rs` and `vault.rs` are each large but locally understandable, and
`accounting.rs` is pure integer arithmetic with [ECONOMICS.md](ECONOMICS.md) as
its commentary.

**What to read when you have to change something:** the tests. They are named as
sentences and [TESTING.md](TESTING.md) says what each one proves, so the fastest
way to learn what a module guarantees is its test module, not its body.

## 6. Decisions that must not be quietly reversed

Each of these has a test that fails if it is:

| Decision | Why | Guarded by |
| --- | --- | --- |
| A feature lives in the library, and a shell draws only what it alone can | A feature written twice is two features: the novice test of 2026-11-01 found the phone short of what the PC page does, and Nicolas chose one interface over three. What a shell lacks is `Features`, hidden in the page and refused by the routes | `scripts/check-parity.py` reads `docs/PARITY.md` against the commands, the page's headings and routes and `Features`; `red_team_every_feature_of_a_shell_is_named_in_the_page` and `red_team_a_route_behind_a_missing_feature_refuses_whatever_the_page_hides` |
| No floating point in placement or accounting | `f64::ln` is libm-dependent; two machines disagreeing in the last ulp about where a chunk lives is a silent, permanent split | `no_floating_point_is_involved` greps the module's own source |
| The FastCDC gear table is derived and pinned | If two devices disagree about boundaries, dedup silently stops network-wide | `the_gear_table_is_pinned_forever` |
| A delete is only acted on for a path the ledger says this device had | Otherwise a fresh device announces the deletion of everything its owner has | `a_brand_new_device_downloads_everything_and_deletes_nothing`, and an exhaustive 27-case matrix in `decision.rs` |
| Concurrent edits keep both, winner chosen by a deterministic total order | A rule two devices could disagree about makes them overwrite each other forever | `the_winner_is_the_same_whichever_side_asks` |
| A concurrent delete loses to an edit | An unexpected file costs a second; a lost edit is unrecoverable | `a_delete_racing_an_edit_never_destroys_the_edit` |
| The network never deletes data as a punishment | Total economic failure returns a member to a local backup, nothing worse | `only_default_permits_reclaiming_...` |
| Availability anyone else reports never reaches placement; what this device measured does. Decided by Nicolas, 2026-10-14 (it replaced "availability affects entitlement, never placement") | The decision that risks data must not depend on a number an untrusted party produced; what this node saw of a host over its own sessions, on its own clock, is not that. The coordinator's `AvailabilityRecord` is never read | ECONOMICS.md §3; `hours_this_node_did_not_try_are_not_counted_against_a_device`; `unknown_availability_counts_as_unreachable`; `no_floating_point_is_involved` with `the_float_guard_lists_every_module_of_the_crate`; and `red_team_no_coordinator_crate_can_reach_the_roster_builder` (2026-10-19), which fails if a coordinator crate becomes a dependency of the roster builder |
| Placement only adds copies | The one way placement could lose data is by taking a copy away, so there is exactly one removal path (next two rows) and a satisfied chunk asks for nothing | `a_satisfied_chunk_wants_nobody`; `repair_never_plans_a_deletion` |
| A `MayEvict` mark never deletes; it is honoured only for the same account, and an owner marks at most 4096 chunks a day | A mark is permission, used only when that account's own next store hits `ACCOUNT_FULL`; a stranger or another account must not be able to empty a host, and a clock set back must not buy a fresh day | `red_team_eviction_deletes_nothing_when_the_marks_cannot_cover_the_need`; `red_team_permission_to_evict_is_refused_from_another_account_a_stranger_and_with_no_bound`; `red_team_eviction_never_takes_another_owners_chunks`; `red_team_an_owner_marks_at_most_a_days_cap_even_with_its_clock_set_back`; `red_team_a_peer_speaking_protocol_9_is_never_asked_to_evict` |
| A release (the owner letting a host go) needs an audit passed within `LIVE_FOR`, a margin of one account and a quarter of the miss goal beyond the target, and a margin that already counts the account's own devices | Releasing on a stale record, or letting two own devices each release "the" spare copy, ends below the target. The owner stops counting the copy at once, conservatively, and releases a (chunk, host) once per `CONFIRMED_FOR` | `red_team_release_needs_an_audit_passed_within_live_for`; `release_picks_the_lowest_ranked_holder_and_only_with_margin`; `red_team_concurrent_own_devices_never_release_below_the_goal`; `red_team_release_needs_a_local_copy_and_margin_and_happens_once_per_confirmed_window` |
| `placement = everyone` is exactly today's behaviour, and the default | Turning placement on trusts inputs nobody had seen; measurement and `status` run in both so a member reads the figures first | `placement_is_everyone_unless_the_file_says_measured`; `red_team_everyone_mode_never_grows_the_short_queue`; `red_team_an_inbound_session_does_not_change_everyones_count`; the existing `two_nodes.rs` suite unchanged |
| The vault takes no keys in any constructor | "A host cannot read what it stores" is structural, not a matter of nobody having written the call | `vault.rs` and `vault/` have no key parameter anywhere |
| Symlinks are skipped, never followed | A link to `~/.ssh` inside the folder would upload a private key | `symlinks_are_skipped_rather_than_followed` |
| Completing a handshake earns a peer nothing | Device keys are free keypairs, so authenticating identifies a peer and vouches for nothing. Treating it as trust turns the anti-flood measure into the flood's best tool | `red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`, `red_team_a_peer_that_only_answered_the_phone_has_earned_nothing` |
| The user id is never broadcast; the beacon's tag is a fresh nonce and a hash keyed on the account's secret, over nonce and device, with no clock in it | A user id is a public key; announcing it every 30 seconds on a café network tells the room whose machine this is. A tag derived from it alone (version 1) let anyone group an account's machines and anyone holding the id recognise them; a tag rotated on a clock strands a Pi booted in 1970 | `red_team_the_user_id_never_appears_on_the_wire`; `red_team_two_beacons_of_one_account_carry_unlinkable_tags`; `red_team_a_stranger_holding_the_user_id_cannot_recognise_the_tag` |
| A version 1 beacon is still read, and never counted as ours | Refusing it makes an upgrade split the household on the LAN; trusting its unkeyed tag makes the old format a downgrade path to a forgeable one | `red_team_an_upgraded_listener_still_learns_a_not_yet_upgraded_sender`; `red_team_a_version_1_beacon_is_still_heard_and_never_counted_as_mine` |
| A replay of the vault happens only when a marker says work is outstanding | Unconditional replay turned the daemon's per-round cost from "the new segments" into "the whole chain, times the peers"; never replaying means deferred work is silently never retried | `a_round_that_deferred_nothing_does_not_replay_the_chain_next_time` |
| Claims are kept in both key orders, written in one transaction, and an older file is repaired on open | A lookup by account used to walk every claim in the directory, so one member's question cost O(devices in the whole network) and the coordinator's work grew with the square of the fleet -- 2.58 ms per lookup at 3000 devices, against ~8 µs now. Denormalised, and only defensible because a device can never change owner, so an index row is written once and never moves. Reading a pre-index file as "this account has no devices" would tell every member their machines were gone | `the_index_and_the_claims_never_disagree_whatever_is_done_to_them`; `a_directory_written_before_the_index_existed_is_repaired_on_open`; `red_team_one_accounts_range_cannot_reach_into_the_next_accounts_devices` |
| The holder ledger is kept in both key orders, written in one transaction | The two questions asked of it are range scans under opposite prefixes; one ordering makes the other a full table walk. Denormalised, and only defensible because every write and every removal touches both | `the_two_orderings_never_disagree_whatever_is_done_to_the_ledger` |
| Nothing walks a log chain without a bound | `segments_for` returns a `Vec`; an unlimited walk materialises a whole history in RAM. Found once already in `blobs().addresses()` | `catalogue::MAX_SEGMENTS_WALKED`, and `Catalogue::complete` says when a listing was truncated |
| An audit's questions are drawn at random, never ordered | Ordered selection is guessable, and one particular ordering — least recently confirmed first — degenerated into a *constant*, because a push round re-stamps a whole batch from one clock reading and the sort fell through to its tie-break. A host could keep the sixteen lowest chunk ids out of fourteen million and hold a spotless record | `red_team_the_same_question_is_not_asked_twice_every_round`; `red_team_a_host_that_keeps_only_what_it_expects_to_be_asked_is_caught` |
| A paused peer receives one chunk per round **and is audited on that chunk alone** | Its other records are the ones it is paused for, so drawing questions from them guarantees failure and makes the suspension a ban. The probe is written down when accepted, not inferred from a timestamp — "the newest record" is precisely the one an ordered audit never reaches | `a_paused_host_that_starts_answering_again_is_sent_data_again`; `a_probe_is_remembered_until_the_peer_answers_for_it` |
| Multi-chunk fixtures in every audit test | With one record on the ledger every selection rule picks the same thing, so a broken one looks correct. The way-back test used a 37-byte file and passed for two commits while the mechanism it named did not work | `a_file_of_many_chunks` in `tests/two_nodes.rs`, with a length assertion in each caller |
| A store written before a table existed is repaired on open, not read as empty | `chunks_to_challenge` reads the device-first ordering; on an older file it would return nothing, no audit would ever ask anything, and a node that has stopped checking its hosts looks exactly like one whose hosts are honest | `a_ledger_written_before_the_second_ordering_is_rebuilt_on_open` |
| A peer paused for failing audits still receives log segments | Segments are kilobytes and keep it able to relay for devices that have done nothing wrong; cutting it out of the log would punish them too | `a_paused_host_that_starts_answering_again_is_sent_data_again` |
| A failed storage challenge withdraws evidence and never destroys data | The rule in ECONOMICS.md §5 is that the network never deletes as a sanction; a host that fails an audit simply stops counting as a holder, and the chunk is re-sent | `red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder` |
| An audit never challenges on a chunk this device cannot verify | Verifying means re-deriving the sealed bytes locally; challenging without a local copy would withdraw an honest peer's record for a reason that is nothing to do with them | `an_audit_never_asks_about_a_chunk_it_could_not_check` |
| A listing shows files not downloaded, and never writes an index entry for one | An index entry means a readable file, which the conflict and delete logic both assume. Faking one is a bug nobody can locate later | `a_metadata_round_makes_the_file_listable_before_it_is_downloaded`; `catalogue.rs` derives, never records |
| A peer's own clock never decides ordering or expiry, anywhere | It is an attacker-controlled integer, and a Pi 4 with no RTC reports 1970. Made twice — in discovery, then again in the coordinator's peer list — and removed twice | `a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address`; `CoordService::peers_of` uses `Directory::last_seen` |
| The escrow rate limit lives on the server, not the connection | Reconnecting costs a handshake and would buy a fresh budget, which is no budget | `red_team_reconnecting_does_not_reset_the_escrow_attempt_budget` |
| The sync schedule comes from `itsanas-policy`, never from a constant in a shell | Three shells with three numbers drift, and the argument for each number then lives nowhere. The daemon asks the policy and prints its reason; `--interval` overrides, `--metered` says what the connection costs | `a_service_on_ethernet_does_not_inherit_a_phone_s_interval`; `itsanas daemon` prints `because` |
| An enum with a decision table behind it exposes `ALL`, and the totality test walks it | A list of variants written out at the call site is one somebody forgets. `Attention::Unattended` was added and `every_combination_produces_a_plan_with_a_reason` went on checking the two it already knew, passing | `every_combination_produces_a_plan_with_a_reason` walks `Network::ALL`, `Power::ALL`, `Attention::ALL` |
| A peer is asked for a lost chunk only if the ledger already records it as holding that chunk | A repair request is a **disclosure**: it says this node no longer has that chunk. Blinded ids leak nothing about content, but "which chunks exist only on hosts now" is exactly the list to delete to destroy somebody's data. The first version asked every peer it connected to, including strangers discovery had just dialled | `red_team_a_stranger_is_not_told_which_chunks_this_node_has_lost` |
| Every detector of local loss writes to the same queue, and repair drains it before it samples | `doctor` knows the whole answer in one pass; the sampling scan needs fifty-five days to reach a given chunk on a terabyte. A human running `doctor` because a file will not open is the fastest detector here, and its answer used to go to a terminal and nowhere else | `what_doctor_finds_is_what_repair_fixes_first` |
| Anything that walks a large table from a cursor starts at a random point and wraps | Starting at the top means the first N entries are the only ones ever reached, and everything behind a run of unactionable ones starves for ever. Three places do this now — the audit draw, the repair scan, the loss queue — and the loss queue was written without it | `the_loss_queue_is_read_from_a_moving_start_and_wraps`; `every_holding_is_reachable_by_some_cursor` |
| A scoped thread that outlives a body which can panic raises its stop flag on unwind | Otherwise `thread::scope` joins a thread waiting on a flag the panic skipped. In a test that is a sixty-second hang instead of an assertion; in the daemon it is a process that stays alive, serving, never syncing, and looking healthy to systemd | `a_failing_assertion_inside_a_server_scope_fails_rather_than_hangs`; `a_panic_anywhere_in_the_scope_raises_the_shutdown_flag`; `UnblockOnDrop` in `handshake.rs` |
| A reply too large to be a chunk is refused on its length, before decryption | The wire allows 8 MiB and the chunker emits at most 256 KiB, so a peer answering every repair request at the frame limit would have this node decrypt a quarter of a gigabyte per round for a result known from the length | `a_reply_too_large_to_be_a_chunk_is_refused_without_decrypting_it` |
| The coordinator can refuse a member but never admit one | It is a notice board, not an authority. An invitation is signed by an existing member and verified against an account the coordinator already holds, so the one thing it can do unilaterally is deny service — which is already in the threat model and is inherent to being the notice board | `red_team_the_coordinator_cannot_write_itself_an_invitation` |
| A list of names is slow to draw up and recovery says nothing of a name | A list of accounts is a list of targets: `GetEscrow` answers a decoy, an invite-only coordinator checks the code first, each address asks `NAME_QUESTIONS` an hour (/64, four times that per /48), none answered before `NAME_ANSWER_FLOOR`. `Lookup` stays open: setup needs it before a machine has an identity | `red_team_a_recovery_fetch_reads_the_same_whether_the_name_exists_or_not`; `red_team_many_ipv6_blocks_of_one_site_share_its_budget`; `red_team_without_a_valid_code_a_taken_a_retired_and_a_free_name_read_the_same`; `red_team_one_address_cannot_draw_up_a_list_of_names`; `red_team_no_answer_about_a_name_leaves_before_the_floor`; `red_team_a_wrong_name_and_a_wrong_passphrase_read_the_same` |
| Every refusal at the door reads identically | Distinguishing "no such code" from "expired" from "spent" lets anybody enumerate which codes exist, and the codes are what keeps strangers out | `every_refusal_reads_the_same_so_codes_cannot_be_enumerated` |
| **Nothing makes `has_chunk` true without proof** | The rule, in one line, because it existed as two methods with two different answers and the unverified one was on the ordinary path. A blob on disk under an address is how every other part of this system decides it need not go looking: write noise there and the repair scan stops searching, no other peer is asked, and the loss queue clears the entry `doctor` put in it. A recoverable loss made permanent, which is strictly worse than the peer refusing to answer | `red_team_a_relay_cannot_poison_a_chunk_on_the_ordinary_pull_path`; `red_team_a_host_cannot_answer_a_repair_request_with_rubbish`; one method, `Store::accept_chunk` |
| A chunk fetched to repair a local loss is verified before it is written | Unverified bytes make `has_chunk` true, the scan stops looking, no other peer is ever asked, and a **recoverable** loss becomes permanent. A host cannot read what it stores, so answering a repair request with noise is its one route to destroying data | `red_team_a_host_cannot_answer_a_repair_request_with_rubbish` |
| A test harness that runs a server stops it when the body panics | Otherwise `thread::scope` joins an accept loop nothing shut down and the suite reports a hang. Every red-team test in `two_nodes.rs` runs inside that harness, so a test catching an attack reported a timeout — which everybody retries and nobody reads | `a_failing_assertion_inside_a_server_scope_fails_rather_than_hangs`; `StopOnDrop` in `two_nodes.rs` and `coordinator.rs` |
| A replication target counts this device | Off by one means the repair loop keeps two copies while reporting three, invisibly, until two machines die instead of three | `a_target_counts_this_device_so_three_asks_for_two_elsewhere` |
| A peer is recorded as a holder only when it accepted or already had the chunk | Recording a refusal as storage is indistinguishable from safety until the local disk dies | `a_host_that_refuses_to_store_is_not_recorded_as_holding_anything` |
| A discovery beacon's address comes from the UDP source, never from the packet | A self-declared address lets any node redirect traffic to a machine that is not it | `a_new_device_is_recorded_with_the_address_it_was_heard_from` |
| The discovery table is bounded and confirmed peers are protected | Device ids are free keypairs, so a flood is cheap; without this it evicts the machines that matter | `a_flood_of_strangers_cannot_evict_a_known_peer`, `the_table_never_grows_past_its_capacity` |
| The sender's clock decides nothing in discovery | A Pi 4 has no RTC and boots in 1970; superseding by sender clock strands it at a stale address | `a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address` |
| The split is a value, and the one that grants entitlement is the coordinator's | It was `CONTRIBUTION_RATIO = 3`, and a constant cannot express 30/70 without becoming a fraction, which is where an `f64` wants to go. Two splits now exist and they are not the same thing: a node's configuration field decides only what that machine refuses its own owner, and the one `assess` is handed decides what the network grants. A `split` field on `DeviceContribution` would let a member widen their own entitlement by editing a text file. The node's field may only be stricter than `Split::DEFAULT`: `itsanas keep` on the owner side and the per-account check on every host (`owners/admission.rs`, refusal `ACCOUNT_FULL`) both read `Split::DEFAULT` | `red_team_entitlement_follows_the_coordinator_s_split_not_a_device_s`; `red_team_a_node_cannot_grant_itself_a_more_generous_split`; `red_team_a_split_with_a_zero_part_is_refused_rather_than_dividing_by_zero` |
| A withdrawal is final for its device id and wins whatever the signing clocks say | Every keystore holds the master secret, so a claim signed after a withdrawal proves nothing about who signed it; and a signer's clock is an opinion. Timestamp ordering let a stolen machine re-enrol and let a slow clock cancel a withdrawal. A reused machine logs in afresh and gets a new device id | `red_team_a_machine_holding_the_master_key_cannot_bring_a_withdrawn_device_back`; `red_team_a_withdrawal_signed_on_a_slow_clock_still_withdraws`; `a_later_enrolment_does_not_supersede_a_withdrawal` |
| A host stores for another account's device only once its coordinator has confirmed the device live, within `STANDING_FOR`; no answer means no storing. Decided by Nicolas on 2026-10-05 ("Refuse") | Every node holds its account key, so a withdrawn device signs itself a fresh live claim; the coordinator's withdrawal is the only word against it. Letting devices through while the coordinator is silent would make every outage, and every coordinator that hangs up on the question, a window for withdrawn devices. The cost, accepted: newcomers wait for an answer, other accounts stop storing on a host whose coordinator is down past `STANDING_FOR`, a host with no coordinator stores for its own account only. Consistent with the coordinator's place: it can refuse a member, never admit one | `red_team_no_word_from_the_coordinator_means_no_storing`; `red_team_a_host_whose_coordinator_does_not_answer_stores_for_no_other_account`; `red_team_a_withdrawn_device_that_re_signs_stores_nothing_on_a_host` |
| An account has at most `MAX_DEVICES_PER_ACCOUNT` live devices -- 20 since 2026-11-20, 10 from 2026-10-22 until then, and 5 from 2026-09-30 until 2026-10-22 | Enforced on the coordinator, the bound a rebuilt client cannot remove, and on the enrolling client so the refusal names the devices. A withdrawal frees its slot; re-signing a live device takes none; an account above the bound is never cut down, it only cannot add one. A running coordinator applies 20 only once it runs a build with 20; an older build can still refuse a new claim at its own lower bound. It does not update from a signed release (releases carry the member binary only): with `install/coordinator.sh --auto-update` its nightly timer fast-forwards its checkout to `origin/main` and rebuilds; without the timer, update it by hand (RELEASING "The coordinator: open, and updating itself"). The tests derive their counts from the constant | `red_team_one_device_too_many_is_refused_and_nothing_is_written`; `red_team_a_withdrawn_slot_lets_one_more_in_and_the_withdrawn_device_stays_out`; `red_team_re_signing_a_live_device_on_a_full_account_takes_no_slot`; `red_team_one_machine_too_many_refuses_to_enrol_itself_even_where_the_coordinator_would_not` |
| Coordinator messages are appended, never inserted | postcard numbers variants by position; the peer protocol already lost a week to it | `red_team_coordinator_messages_keep_their_wire_numbers` |
| Streaming boundaries match slice boundaries exactly | Otherwise one file stored via two paths dedups against nothing | `streaming_and_slicing_agree_on_every_boundary` |
| Published test identities are refused by `Store::open` | Their phrases are in the docs | `the_published_test_identities_are_refused_...` |

## 7. What is built and working

Detail and measurements are in ROADMAP.md; this is the map.

- **Core**: content-defined chunking, sealed blinded chunks, transactional index,
  signed chained log, version vectors and conflict siblings, GC with grace.
- **Network**: TLS 1.3 with device-key channel binding; peer protocol with a
  version window; storage challenges on a random sample; a per-(chunk, device)
  holder ledger with freshness; **releasing local data requires two holders that
  have answered a challenge**; 256-bucket set reconciliation so an idle round
  sends one hash; repair from peers with every byte verified.
- **Coordinator**: server and CLI, invite-only admission, one key one account,
  escrow of a sealed recovery container. Every host bounds what each other
  account stores to `Split::DEFAULT.room_earned` of its claimed pledge, or of
  what is proved once its audit contradicts the claim (`itsanas-node`
  `owners/admission.rs`, refusal `ACCOUNT_FULL`; §8 1c).
- **Machines that hold less than the account**: `itsanas keep` budgets with an
  ordering, selective fetch, `itsanas space` bounded by disk and pledge.
- **Front ends**: folder daemon with watcher; Android app (Compose over JNI);
  an iPhone app (`crates/itsanas-ios` and `ios/`); Windows virtual drive over
  ProjFS, read and write since 2026-11-03: a file saved, renamed or deleted there
  reaches every machine (`crates/itsanas-drive/src/writes.rs`; run by hand
  against real ProjFS, not in CI).
- **Installers** for Linux, Windows, macOS, Termux and the coordinator, each with
  `--clean` delegating to one uninstaller. Run for real on the Pi, the VM and
  Windows; macOS in CI.

## 8. What is next, in order

Open work only. A done item or step keeps its number and one line, so the references to it still
resolve; its full text, as it stood, is in [HANDOVER-ARCHIVE.md](HANDOVER-ARCHIVE.md), "Moved
from HANDOVER.md on 2026-11-27", and so is the full text of every open item condensed here.

00. **Application mode: Nicolas's top priority (2026-11-25).** For a machine that backs up a
    program's data (small encrypted files written about every five minutes, append-only), such as
    itsaresume's premium base, which goes through an account of its own (`itsaresume`). Goal: each
    new file on at least two other machines within minutes, and the program able to read that it
    is.
    (i) ✅ **The switch, the reciprocity, the light round and the readout** (#464): `itsanas app
      on|off`, peer protocol 13 `Request::AppMode`, privileged hosts dialled first, a light round
      every `APP_INTERVAL`, `newest M` on the snapshot's first line; see HANDOVER-ARCHIVE.md.
    (ii) **Measured placement prefers compatible hosts.** In `measured`, `Placement::build`
      (`itsanas-placement`, called from `placing::begin_round`) ranks hosts by measured uptime
      already; for an account whose machine is in application mode, rank the privileged hosts
      (the `AppPeers` set) first among those with enough room and an availability figure, and
      never a phone on metered data. `itsaresume` is in `everyone`, where the dial order of (i) is
      the whole preference, so this waits until an application account runs `measured`. Test to
      write: an app account with an app peer and a non-app peer of equal uptime places on the app
      peer first; sabotage by dropping the rank.
    (iii) ✅ **Check on the live client**, 2026-11-25 (#466): a new premium file was sent to three
      hosts 3 s after it was written, in a light round; see HANDOVER-ARCHIVE.md.
    (iv) For itsaresume (its own repository): read `newest` and `copies` from the snapshot and
      raise its alert when the newest changeset is not on two machines after a few minutes.

0. **Pass the MVP on the fleet, before any more enforcement.** Decided 2026-09-14, when Nicolas
   asked for a valid MVP. `docs/MVP.md` defines it as tests A–M and O passing unassisted on the
   fleet. What an agent cannot do: cut power, reboot the Pi, leave a laptop asleep for a day. What
   it can do is make each of those a command that checks and prints a verdict, so a test costs
   Nicolas minutes and no judgement.

   a2. ✅ C, K and M automated in `acceptance-local.sh` (2026-09-16).
   a. ✅ An acceptance kit: `scripts/acceptance.sh <test> <phase> [args]` (2026-09-14).
   b. ✅ Say why a peer refused what was pushed (2026-09-15).
   c. **Nicolas runs A–M and O with the kit.** Gated on (i) and (j) since
      2026-09-16. The next session pastes the receipts
      into MVP.md §6 and applies the verdict rule of §4 as written.
   d. **Whatever fails becomes the next item**, ahead of everything below.
   e. ✅ Small, intermittent and portable devices, never a self-inflicted block, never an abusable
      tolerance (asked 2026-11-10; steps 1 to 8 built, the last on 2026-11-24).
   x. 🟨 **The fleet tested by CI, so the hand test shrinks to what needs hands** (asked
      2026-10-23; [FLEET-CI.md](FLEET-CI.md)). Built: `scripts/fleet/agent.sh` behind a
      `restrict,command=` key on the Pi and the VM, `scripts/fleet/run.sh`, the runner as a member
      of `test` (`scripts/fleet/member.sh`), `.github/workflows/fleet.yml` (nightly, an issue
      `fleet: red` when red), `scripts/check-fleet.sh` on every push. A SKIP nobody planned turns
      red after seven nights; one that waits on a named action (`scripts/fleet/planned-skips`,
      the number of its item in "Waiting on Nicolas") does not (#339). C closed 2026-10-29 (run
      40941797343). **Left:** (3) the laptop (Windows): our side built (`acceptance.ps1`,
      `agent.ps1`, `setup.ps1`); Nicolas decides, then follows FLEET-CI.md "The laptop, once"
      (first, decide admin or a non-admin account for the agent); if it SKIPs, suspect
      Win32-OpenSSH not handing over `SSH_ORIGINAL_COMMAND` (never run there). (4) E/G, which need
      a node stopped (docker, so root: decide first).
   y. 🟨 **Test L: told, unasked, that the files are not safe yet.** Built 2026-10-23
      (`safety.rs`, `brief_status`'s `exposed`, the three trays and Settings), a nightly fleet row
      L (2026-10-27), the phone (2026-11-02), `failing` and `lowdisk` (2026-11-03). **Left:** the
      desktop notifications (Windows balloon, `notify-send`, macOS notification) seen by Nicolas
      at the next hand test.
   e. ✅ Measure H on the machine it is about (`scripts/acceptance.ps1 H`, 2026-09-15).
   i. 🟨 **Install and configure two accounts per machine, in one command each.** First pass done
      2026-09-16; the hermetic red-team test in `scripts/check-installers.sh` done 2026-09-30.
      **Left:** `provision.ps1 -Instance` / `provision.sh --instance` have not been run cold by a
      person -- they touch scheduled tasks, systemd units and running processes, and the laptop's
      daemon holds real data. Run it, keep a verbatim log of every place a person has to think,
      then fix those places. Wants a scratch machine or Nicolas at the keyboard.
      (`install/macos.sh` ran on a partner's Mac on 2026-10-05, §10 5.)
   j. ✅ The node's state without the passphrase (2026-09-16).
   k. **v0.2.0, the marker release.** Asked for by Nicolas on 2026-09-16: a version that says
      "concrete enough to test, and nowhere near v1.0.0". **With Nicolas, not an agent step
      (2026-09-30):** re-pinning FIRST-STEPS before the tag exists breaks the documented install,
      so the whole of (k) happens at the moment he cuts the tag: the workspace version bumped,
      `FIRST-STEPS.md` re-pinned, a changelog saying plainly what is testable and what is not. Do
      not tag from an agent session without saying so: a tag is the one thing here that other
      people's machines will pin to.
   l. ✅ A storage location that vanished never reads as a deletion (2026-09-18).
   m. ✅ Count the live copies, and let a machine leave politely (2026-09-21 to 2026-09-30).
   n. ✅ Refuse a file that will not fit, before copying it (2026-09-28). Remaining, 🟨: native
      quotas per platform, research and presentation only.
   o. ✅ Reach the network from outside the LAN, with machines that move (phases 1 and 2,
      2026-09-29). **Left:** that the `Dialling` adapter hands the relay and `ask_presences` on is
      still read, not run; phase 3 only if the fleet shows 1 and 2 fall short.
   p. 🟨 **Named instances only, each showing its account and storage.** Built 2026-09-30
      (#189-#192). **Left:** the service advice `migrate` prints is unexercised on systemd and the
      Task Scheduler (`ROADMAP.md`, "Named instances (0p)"), a job for 0i's human runs.
   q. ✅ First-user friction, from a persona run of `FIRST-STEPS.md` (closed 2026-09-30, #195-#197).
   r. 🟨 **A test bed in one command per machine, and a visible answer.** Built 2026-10-06
      (`install/testbed.sh`, `install/testbed.ps1`; three hermetic tests in
      `check-installers.sh`). **Left:** the run on real machines. **Done when** Nicolas has run it
      on Windows and the Mac, and the `bonjour` files have crossed both ways.
   w. 🟨 **A setup wizard and a tray, friendly enough for testers who are not us** (asked
      2026-10-06). Decided: the wizard is a local web page served by the `itsanas` binary itself.
      What each part still lacks:
      1. ✅ The control channel (`itsanas pause|resume|sync-now|interval`, `control.rs`).
      2. 🟨 The setup engine (`crates/itsanas-cli/src/setup/`). Not verified: the macOS/Linux
         windows and services never ran; verify against a real coordinator; the terminal fallback
         never driven by hand.
      3. 🟨 0t's parts 1-3, a release a tester downloads (`crates/itsanas-release`, `release.yml`,
         `sign-release.*`, `get.ps1` / `get.sh`); v0.1.0 published 2026-10-07.
      4. 🟨 The same engine behind a local web page (`setup/web/`). Not verified: macOS and Linux
         desktops; joining an existing account through the page; layout, contrast and
         keyboard-only use.
      5. 🟨 The tray, finished (`itsanas pause --for`, three trays held equal by
         `check-installers.sh`). Not verified: the macOS and Linux trays on a desktop; a clock
         jump during a timed pause.
      6. 🟨 0t's part 4, the self-update (`crates/itsanas-cli/src/update.rs`, `itsanas update`).
         Left: the macOS/Linux trays keep their plain dots.
      6b. 🟨 The setup after its first real install, its leftovers: none of the three words
         windows (Copy / Save) has been clicked by a person; UPnP (`upnp.rs`) and NAT-PMP never
         seen against a real router, still no PCP; `install/windows/itsanas.iss`'s uninstaller
         only signs out.
      7. 🟨 Clean removal: the drain (`leave --drain-for`) and the trays' "Remove this
         machine..." built 2026-10-12; never run from a real tray, and the window does not offer
         `clean -PurgeAccount` afterwards.
      8. ✅ Account deletion (`itsanas delete-account --confirm NAME`, 2026-10-12).
   t. **A release that updates itself, signed with Nicolas's key.** Split on 2026-10-06 into 0w (3)
      (parts 1-3, the release) and 0w (6) (part 4, the self-update); its text stays their
      specification, in HANDOVER-ARCHIVE.md. The signing key is Nicolas's, on his PC (§10 12).
   u. 🟨 **Reachable without port forwards: UPnP/NAT-PMP/PCP and IPv6.** Built 2026-10-13 to
      2026-10-30: the global IPv6 announced over a private IPv4 (`coordinator::prefer_global_ipv6`),
      NAT-PMP (`natpmp.rs`) after UPnP, several addresses per device
      (`itsanas_tls::reach::AddressList`, on since 0.1.7), Windows over IPv6
      (`reach::Listeners`), `sticky_ipv6`, and a global IPv6 address is not proof of a way in
      (`Witness`, `on_link`). **Left:** not yet seen on the laptop from another house (needs a
      release); on Windows any program of the same user can bind one specific address of the
      machine on the node's port and take that address's callers (a denial of service; only
      `SO_EXCLUSIVEADDRUSE` stops it, which needs `unsafe`, forbidden here); not measured: that a
      deprecated temporary address still accepts connections on Windows. PCP or hole punching only
      if the fleet shows the rest falls short.
   v. 🟨 **A relay for two machines that cannot reach each other** (0o's phase 3), data still
      sealed end to end. Built 2026-10-13 (`itsanas-net/src/relay.rs`, protocol 9;
      `park_while_unreachable`; a host of another account as a relay of last resort) and
      2026-10-31 (`itsanas get` reaches the account's machines, `account_targets`/`peer_targets`).
      **Not tested:** `peer add`'s refusal, and a `get` over real sockets through a relay. Open: a
      vanished parker is seen at the next keepalive (20 s). Coordinator relay: not built, only if
      measured.
   s. ✅ The vault on another disk (a NAS), the default unchanged (2026-10-13). Not done: moving the
      VM's vault (Nicolas, later); the index on local disk with blobs on the share; `clean`
      removing an external vault.
   f. 🟨 **A tray icon for the Windows daemon.** Built 2026-09-30 (`status --brief`,
      `scripts/itsanas-tray.ps1`, started at logon per node); Pause / resume, Sync now and Sync
      every since 2026-10-06 (0w (1)); disconnect and decommission built as 0w (5) and (7).
      **Left:** Nicolas sees the icon once and says what it gets wrong; "Remove this machine..."
      never seen on a real desktop.
   g. ✅ Accounts and devices, red-teamed and repaired (2026-09-15).
   h. ✅ Two accounts on one machine, then the full fleet protocol (2026-09-15).

1. ✅ **Enforce the space split** (asked for by Nicolas on 2026-09-14). All four steps built; hosts
   bound each owner since 2026-10-04 (#220-#224).
   a. ✅ The split as a value (2026-09-14).
   b. ✅ Bound writes on the honest client (disk and pull halves, 2026-09-30).
   c. ✅ Bound owners on the host — the part a rebuilt client cannot delete (first part
      2026-10-04; (i) and (ii) 2026-10-05; (iii) judged not worth its table, 2026-10-05).
   d. ✅ `ECONOMICS.md` §1 back to ✅ built (2026-10-05).

2. ✅ **Finish the red team, one surface per session, by hand.** Every step built.
   a. ✅ Integrity, by hand: what a hostile peer can make this node believe (2026-09-30).
   b. ✅ Confidentiality, by hand: what a host learns, alone or comparing notes (2026-09-30).
   d. ✅ At most `MAX_DEVICES_PER_ACCOUNT` live devices per account (2026-10-01; 20 since
      2026-11-20).
   c. ✅ A hostile host cannot stall a whole pull with one refused segment (2026-10-01).
   e. ✅ The Android app can withdraw a device (2026-10-01).

3. **The open findings** listed in ROADMAP.md, one per session. Steps (a) to (f) built.
   a. ✅ `pledge` and the Android setters keep the split (2026-09-30).
   b. ✅ A store the host will refuse costs no walk of its vault (2026-09-30).
   c. ✅ Chunk-size sequences stop fingerprinting files (keyed chunking, 2026-10-20).
   d. ✅ The LAN beacon stops grouping an account's machines (2026-09-30).
   e. ✅ The macOS installer puts `itsanas` on the PATH and waits for Apple's tools (2026-10-05).
   f. ✅ The account device roster (§10 item 7, option 2; 2026-10-19, #293).

4. ✅ **Verification at a terabyte.** The criterion: under 100 MB a day to verify under a terabyte
   (DESIGN.md §6.5).
   a. ✅ Within a differing bucket, ask only about chunks with no fresh record (2026-10-04).
   b. ✅ Stop re-offering a peer the chunks it refused (2026-10-04).
   c. ✅ The full walk itself (2026-10-04).

5. **A real phone**, and a release signing key for the APK that Nicolas holds
   (v0.1.0 ships with the development key).

6. **CI that costs what the change costs.** Asked for by Nicolas on 2026-10-06.
   a. ✅ Selective CI, by crate (2026-10-06, #228).
   b. ✅ Red-team tests by attack surface (2026-10-19, `scripts/redteam_surfaces.py`).
   c. **Not decided -- needs Nicolas.** The push to `main` re-runs the full
      suite on a tree a PR run usually already tested, which roughly doubles
      the cost of a merge. Skipping it when the merged tree equals the tested
      PR merge commit's tree would halve that, but Nicolas asked on
      2026-10-06 for `main` to run in full unconditionally, and the full run
      on `main` is half of what catches environment breaks (ROADMAP,
      "Selective CI"). Asked in §10 item 11; do not build before the answer.

7. ✅ **Availability-aware placement.** Built 2026-10-19. `placement = everyone | measured`,
   default `everyone`. Left: (a) 🟨 run `measured` on the fleet before it is ever the default, and
   read the `status` figures (`up N %`, `longest absence`, chunks short) for a week first: started
   2026-11-18, every fleet run sets it on the `test` accounts (row P) and prints the figures as
   notes; the week counts from the first run that sets it, then the default changes in
   `config.rs`. (b), (c), (d) ✅ 2026-10-19; the coordinator's swarm-wide marking stays ⬜, and
   must never feed placement (§6).

20. ⬜ **Paying for a better ratio, a market and not a bank** (Nicolas, 2026-11-24: an idea for a
    business, not to build yet). A member may pay others to provide their share of storage, and
    the project takes a small commission. Conditions, each to hold before anything is built: (a)
    no unwanted centralisation, proved; (b) a bidding system whose price follows offer and
    demand; (c) the network keeps far more capacity than data, the margin stated as a number with
    its arithmetic; (d) the Apple developer membership waits for this. First step when it is
    opened: a design with Rodin, the attacks first, and nothing public says it exists before it
    does (AGENTS.md).

19. 🟨 **A phone in China** (asked by Nicolas, 2026-11-23; a tester in China, on a Huawei). Done in
    0.1.22: the page and the native screens speak Simplified Chinese
    (`every_language_of_the_page_says_all_the_french_one_says`, `values-zh-rCN`); the app needs no
    Google service. Left, each to be decided on the tester's report: (a) ✅ the update check falls
    back to a mirror (`red_team_a_machine_that_cannot_reach_github_updates_from_the_mirror`); ⬜ the
    web server there and `ITSANAS_MIRROR` on the signing PC are Nicolas's; (b) HarmonyOS 5 runs no
    APK (§10 0); (c) Huawei's launch manager: the tester sets ITSaNAS to manual; (d) what the
    engine itself says stays English; (e) the 24 words stay the English BIP-39 list; (f) the
    latency from China is what the test is for; (g) ✅ the censored path simulated
    (`itsanas_testkit::path`, 2026-11-24). ⬜ An obfuscated transport, if the tester's report shows
    the fixed ports or the handshake blocked.

18. 🟨 **An occasional user is never cut off by an update** (Nicolas, 2026-11-23). Done
    2026-11-23: the coordinator protocol has the peer protocol's window (`MIN_COORD_VERSION`,
    `red_team_a_newer_client_is_answered_at_the_coordinators_version`), effective once the
    coordinator runs a build with it (redeploy it before a release raises `COORD_VERSION`); a
    machine at the oldest peer version kept syncs both ways; stored formats (§8 17) and the
    release manifest's form are pinned. **Left:** a fleet scenario where one machine stays on the
    oldest supported release while the others move, and an update path that works from it. Until
    the public announcement a version may break compatibility; gate for the announcement, not for
    the next release.

17. ✅ **Pin the bytes of what is written to disk, as the wire's are** (Rodin, 2026-11-23; done the
    same day: `red_team_what_the_store_writes_to_disk_keeps_its_bytes` and three more).

16. ⬜ **A small database's backup in a dedicated account, and companies as hosts** (asked by
    Nicolas, 2026-11-23). First the owner's own case: itsaresume's data kept by a dedicated
    account, `itsaresume`, whose node runs beside the app on the VM. (a) The app never hands
    ITSaNAS a file mid-write: ITSaNAS's half done 2026-11-24
    (`red_team_a_file_still_being_written_is_not_taken_half_read`); the app's half stays: it
    writes its snapshot aside and renames it into the folder (`VACUUM INTO` a temporary name, then
    a rename). (b) A point in time comes back on a machine that never held it (§8 15). (c) What it
    costs: estimated, not measured on the app's data; measure on itsaresume's real database before
    choosing the interval. **Decided by Nicolas, 2026-11-24:** such an account is promised nothing
    more than any member; retention is the account's own trash rules and trace-keeping (§8 13).

15. 🟨 **Rename, and earlier versions** (asked by Nicolas, 2026-11-19). `itsanas mv`, `itsanas
    versions`, the daemon's jobs, the phone's `rename`, `versions_of`, `bring_back`, the desktop
    page (`red_team_the_page_brings_a_version_back_and_renames`), a version fetched from another
    machine or a host (`red_team_a_version_never_held_here_comes_back_from_another_machine`), and
    since 2026-11-26 the iPhone (`FileEditsTest`). **Left:** a rename that crosses an edit it did
    not see leaves both names (the edit is kept, on purpose).

14. 🟨 **Sharing, both ways, as a storage app does** (asked by Nicolas, 2026-11-16: never a public
    link to thousands). Android shares out and takes in (`ShareTest`, by hand on the S25); the
    iPhone is an "Open in" and share-sheet target (`CFBundleDocumentTypes`). **Left:** the
    iPhone's share target is compiled by CI only, run by no test; the iPhone shares one file at a
    time.

13. 🟨 **What a delete does, machine by machine** (asked by Nicolas, 2026-11-12). Each machine has
    its rules, and the account a default that a new machine follows (`itsanas_store::deletion`):
    the account's trash, at once everywhere, or from this machine only; a file another machine
    deletes at once is protected here for the days. Settings' Trash section and `itsanas trash
    mode|receive|ask|confirm|follow`; the traces of deletes kept ten days to five years or for
    ever, a year by default (`itsanas trash traces`). Proved by `tests/deletion.rs`
    (itsanas-sync), `red_team_a_delete_follows_this_machines_rules_and_the_default_keeps_it` and
    the bench's phase T. **Left:** the iPhone's trash screen changes (identity, Empty) have only
    been compiled by CI.

12. ✅ **Start at login, per account, from Settings** (asked by Nicolas, 2026-11-10).

11. 🟨 **Files that need not stay here** (asked by Nicolas, 2026-11-10). `itsanas free PATH`,
    Settings' *Files on this machine*, the phones' Files screens (`FreeTest`), the bench's phase H
    (`red_team_a_freed_file_is_never_trashed_nor_deleted_and_comes_back`). **Left:** the release
    with real proved holders has run on no fleet.

10. **The next version, before the hand test (decided by Nicolas, 2026-11-03).** Rule: nothing tells
    a phone from another machine on the network; it runs the same loop, and only `itsanas-policy`
    decides when. In order:

    a. 🟨 **A phone hosts.** `host_until`, `host_run`, `hosting`, built; hosts between accounts
       (`Request::Hosts`) built. **Left:** not run across two real networks.
    b. ✅ One switch, "Contribute to the network", on by default.
    c. 🟨 **Android 15's six hours.** `onTimeout`, `onTaskRemoved`, not sticky; `SyncWorker`
       (`ServiceLimitTest`, `WorkerTest`). Not run: the worker scheduled by the system itself
       over hours.
    d. 🟨 **Windows Hello and Touch ID**, optional, off by default (`setup/unlock.rs`;
       `red_team_three_accounts_at_sign_in_ask_the_system_once`). The logic is tested with
       stand-in gates; neither system's own prompt has run.
    e. ✅ Legal texts: `docs/legal/`, embedded, on the first screen and in Settings.
    f. ✅ The Android logo.
    h. 🟨 **The fleet's F row with the trash.** Built 2026-11-10, green on the fleet with 0.1.19;
       row T (2026-11-18) runs the purge. **Left:** row T has not yet run on the fleet: its first
       nightly run after merge is the test; the containers' agent offers no T, the laptop is its
       only writer.
    g. Then 8.0c, Nicolas's hand test with the partner, and two modes to measure on it: plugged in on
       Wi-Fi, and on battery on Wi-Fi answering only.

9. **Three features Nicolas chose on 2026-11-03.**

   a. 🟨 **A phone folder that syncs by itself** (issue 95; `PhoneFolder.kt`, `phone_folder.rs`).
      **Left:** a real phone and its gallery.
   b. 🟨 **Writing into the virtual drive** (`crates/itsanas-drive/src/writes.rs`). **Left:** no
      real Explorer or Office session has driven it, only a script against real ProjFS on the
      laptop; it does not run in CI; Windows refuses to rename a projected folder (error 50), so
      that stays an error.
   c. 🟨 **Small residues.** **Left, on purpose:** the desktop `show_words` debt needs a native
      window per platform, which nothing here can test before a release.

8. ✅ **Removable disks** (2026-10-19, all four uses Nicolas listed). Open: a real disk pulled
   mid-write (8.0c, by hand).

## 9. Known gaps, deliberately open

The gaps fixed since are in [HANDOVER-ARCHIVE.md](HANDOVER-ARCHIVE.md), "Moved from HANDOVER.md
on 2026-11-27".

- **Left open by the 2026-11-27 red team**, each with its reason: the device proof is not bound
  to a role in its signed domain (a listener echoing the caller's own proof is refused by
  checking the device id instead; binding the role changes the wire and needs a version step);
  the virtual drive checks that a file is not a link, then copies it, so a file swapped for a
  link between the two is still followed; `Store::flush_segment` does not split a very large
  batch, it is refused above `MAX_SEGMENT_BYTES` instead; the Unix-only tests of owner-only
  modes (node home and keystore, recovery words) run in CI only.
- **A name Windows cannot hold** is refused on a Windows folder since 2026-11-23, with the file
  kept in the account. Left: the virtual drive (ProjFS) presents such names unchecked.
- **Tail truncation** is caught since 2026-11-18 (`Presence::head`). Not covered: a machine that
  never read a presence of the device (no coordinator and no sibling met); the length a presence
  carries is at most an hour old.
- **Usage is self-reported.** A member who under-reports gains entitlement they
  have not earned. Verifiable usage needs hosts to report what they hold.
- **Storage challenges prove possession at a moment**, not continuously, and a
  host that fetches from another replica just in time passes.
- **No bandwidth accounting.** 10 TB on a 1 Mbit uplink is worth far less than
  the number says. Deferred because measuring it badly punishes people for their
  ISP.
- **The self-update's scratch directory is beside the program**
  (`update.rs`, `SCRATCH`), so the final rename stays on one disk. With a
  `--prefix` the daemon's account cannot write (`/usr/local`), the install
  fails there; the daily check no longer does (2026-11-16: its manifests
  fall back to a private directory with a random name,
  `red_team_a_program_beside_which_nothing_can_be_written_still_hears_of_a_release`),
  and since 2026-11-17 `updates = auto` there tells the release as `notify`
  does instead of failing every day, and `itsanas update` says to run it as
  the account that installed it
  (`red_team_an_auto_update_where_the_program_cannot_be_replaced_is_told_not_failed`).
- **A store now knows which /24 each known host was last seen from** (§8
  7c, `device_network`). A prefix, never an address, for known devices only,
  removed when a device is forgotten; never on the wire or at the
  coordinator. Still, a copied store says which networks a member's hosts
  live on.
- 🟨 **A withdrawn device keeps its old claim.** Revocation reaches a node only
  when it sees the revoked claim (a session, a host, the coordinator). Since
  2026-11-17 the machine that withdraws keeps the revoked claim and hands it to
  its siblings with the others (`Node::withdrawn_here`, `Request::Claims`), and a
  host keeps it over the old live claim the withdrawn machine still presents
  (`red_team_a_withdrawal_reaches_a_machine_that_reads_no_coordinator`). Left: a
  machine of the account that meets none of its siblings after the withdrawal,
  and no coordinator, still admits it. A host of another account learns it from
  any machine of the account that introduces itself there (`Node::withdrawals`,
  `ClaimBook::withdrawn`,
  `red_team_a_host_of_another_account_learns_a_withdrawal_from_a_sibling`).
- **No file-level sharing between users.** Not needed for mutual storage;
  `UserKeys::agree` exists, is tested, and is deliberately unused until it is.

## 10. Open, waiting on Nicolas

Resolved items keep their number and one line; their full text is in
[HANDOVER-ARCHIVE.md](HANDOVER-ARCHIVE.md), "Moved from HANDOVER.md on 2026-11-27".

0. **Which HarmonyOS the tester in China runs** (Rodin, 2026-11-24). 4.x runs the APK; 5
   ("NEXT") runs none, and then nothing of §8 19 reaches them. Ask before more work for them.
1. **Who joins next.** The network has one person. Every economic and
   adversarial property above is untested against someone else's machine.
2. **An Android release key.** It must be generated and kept by Nicolas, never
   committed; the APK cannot be upgraded in place across a key change.
3. **How the bargain is enforced**: bilateral ledgers between hosts, or the
   coordinator computing standings. ECONOMICS.md argues for the first.
   **Decided 2026-10-05 (Nicolas):** bilateral, essentially, with occasional
   checks against the coordinator so the ledgers cannot drift unseen.
   Nothing built; what an "occasional check" compares, and how often, is the
   first design question of the session that builds ECONOMICS §3.
4. ✅ **Merging**: the agent merges once CI is green (2026-09-17); the rule now lives in AGENTS.md.
5. **Whether `install/macos.sh` works.** It has run on a CI runner and never on
   a Mac. The second person's machine is a Mac, so this is on the critical path
   of the pilot rather than a nicety.
   🟨 **Ran on a partner's Apple-silicon Mac, 2026-10-05**: build, install and
   smoke test pass (`--yes --no-service`, from the `main` tarball). Not seen:
   the fresh-Mac path (tools and Rust were most likely present, 29 s in all),
   the LaunchAgent, an Intel Mac. Found: the PATH (§8 3e).
6. ✅ **`main` is protected** (enabled 2026-09-16, `enforce_admins` on).
7. ✅ **A host can re-sign a genuine segment body under its own key**: option 2, the account
   device roster, decided 2026-10-05 and built 2026-10-19, both halves (§8 3f).
8. ✅ **May an agreeing bucket hash re-stamp holder records?** (§8 4c.) Decided 2026-10-04, yes,
   with the host-side check.
9. ✅ **Chunk sizes as a fingerprint (§8 3c).** Decided 2026-10-06 (Nicolas): keyed chunking.
10. **Make the skip verifier a required check** (selective CI, 2026-10-06).
   Branch protection requires lint, the three `Test (...)` legs and
   `acceptance-local`, and GitHub counts a skipped required check as passing.
   `merge-when-green.sh` refuses a skip unless `No warnings anywhere in this
   run` passed, but a merge by hand is held by nothing of the kind. Closed
   question: **add** that check to the required list (a hand merge then waits
   for the whole run, about the time the slowest job takes), or **leave** it
   and accept that only agent merges are guarded. Changing protection is
   Nicolas's to do; no agent touches it.

11. **Skip the push-to-`main` run when the merged tree was already tested**
   (§8 6c). Halves the cost of a merge; loses the second full run that catches
   environment breaks the same day rather than at the nightly. **Skip** or
   **keep**.

12. ✅ **Who signs a self-updating release?** (§8 0t.) Decided 2026-10-06 (Nicolas): his key, on
   his PC.

13. **Run the expensive tests on Windows and macOS too** (2026-10-06, from
   #243's review). `slow-tests` is `runs-on: ubuntu-latest`, so the
   `#[ignore]`d tests -- now including the three that run the daemon loop,
   whose main client is the Windows tray -- never run on Windows or macOS
   in CI. `ci.yml` is Nicolas's to change. The block, replacing the job's
   first two lines; the check is then named per OS, so a required
   "Expensive tests" in branch protection must be renamed with it:

   ```yaml
     slow-tests:
       name: Expensive tests (${{ matrix.os }})
       runs-on: ${{ matrix.os }}
       strategy:
         fail-fast: false
         matrix:
           os: [ubuntu-latest, windows-latest, macos-latest]
   ```

   Cost: the three release builds, about twice the job's minutes on macOS.
   **Add** or **keep Linux only**.
14. ✅ **Generate the release key** (§8 0w (3); done 2026-10-07, pinned in `RELEASE_KEY`).
15. ✅ **Clean removal: drain or refuse?** (§8 0w (7).) Decided 2026-10-07: (b), drain. Idea for a
   later version, **not decided**: as a last resort the coordinator takes the chunks no host
   accepted (sealed, so unreadable to it) until a new host is found; to be weighed (and Rodin'd)
   before anything is built.
16. ✅ **Two accounts on one network: one household** (answered 2026-10-19, built the same day,
   `households` in `itsanas-placement/src/target.rs`).

## 11. Working style Nicolas expects

- Blunt assessments. Say "you are wrong here" and then show why.
- Tests must state what they would catch. A test whose failure message does not
  name a consequence is a bad test.
- Comments explain *why*, never *what*.
- Keep docs synchronised in the same commit.
- Decide alone and proceed; flag the decision rather than asking permission for
  routine calls.
- Code for a Raspberry Pi and an unreliable network: bounded memory, no
  assumption that any machine is up.

An adversarial audit persona (`anthropic-skills:rodin`, French, blunt) has been
used on this project repeatedly and found real gaps every time. It is still
worth running after a substantial milestone -- **and it has a stopping rule,
because it did not and that cost months.**

### The stopping rule

**"The last pass found nothing" is not a condition for starting the fleet
tests, and must never be used as one.** An adversarial pass over a corpus this
size always finds something; the rate is a function of how hard somebody looks,
not of how good the software is. A share of each pass's findings is prose the
previous pass wrote, so the loop partly feeds itself.

Therefore:

1. **The exit criterion is `docs/MVP.md` A-M and O, and the verdict rule of
   §4.** Nothing else. Not "stable", not "no open findings". O joined it on
   2026-09-18 at Nicolas's instruction: four machines in one house cannot
   produce a decent testing run, and every other test can pass while the thing
   is a LAN product.
2. **While §8 item 0 is open, do not open a new audit pass on code that the
   fleet tests have never exercised.** Audit what a step changed, as §5 of the
   working loop requires, and stop there.
3. **A finding that is not a product defect is not a finding.** A stale
   sentence, a drifted unit, a message that could be clearer: fix it silently
   in the step that touches it and do not report it as a gap.
4. **Findings about code the tests have not reached go to `ROADMAP.md` "Known
   ceilings", not into the current step.** They are real; they are not now.

The cost of getting this wrong is not wasted effort, it is the appearance of
regression: every pass ends with a list, so the project looks like it is going
backwards while it is going forwards.
