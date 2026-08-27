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

Test counts in TESTING.md are mechanical:

```bash
cargo test --workspace -- --list
```

## 3. Verify a clean tree in one go

Every gate CI runs, runnable locally:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo test --workspace --all-features
cargo test --workspace --all-features -- --ignored     # 2 slow tests
cargo +1.88.0 check --workspace --all-features          # MSRV
cargo deny --all-features check
```

All of these pass as of the last commit. **MSRV is 1.88** (let-chains), not the
1.85 that edition 2024 alone would need.

## 4. Crate layout and what each is for

```
crypto     identity, key schedule, sealing, blinded addressing, keystore
testkit    Alice/Bob/Carol — published test users, generated corpus, canaries
wire       length-prefixed framing + a generic Connection<S: Read + Write>
tls        anonymous TLS + device authentication bound to the channel
store      chunking, blob store, index, operation log, vault, version vectors
sync       version-vector merge, conflict resolution, convergence simulation
net        peer protocol, TLS transport, push/pull sessions
placement  rendezvous hashing (integer, no floats), repair planning
coord      device certificates, accounting, directory  ← library only, no server
folder     a real directory mirrored into the store and back, with a watcher
cli        `itsanas` binary: commands + daemon
```

Dependency direction is strict: `crypto → store → sync → net → cli`, with
`wire`/`tls` beside them and `coord` deliberately unable to reach `store` or
`sync`.

## 5. Decisions that must not be quietly reversed

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
| Streaming boundaries match slice boundaries exactly | Otherwise one file stored via two paths dedups against nothing | `streaming_and_slicing_agree_on_every_boundary` |
| Published test identities are refused by `Store::open` | Their phrases are in the docs | `the_published_test_identities_are_refused_...` |

## 6. What is built and working

- **Local store**: content-defined chunking, sealed content-addressed blobs,
  transactional index, chained operation log, GC with grace, integrity check.
- **Streaming**: `write_stream`/`read_stream` bound memory to ~½ MB regardless
  of file size. The buffer variants are thin wrappers.
- **Sync**: version vectors, full merge decision table, conflict siblings,
  tombstones, deferred operations, deterministic 3-device simulation.
- **Network**: TLS 1.3, device-authenticated, peer protocol with resume and
  batched have/missing, vault for foreign data, storage challenges, relaying.
- **Folder**: import/export/delete, conflict handling, watcher with debounce,
  periodic and deep rescans, atomic streamed export.
- **Daemon**: serve + sync + reconcile in one process.
- **Placement**: weighted rendezvous hashing, owner affinity, anchors, repair
  *planning*.
- **Coordinator library**: device claims and revocation, presence, measured
  availability, accounting, account directory, escrow storage.

Verified by running it, not only by tests: two daemons, a file dropped in one
folder appearing in the other, an edit propagating, a file created on the far
side coming back, a deletion removing it from both, both folders byte-identical.

## 7. What is next, in order

1. **Coordinator server and client.** The library (`coord`) is complete and
   tested; nothing serves it. Needs: a protocol enum, a `service.rs` handling
   requests against `Directory`, and a TLS server reusing `itsanas-tls` and
   `wire::Connection`. Then a `itsanas-coordinator` binary.
2. **Signed node-set epochs.** `NodeSetEpoch` does not exist yet. Coordinator
   signs, peers pin, placement consumes. This is what makes `placement` usable.
3. **CLI wiring**: `itsanas register`, `itsanas coordinator <addr>`, peer
   discovery by username, and pinning peer device ids when dialling (the
   `expect` argument to `PeerClient::connect` is currently always `None`).
4. **Escrow recovery**: `itsanas login --username X` fetching the blob from the
   coordinator. `Keystore` already supports it; only the wiring is missing, and
   it is the recovery story Nicolas originally asked for.
5. **Repair execution.** `placement::repair::plan` is wired to nothing. Needs a
   census built from peer queries, which needs (2).
6. **Scheduled storage challenges** and recording the results, so a host that
   discards data is caught rather than trusted.
7. **Benchmarks.** There are none. Nobody knows the throughput on a Pi, which is
   the machine that matters.
8. **Raspberry Pi bring-up.** Never run on ARM. Only `cargo check` for
   aarch64 has been done, and blake3 needs a cross C compiler.

## 8. Known gaps, deliberately open

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
- **No file-level sharing between users.** Not needed for mutual storage;
  `UserKeys::agree` exists, is tested, and is deliberately unused until it is.

## 9. Open, waiting on Nicolas

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

## 10. Working style Nicolas expects

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
