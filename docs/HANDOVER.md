# Handover

Everything needed to pick this project up cold. Read this, then
[ROADMAP.md](ROADMAP.md) for status and [ECONOMICS.md](ECONOMICS.md) for the
contract.

---

## 1. Where things are

```
C:\Users\SigSeg\itsanas
branch: overnight-m2-to-m5   (unmerged; main is at the initial commit)
remote: none — nothing has ever been pushed
```

Publishing is Nicolas's decision and has not been made. Do not create a GitHub
repository without asking.

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

Test counts in TESTING.md are mechanical:

```bash
cargo test --workspace --all-features -- --list
```

That prints 464: **462 test functions across 17 binaries** (2 of them
`#[ignore]`d, which is the figure ROADMAP and TESTING both quote) **plus 2
doctests**. Quote the 462 and say what it excludes, or the number drifts.

## 3. Verify a clean tree in one go

Every gate CI runs, runnable locally:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo test --workspace --all-features
cargo test --workspace --all-features -- --ignored     # 3 slow tests
cargo +1.88.0 check --workspace --all-features          # MSRV
cargo deny --all-features check
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
policy     when and how much to sync — decided, tested, and called by nothing
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
cli        `itsanas` binary: commands + daemon
```

Dependency direction is strict: `crypto → store → sync → net → cli`, with
`wire`/`tls` beside them and `coord` deliberately unable to reach `store` or
`sync`.

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
| No floating point in placement or accounting | `f64::ln` is libm-dependent; two machines disagreeing in the last ulp about where a chunk lives is a silent, permanent split | `no_floating_point_is_involved` greps the module's own source |
| The FastCDC gear table is derived and pinned | If two devices disagree about boundaries, dedup silently stops network-wide | `the_gear_table_is_pinned_forever` |
| A delete is only acted on for a path the ledger says this device had | Otherwise a fresh device announces the deletion of everything its owner has | `a_brand_new_device_downloads_everything_and_deletes_nothing`, and an exhaustive 27-case matrix in `decision.rs` |
| Concurrent edits keep both, winner chosen by a deterministic total order | A rule two devices could disagree about makes them overwrite each other forever | `the_winner_is_the_same_whichever_side_asks` |
| A concurrent delete loses to an edit | An unexpected file costs a second; a lost edit is unrecoverable | `a_delete_racing_an_edit_never_destroys_the_edit` |
| The network never deletes data as a punishment | Total economic failure returns a member to a local backup, nothing worse | `only_default_permits_reclaiming_...` |
| Availability affects entitlement, never placement | The decision that risks data must not depend on the untrusted coordinator | ECONOMICS.md §3; placement takes no availability input |
| The vault takes no keys in any constructor | "A host cannot read what it stores" is structural, not a matter of nobody having written the call | `vault.rs` has no key parameter anywhere |
| Symlinks are skipped, never followed | A link to `~/.ssh` inside the folder would upload a private key | `symlinks_are_skipped_rather_than_followed` |
| Completing a handshake earns a peer nothing | Device keys are free keypairs, so authenticating identifies a peer and vouches for nothing. Treating it as trust turns the anti-flood measure into the flood's best tool | `red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`, `red_team_a_peer_that_only_answered_the_phone_has_earned_nothing` |
| The user id is never broadcast, only a keyed tag of it | A user id is a public key; announcing it every 30 seconds on a café network tells the room whose machine this is | `red_team_the_user_id_never_appears_on_the_wire` |
| A replay of the vault happens only when a marker says work is outstanding | Unconditional replay turned the daemon's per-round cost from "the new segments" into "the whole chain, times the peers"; never replaying means deferred work is silently never retried | `a_round_that_deferred_nothing_does_not_replay_the_chain_next_time` |
| Nothing walks a log chain without a bound | `segments_for` returns a `Vec`; an unlimited walk materialises a whole history in RAM. Found once already in `blobs().addresses()` | `catalogue::MAX_SEGMENTS_WALKED`, and `Catalogue::complete` says when a listing was truncated |
| A failed storage challenge withdraws evidence and never destroys data | The rule in ECONOMICS.md §5 is that the network never deletes as a sanction; a host that fails an audit simply stops counting as a holder, and the chunk is re-sent | `red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder` |
| An audit never challenges on a chunk this device cannot verify | Verifying means re-deriving the sealed bytes locally; challenging without a local copy would withdraw an honest peer's record for a reason that is nothing to do with them | `an_audit_never_asks_about_a_chunk_it_could_not_check` |
| A listing shows files not downloaded, and never writes an index entry for one | An index entry means a readable file, which the conflict and delete logic both assume. Faking one is a bug nobody can locate later | `a_metadata_round_makes_the_file_listable_before_it_is_downloaded`; `catalogue.rs` derives, never records |
| A peer's own clock never decides ordering or expiry, anywhere | It is an attacker-controlled integer, and a Pi 4 with no RTC reports 1970. Made twice — in discovery, then again in the coordinator's peer list — and removed twice | `a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address`; `CoordService::peers_of` uses `Directory::last_seen` |
| The escrow rate limit lives on the server, not the connection | Reconnecting costs a handshake and would buy a fresh budget, which is no budget | `red_team_reconnecting_does_not_reset_the_escrow_attempt_budget` |
| A replication target counts this device | Off by one means the repair loop keeps two copies while reporting three, invisibly, until two machines die instead of three | `a_target_counts_this_device_so_three_asks_for_two_elsewhere` |
| A peer is recorded as a holder only when it accepted or already had the chunk | Recording a refusal as storage is indistinguishable from safety until the local disk dies | `a_host_that_refuses_to_store_is_not_recorded_as_holding_anything` |
| A discovery beacon's address comes from the UDP source, never from the packet | A self-declared address lets any node redirect traffic to a machine that is not it | `a_new_device_is_recorded_with_the_address_it_was_heard_from` |
| The discovery table is bounded and confirmed peers are protected | Device ids are free keypairs, so a flood is cheap; without this it evicts the machines that matter | `a_flood_of_strangers_cannot_evict_a_known_peer`, `the_table_never_grows_past_its_capacity` |
| The sender's clock decides nothing in discovery | A Pi 4 has no RTC and boots in 1970; superseding by sender clock strands it at a stale address | `a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address` |
| Streaming boundaries match slice boundaries exactly | Otherwise one file stored via two paths dedups against nothing | `streaming_and_slicing_agree_on_every_boundary` |
| Published test identities are refused by `Store::open` | Their phrases are in the docs | `the_published_test_identities_are_refused_...` |

## 7. What is built and working

- **Local store**: content-defined chunking, sealed content-addressed blobs,
  transactional index, chained operation log, GC with grace, integrity check.
- **Streaming**: `write_stream`/`read_stream` bound memory to ~½ MB regardless
  of file size. The buffer variants are thin wrappers.
- **Sync**: version vectors, full merge decision table, conflict siblings,
  tombstones, deferred operations, deterministic 3-device simulation.
- **Network**: TLS 1.3, device-authenticated, peer protocol with resume and
  batched have/missing, vault for foreign data, storage challenges, relaying.
- **Placement ledger**: which peers hold each chunk, recorded on every sync and
  converging from what a peer says it already has. `itsanas status` reports
  whether the data exists anywhere but this disk.
- **Discovery**: machines on one network find each other with nothing
  configured — signed 147-byte UDP announcements, a bounded table, own devices
  dialled first, each pinned to the device that announced it. Verified against a
  real broadcast, not only in tests.
- **Folder**: import/export/delete, conflict handling, watcher with debounce,
  periodic and deep rescans, atomic streamed export.
- **Daemon**: serve + sync + reconcile in one process.
- **Placement**: weighted rendezvous hashing, owner affinity, repair
  *planning*. **No anchor rule and no availability input** — see
  [ECONOMICS.md](ECONOMICS.md) §2.
- **Coordinator library**: device claims and revocation, presence, measured
  availability, accounting, account directory, escrow storage.

Verified by running it, not only by tests: two daemons, a file dropped in one
folder appearing in the other, an edit propagating, a file created on the far
side coming back, a deletion removing it from both, both folders byte-identical.

## 8. What is next, in order

1. ~~**Owner-recorded placement.**~~ **Done.** The `HOLDERS` table in the store,
   filled by `session::push` from what the peer says it already has, plus
   `under_replicated` and the `itsanas status` report. What is still missing is
   a repair loop that *chooses* peers to fix a shortfall, rather than relying on
   a node pushing to every peer it has.
2. ~~**Coordinator server and client.**~~ **Done.** `protocol.rs`, `service.rs`,
   `server.rs` and the `itsanas-coordinator` binary, plus CLI wiring:
   `itsanas coordinator`, `itsanas register [--recovery]`,
   `itsanas login --from`. The daemon announces its address each round and
   dials whatever the coordinator reports, pinned.

   What is left here: nothing blocking. The old note said the library was
   complete and
   tested; nothing serves it. Needs: a protocol enum, a `service.rs` handling
   requests against `Directory`, and a TLS server reusing `itsanas-tls` and
   `wire::Connection`. Then a `itsanas-coordinator` binary.
3. **~~Signed node-set epochs~~ — cancelled.** This was going to be the
   coordinator publishing a membership list everyone agreed on. Requiring every
   peer to hold the same list *is* an agreement protocol, and ITSaNAS does not
   need one: every chunk has exactly one owner who already keeps a log of it.
   Superseded by owner-recorded placement above.
3. **CLI wiring**: `itsanas register`, `itsanas coordinator <addr>`, peer
   discovery by username, and pinning peer device ids when dialling (the
   `expect` argument to `PeerClient::connect` is currently always `None`).
4. **Escrow recovery**: `itsanas login --username X` fetching the blob from the
   coordinator. `Keystore` already supports it; only the wiring is missing, and
   it is the recovery story Nicolas originally asked for.
5. **Repair execution.** `placement::repair::plan` is wired to nothing. Needs a
   census built from peer queries, which needs (2).
6. ~~**Scheduled storage challenges.**~~ **Done.** `session::audit` challenges
   a sample of a peer's holdings each round and withdraws the record when it
   cannot answer, which makes the chunk under-replicated and gets it re-sent.
   What remains is *reputation*: nothing remembers that a peer failed, so a
   host that fails every audit is silently corrected forever instead of being
   dropped.
7. **Benchmarks.** There are none. Nobody knows the throughput on a Pi, which is
   the machine that matters.
8. **Raspberry Pi bring-up.** Never run on ARM. Only `cargo check` for
   aarch64 has been done, and blake3 needs a cross C compiler.

## 9. Known gaps, deliberately open

- **Tail truncation.** A host can serve an internally consistent *prefix* of a
  segment chain. Detecting it needs signed, timestamped head records gossiped
  between peers. Documented at the top of `store/src/oplog.rs`.
- **Usage is self-reported.** A member who under-reports gains entitlement they
  have not earned. Verifiable usage needs hosts to report what they hold.
- **Storage challenges prove possession at a moment**, not continuously, and a
  host that fetches from another replica just in time passes.
- **No bandwidth accounting.** 10 TB on a 1 Mbit uplink is worth far less than
  the number says. Deferred because measuring it badly punishes people for their
  ISP.
- **One process per node.** The index is under an exclusive lock, so commands
  refuse to run while the daemon holds it. A local control socket is the fix.
- **The escrow attempt counters are in memory only.** A coordinator restart
  clears every one of them, so anybody who can provoke a restart — or who simply
  waits for one — gets a fresh budget. Persisting them is the fix; the Argon2id
  cost is what carries the weight until then.
- **A stale address is handed out for a week.** `PRESENCE_TTL` bounds it, but
  every peer pays a dial for every stale entry until it expires.
- **The blob layout does not reach a terabyte.** Measured, not suspected: see
  [ROADMAP.md](ROADMAP.md) M9. One file per chunk is 14.7 million files per
  terabyte, and `blobs().addresses()` walks all of them on every sync round.
  Pack files are the decided answer, scheduled after the coordinator because
  M9's third measurement showed the *daily* experience is already fine — a Word
  document saves in 28 ms.
- **No file-level sharing between users.** Not needed for mutual storage;
  `UserKeys::agree` exists, is tested, and is deliberately unused until it is.

## 10. Open, waiting on Nicolas

Three things are deliberately not decided, and none of them should be decided
unilaterally:

1. **Publishing.** No remote exists. AGPL-3.0 is chosen and `deny.toml` allows
   it, so the licence side is ready; whether and where to publish is not.
2. **Merging to `main`.** The whole project after the initial commit lives on
   `overnight-m2-to-m5`. Merging locally is trivial and reversible; it was left
   undone because "push to main" was asked for in a context that assumed a
   remote.
3. **Installing on the Windows laptop.** `cargo install --path crates/itsanas-cli`
   puts `itsanas.exe` on the PATH. Not done — it writes outside the repository.
   [QUICKSTART.md](QUICKSTART.md) is the walkthrough once it is.

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
used twice on this project and found real gaps both times. Worth repeating after
each substantial milestone.
