# Handover archive

The dated entries that used to sit in [HANDOVER.md](HANDOVER.md) §0, moved
here verbatim on 2026-10-01 so that §0 fits on a page again. This is
history, not instructions: what an entry calls "next", "open" or "owed" may
have been done since, and "above" or "below" refer to §0 as it was (the
2026-09-15 entry sat above the 2026-09-16 one there; here it is newest
first). The current step is the `ITSANAS-STATE` block in HANDOVER.md §0, the
standing rules are HANDOVER.md §3, §4b and §11, and the code wins over
both.

---

**2026-10-04, §8 4c: the ledger walk no longer lists an idle account**
(branch `ccr-c9f4329d-v9cnbp`). Owner: a due walk against a peer that
summarises lists only the differing buckets, in full, and re-stamps the
agreeing ones locally (`restamp_agreeing`, `session.rs`), after recomputing
each bucket's digest from the rows it re-stamps (`summary::bucket_digest`).
Host: every daemon round runs `check_disks` (`daemon.rs`):
`Vault::check_disk` removes index rows whose blob is gone, cursor kept on disk
in a table of its own; `Store::check_disk` records own losses in one
transaction, and the own-account summary is `Store::held_summary` (live less
losses). Slice: `holders::rows_per_round` = rows × interval / REFRESH_AFTER,
rounded up. Verified: 9 tests (6 red-team), 6 sabotages red; `cargo test -p
itsanas-store -p itsanas-net` green.

Rodin on the plan, three fixed before code: the summary-then-re-stamp race
(digest recomputed per bucket); own-account peers answer from the store index,
not the vault (held summary + ordered store pass, the random repair scan takes
~27 days a pass at 1 TB); a fixed 16 384-row slice is wrong at any other
interval. **Named, not fixed (ROADMAP):** an older host, or `itsanas serve`
alone, never checks its disk and is believed; one bucket of ids in memory per
due walk (20 MB at 10 TB); ~16 000 file lookups a round at 1 TB, never timed;
the service's switch to `held_summary` has no end-to-end test (unit-tested
in the store). Trap: my hand arithmetic said 15 873 rows a round, the code
said 15 874 -- the test caught me, the code was right.

**2026-10-04, §8 4b: a full peer is not re-sent what it refused** (branch
`step/8.4b-refused-reoffer`). `ask` stops offering at the first
`Refusal::PledgeFull` and stamps the peer (index table `peer_full`); for
`FULL_RETRY` (1 h, `session.rs`) a round asks that peer only about chunks it
has a record for (`Index::with_record`) and offers nothing; then one ordinary
round probes. Verified: 1 red-team test, 4 sabotages red. Also corrected: the
summary is 256 digests, 8 KB a round on the wire, not "thirty-two bytes"
(DESIGN §6.7, ROADMAP, FIRST-STEPS). §10 8 decided under Nicolas's delegation
("if it is a no-brainer, do it"): yes, *with* the host-side disk check, see 4c.
**Not verified:** nothing measured at size; a peer whose pledge grows waits up
to an hour.

**2026-10-04, §8 4a: a change asks only about what the peer has not
confirmed** (branch `step/8.4a-narrowed-sweep`). Within a bucket the summary
names, `sweep` (`crates/itsanas-net/src/session.rs`, now `sweep`/`walk`/`ask`/
`bucket_floor`) asks only about chunks with no record for that peer younger
than `REFRESH_AFTER` (`Index::without_fresh_record`, the same rule
`record_holders` writes by), and reads only those buckets' index ranges. The
full walk, due every `REFRESH_AFTER`, still asks about everything. Verified: 2
red-team tests, 3 sabotages red (filter removed, filter in the full walk,
bucket range starting past its first chunk); `cargo test -p itsanas-net -p
itsanas-store` green. **Found doing the arithmetic:** FIRST-STEPS said an idle
terabyte costs 600 KB a day; the full walk lists 537 MB per peer every 3.5
days, about 150 MB a day, over budget -- already false before this change,
corrected. **Found by Rodin, read in code, not measured:** a peer with a
smaller budget is re-offered every chunk it refused, with the bytes, every
round (now §8 4b). **Not verified:** nothing measured at size; disk cost on an
SD card. Trap: the first fixture put no confirmed chunk in the new chunks'
buckets, and its guard assertion is what said so.

**2026-10-01, final fixes of the run's verification pass** (branch
`step/final-redteam-fixes`; not a §8 step). Five findings of an
`itsanas-redteam` pass over `8701222..main`, each re-read in the code first:
`register` refuses after `leave` (Android has no `leave`, so its half did not
hold); `leave` keeps the claim and its slot and says to run `itsanas device
forget <id>` elsewhere; a derived home with no node is refused when the other
of HOME/USERPROFILE holds one (`config::stranded_node`), and `migrate`'s advice
says the old unit restart-loops; `Index::open` waits out a lock for up to 2 s
(`LOCK_PATIENCE`) so an `is_locked` probe cannot keep a starting daemon down;
`held_for_others` reads a per-owner running total (table vault_owner_chunk_bytes)
instead of two walks; a pull charges the synced folder's copy too
(`FolderCopy`: twice on the home's volume, the folder volume's free space
otherwise). Verified: 9 tests, 11 sabotages red. **Not verified:** no CLI run
by hand; the Windows drive-prefix comparison runs only in CI; a command beside
a running daemon now takes 2 s to say `Locked`. Trap: a Python edit script
run through `runpy` executes its module-level code twice.

**2026-10-01, folder reports name files and recognise a rename** (branch
`step/folder-reports-renames`; not a §8 step, the second "not timid on
features" PR, chosen over §8 4 because every user reads these lines every
day and a move shown as "1 deleted locally" sends people to restore files
they only moved, while 4 matters only past a few hundred GB).
`ReconcileReport` gains two rename lists, here and elsewhere (one deletion
and one addition of the same non-empty bytes in one pass, unambiguous only),
`summary` counts a rename once, `lines(limit)` names every file; `itsanas
scan` prints all, `sync` and the daemon every deletion and conflict, then
the first `LINES_IN_A_LOG` (20) others, and count the rest. Report only: the
store and the log still do delete + create. `itsanas-redteam` found two ways
the report hid a deletion -- a log bound cutting deletions and conflicts
into "N more", and one of two identical copies deleted shown as `mv` -- both
fixed and tested; it suspects (older, untested) that a file name holding a
newline can forge a log line. Verified: 6 functional tests, 7 sabotages red.
**Not verified:** no CLI run by hand; Android shows no per-file report.
Others named and left: §8 4; a "listed only" phone mode (ROADMAP M12); the
tray showing the folder's state; `fetch_only`/`drain_vault`/budgeted
`keeping::round` still failing whole on one refused chain. Traps: Python
`write_text` on Windows writes CRLF (use `write_bytes`);
`check-catalogue.sh` reads any backticked snake_case name in a doc as a test
name.

**2026-10-01, 2e: the Android app lists and withdraws devices** (branch
`step/8.2e-android-devices`). Settings, "This account's devices": the
coordinator's list, this phone marked, Withdraw beside the others, a
confirmation saying the consequence. Joining at the 5-device limit opens that
screen on the ids the refusal named (`coordinator::cap_named`), so an
all-phone account whose phones are lost frees a slot from the new one. Core
shared with the CLI: `coordinator::withdraw_device` (refuses the device
asking); phone rule: only a full id the listing shows. Also: `setLimits` saves
pledge and keep as a pair (raising both was refused), split refusals in the
phone's words, a test holding `Native.kt` to the JNI exports.
`itsanas-redteam` found 3, fixed and tested: stale remembered ids in a
complete list, a partial refusal miscounted, a negative pledge taken as
positive. Verified: 7 Rust tests, 11 sabotages red, APK built by hand.
**Not verified:** no screen opened, Kotlin untested (no CI job runs the app);
the node lock is held across the coordinator dials of `withdraw` (redteam,
suspected, not measured). Trap: Git Bash heredocs ate a `\` in a Python
edit script again -- write scripts with the Write tool.

**2026-10-01, 2c: one refused chain no longer stalls a pull** (branch
`step/8.2c-refused-chain`). `session::pull_scoped` only: `keep_chain` keeps a
chain's genuine prefix when `put_segment` refuses a segment, `apply_per_chain`
leaves out a chain failing `validate_chain` or `open_segment` and applies the
others; `SyncReport::refused_chains` (new field, `absorb` to sum) is printed by
`itsanas sync` and the daemon. No protocol or `validate_chain` change. Worse
than §8 said: a stranger's free key signing one segment under a user id
stalled that account's pull on every node, every round. Verified: 3 tests,
4 sabotages red; a marker condition I added turned nothing red and was
removed. Not done (ROADMAP "One refused chain"): `fetch_only`, `drain_vault`,
budgeted `keeping::round`; refresh stage not end to end. Over the 20-point
cap, so no `itsanas-redteam` pass. Next: 2e, Android withdraw.

**2026-10-01, 2d: at most 5 live devices per account** (branch
`step/8.2d-device-cap`; Nicolas's decision of 2026-09-30, §6). One constant,
`MAX_DEVICES_PER_ACCOUNT` (`itsanas-coord` `claim.rs`), enforced in
`Directory::claim` (inside the write transaction, nothing written on refusal)
and in `coordinator::register_with` before signing. Re-signing a live device
takes no slot, a withdrawal frees one, a withdrawn device stays out, an
account above 5 keeps its devices. `check-bargain.py` now also holds four
documents to the number (no new gate, so no `ci.yml` change). `itsanas-redteam`
found a permanent lockout (all five machines lost, the new one could neither
register nor `forget`): fixed, refusals carry full ids and `forget <full id>`
lists nothing. Verified: 10 tests, 10 code sabotages red, the gate red three
ways, the CLI path by hand both ways. Not done: Android has no withdraw;
withdrawn rows still unbounded. Next is 2c.

**2026-09-30, 2b: confidentiality surface red-teamed by hand** (branch
`step/8.2b-confidentiality-redteam`). Table in ROADMAP "The confidentiality
surface, by hand". One new red-team test, sabotaged red by a convergent seal:
two accounts sealing one file share no sealed bytes. **Found, named, not
fixed:** reads are not scoped to the account -- any device key (anyone can
make one) reads any account's envelopes, addresses and sealed chunks from any
host, given the user id; the fix is the roster of §10 question 7. By design:
op counts, sizes, cadence. **Not tested:** the has_chunk race in `kept` (no
seam for a second writer) and repeated fetches of a shared noisy chunk (the
simulator counts no fetches); the path-echo answer is from reading. §10
gains question 7 (the #207 re-signing hole). Next, 2c: still agent-doable,
client-side, not enforcement. Trap: `sabotage.py` with `cargo test -q`
reports "the build itself refused it" for a test that failed; drop `-q`.

**2026-09-30, 2a: integrity surface red-teamed by hand** (branch
`step/8.2a-integrity-redteam`). Table of attacks, defences and tests in ROADMAP
"The integrity surface, by hand". Two holes fixed, one named: a chunk refused by
`accept_chunk` counted as fetched (`itsanas-sync` `fetch_missing`), so the file
was adopted with a hole and the round read as finished; and a relay answering
noise was recorded as a holder (`session.rs` `kept`). Three red-team tests, three
sabotages red. **Named, not fixed:** a host re-signs a lifted segment body under
its own key and it opens (the seal omits the device), replaying the owner's own
ops and undoing releases. A version check was written, then withdrawn after
`itsanas-redteam` showed it stalls peers on a keystore-replaced node. The fix is
a protocol change. **Not verified:** the
has_chunk race in `kept` and repeated fetches of a shared noisy chunk (named in
ROADMAP). Next, 8.2b: confidentiality, the last §8 2 surface.

**2026-09-30, 3d: the LAN beacon is version 2** (branch
`step/8.3d-beacon-keyed-tag`). Tag field = 16 B random nonce + 16 B
`keyed_hash(UserKeys::lan_tag_key, nonce || device)`, 147 B kept, no clock.
The daemon already held `Node::user`, so no redesign. `parse` reads v1 and v2;
v1 is `OwnerTag::Legacy`: dialled, never "mine". **§8 3d was wrong** ("v1 heard
as a stranger is safe"): `parse` refused every version but its own. Verified:
seven new red-team tests, one plain; eight sabotages red (fixed nonce, unkeyed
hash, key = user id, device out of the hash, v2-only check, legacy read as
mine; from `itsanas-redteam`: a replayed v1 beacon demoting a v2 machine, now
refused, and the table checking the wrong device). **Not verified:** a real v1
build against a v2 one (a test helper builds the old layout); a v1 build refuses
v2, so an old machine hears nobody and is found only by being dialled. Named, not fixed:
a replayed v2 beacon reads as "mine" (one dial, TLS pinning refuses). Next is
8.2a: all other open §8 items are host-side or wait on Nicolas.

**2026-09-30, 3b: the pledge reads a running total** (branch
`step/8.3b-chunk-bytes-total`). `Vault::held_bytes` = `vault_totals`
`chunk_bytes` (changed in the same transaction as each `vault_chunks` row)
plus `CHAIN_BYTES`; read by `would_exceed_pledge`, `host_for` and
`Node::held_for_others`. A put indexes the size **on disk** (a re-put keeps
the old file; indexing `sealed.len()` let a peer's 1-byte re-put hide 8 MiB).
An `open` mark, set at open and cleared on a clean `Drop` (not after a
write that failed past its blob, nor a failed open: both found by
`itsanas-redteam`), makes the next open rebuild index and total from the
directories and sweep staging. Measured: 3.4 s
per refused offer at 50k chunks, ~1 µs now (ROADMAP). **Verified:** six
tests, eleven sabotages red. **Not verified:** a real kill mid-write (the crash
is simulated); a daemon killed by the Task Scheduler walks once per restart.
`itsanas status` still walks, on purpose.

**2026-09-30, 3a: every setter keeps the split** (branch
`step/8.3a-pledge-keeps-split`). `Node::check_split` (`itsanas-node`
`node.rs`) is the one rule, asked by `keep`, `space --apply`, `pledge` (new
`set_pledge`) and the phone's `set_pledge`/`set_keep` (the JNI bodies,
extracted); `pledge` counts hosted bytes with `held_for_others`, now public.
Refusal text ends "or keep less" (was "ask for less"; QUICKSTART follows).
**Verified:** three red-team tests, sabotaged eight ways (the check always
`Ok`; `allowed_for` on `Split::DEFAULT`; each call dropped; each moved below
its assignment), red each time. **Not verified:** the JNI shims themselves --
the tests reach `set_pledge`/`set_keep`, not `Java_..._setPledge`/`setKeep`.
**Decision:** strict -- a node already keeping more than it earns has
`setKeep` (order, filter) and a too-small raise of `pledge` refused until keep
is lowered. Left as found: negative `setPledge` via `unsigned_abs`, and a
suggested `--keep NG` rounded up past its own quoted pledge (itsanas-redteam).

**2026-09-30, 0f cut down: the tray icon starts at logon, per node**
(branch `step/8.0f-tray-at-logon`). `install/provision.ps1` copies
`scripts/itsanas-tray.ps1` beside `itsanas.exe` and writes a shortcut in the
user's Startup folder, `ITSaNAS tray.lnk` or `ITSaNAS tray (NAME).lnk`,
running `conhost.exe --headless powershell.exe ... -File <installed script>
[-Instance NAME]`; `-NoTray` skips it. `install/clean.ps1 -Instance NAME`
removes that literal shortcut; the full clean removes `ITSaNAS tray*.lnk`
with the programs. **Decision, against §8 f's "a second scheduled task":**
every task named `ITSaNAS*` is read as a node (`provision.ps1` ~270, "other
nodes"; `clean.ps1`'s `ITSaNAS-*`), and `ITSaNAS-tray` *is* the task of an
instance named tray; a shortcut needs no admin either. conhost.exe checked
to be a GUI-subsystem binary (PE subsystem 2), so Explorer starting it opens
no console. **Verified:** `check-installers.sh` evaluates the shortcut's
name and arguments as written in provision.ps1 for two instances and the
default, then runs `clean.ps1 -Instance zz-check-b -Yes` for real against a
throwaway Startup folder; sabotaged three ways (no `-Instance`, a glob in
the clean, one name for all), red each time. The shortcut code, run into the
scratchpad, wrote the expected target and arguments. **Not verified:**
provision.ps1 and clean.ps1 were not run on this laptop (real tasks and
node), so no shortcut exists here and the icon has still never been seen;
an icon already showing is not stopped by `clean.ps1 -Instance` (it stays
until logoff or Quit). **No menu item added**: pause, disconnect and
decommission wait for Nicolas to see the icon. **Next is 8.3a**, not 1c: §8 0
defers host-side enforcement until the fleet MVP, and 3a is the honest
client's own check.

**2026-09-30, 1b's pull half: a download is held to the disk ceiling**
(branch `step/8.1b-pull-bound`). Chosen over 0f's second half by an
`itsanas-lead` checkpoint. `apply_upsert` and `apply_conflict`
(`crates/itsanas-sync/src/engine.rs`) ask `Store::pull_room` per file
**before its first chunk**; a file past `local_ceiling` returns the new
`Applied::NoRoom`, counted in `deferred` (so the markers stay and it is
retried) and in `SyncReport::no_room` (so `sync` and the daemon say "left on
the other machines: no room"). Not in `accept_chunk`: a per-chunk refusal
half-writes the file, and repair uses it for files already counted.
Only the disk half applies to a pull; the file is already in the account.
**Verified, then fixed:** `held` in `disk_room` did include our own
account's chunks -- `Vault::stats` sums every owner, and our devices push to
our vault -- so `Node::held_for_others` subtracts `stats_for(own)`. The
daemon loop without a folder, `sync_folder` and Android's sync now refresh
the bound before pulling (before, the ceiling there was `Node::open`'s
`None`). Three red-team tests, six sabotages, red. **The `itsanas-redteam`
agent** found `stats_for` creates our own vault directory (a fresh node then
"hosted" one account): guarded, tested. Its other findings are named, not
fixed, in `ROADMAP.md` ("The disk bound (1b) on pulls"): `itsanas get`
unbounded by decision, frozen markers replay every round while a file waits
for room, the refresh fails open, the three call sites untested. **Next is
0f cut down** (see §8 f): logon start only, nothing destructive until
Nicolas has seen the icon.

**2026-09-30, 0f first half: the tray, as `status --brief` + PowerShell**
(branch `step/8.0f-brief-status`). **Decision, against the letter of §8 f
("a new crate `itsanas-tray`"):** `cargo deny` rejects `tray-icon` and
`winit` -- MPL-2.0 and BSD-2-Clause licences and two "unmaintained"
advisories, all from their GTK/Wayland dependencies. Declaring the crate
`cfg(windows)`-only does not help: with several `[graph] targets`, cargo
deny judges each dependency edge on every listed target (checked: each
target alone passes, the list fails). Calling `Shell_NotifyIcon` directly
needs `unsafe`, allowed in one file only. So the rule lives in Rust and the
drawing in PowerShell, which ships `NotifyIcon`: `itsanas status --brief`
prints `healthy|stale|stopped|departed|unknown [age]` without a
passphrase; `healthy` needs the store lock **and** a snapshot within two
intervals, which the daemon now writes into the stamp (`snapshot T every
S`; readers take the first number, so old snapshots still parse).
`scripts/itsanas-tray.ps1 [-Instance NAME]` draws it: icon + tooltip every
30 s, left click opens the folder (from `itsanas instances`), menu opens
the log, restarts the task, quits. One red-team test, sabotaged red; the
script parses in pwsh and Windows PowerShell 5.1. Run against the laptop's live node: `status --brief`
said `healthy`, and the script's reader functions found the folder and the
state. **🟨:** the icon and menu themselves were never shown (a GUI in
Nicolas's session); nothing launches it at logon.

**2026-09-30, 0o: a v5 peer is proven not asked** (branch
`step/8.0o-v5-not-asked`). `PeerService::speaking_at_most(v)` (doc-hidden,
test use) caps the version admitted to in `Hello`; with a counting `Relay`,
`two_nodes.rs` proves a v6 service is asked once and a v5 one never. One
red-team test, sabotaged red. **Judged not worth a test:** the daemon's
relay wiring (`dial_listed`, ~899) is five lines between two tested pieces
(the client gate, `Contact::relayed`); testing it needs two full nodes in
the CLI's tests. 0o stays 🟨 only for that and for phase 3, which waits on
the fleet. **Next is f, the tray.**

**2026-09-30, 0o: the 2099-clock gap closed** (branch
`step/8.0o-clock-ahead`). `Contact::relayed` takes `now`; a row whose
presence is dated past `now + MAX_CLOCK_SKEW` is refused, and a held
presence dated so no longer counts as the latest -- so a device whose clock
once read 2099 no longer blinds the relay for good. One red-team test, two
sabotages, red. **Left in 0o, 🟨:** no test that a v5 peer is not asked
(`PeerClient::presences` returns early when `spoken < 6`; nothing emulates a
v5 server -- the cheapest way is a test-only protocol cap on
`PeerService`'s `Hello` answer and a counter of `Presences` requests), and
the daemon's relay wiring (`daemon.rs` ~899) has no test of its own.

**2026-09-30, 1b closed: the disk half of bounded writes** (branch
`step/8.1b-disk-bound`). `WriteBudget` gains `local_ceiling: Option<u64>`, the most bytes of the
account this device may hold locally; `Store::refuse_past` refuses a write
that would take the local bytes past it with the new `StoreError::DiskFull
{ incoming, room }`, on the same path as the account bound, so nothing is
left behind. `Node::bound_writes` sets it to local bytes now +
`Node::disk_room(free, pledge, held)` = free space (`fs4`, new dependency of
`itsanas-node`, already in the lock via the CLI) less what the pledge still
owes beyond the vault's bytes; `None` only when the free space cannot be
read (a real 0 bounds everything), and at `Node::open`, whose vault opens
later -- every writing path calls `bound_writes` first. **The
`itsanas-redteam` agent broke the first version before merge:** it compared
each write alone against a per-pass snapshot of the room, so a pass of a
hundred 1 GB files fit in 10 GB; and it read a full disk (0 free) as
unknown, i.e. unbounded. Both fixed, each with a test sabotaged red. Its
other findings are named in `ROADMAP.md` ("The disk bound (1b)"): pulls
bypass the bound, and `held` may count our own chunks. Three red-team tests,
four sabotages, red. **Skipped 8.0k on purpose:** its
agent half (version, CHANGELOG, re-pin FIRST-STEPS) would point the
documented install at a `v0.2.0` tag that does not exist until Nicolas cuts
it -- found by the `itsanas-lead` checkpoint. It is now marked "with
Nicolas". Also from that checkpoint: 0p is 🟨, not ✅, while its service
advice is unexercised. **Not 1c next:** §8 0 defers all further
enforcement until the MVP passes on the fleet, and 1c is host-side
enforcement; `NEXT` is 0o's leftovers instead. Trap this time: restoring a sabotage with
`git checkout <file>` wiped the real edit in that file; back up to the
scratchpad and copy back instead.

**2026-09-30, 0q closed: the leftovers** (branch `step/8.0q-leftovers`).
`sync` ends with a line on stderr naming every peer that refused
(`finish_sync`; exit status kept at 0, because `acceptance-local.sh` expects
a round against a pledge-0 host to succeed and print why); `instances` says
"daemon running/stopped"; `install/android.md` says how to point the phone
at a computer (address + `status`'s `listen` port) and the Doze / off-LAN
limits; install/README says a second *Windows account* needs its own
`itsanas.exe` and **must still use `-Instance`**: task names are
machine-wide, so its plain `ITSaNAS` task would be the first account's
(read from `provision.ps1` ~564, `Register-ScheduledTask -Force`; not run
with two real Windows accounts -- add it to 0i's human runs). **Not done, named:** `scan`/`sync`
folder lines still name no files and show a rename as delete + add (a
reporting redesign, not friction); no test for the new stderr line.

**2026-09-30, 0q, second persona (Windows + Android): the pledge, the
home, the instance** (branch `step/8.0q-pledge-first`). A second persona
run (a second account `--instance sam` beside another person's node, accents
and nested folders; Android walked on paper) found: a new node pledges 0 and
so refuses **even its own account's other devices** (`refused ... pledge is
full or zero`) while `sync` exits 0 -- FIRST-STEPS §2 stopped at "add
`--apply`" without saying it is required; now it says so and shows the
command. §3 says which commands need the daemon stopped and how (systemd,
`Stop-ScheduledTask ITSaNAS`); §4 that both machines need a daemon, and a
paragraph on a second account (`--instance`, `ITSANAS_INSTANCE`,
`instances`). **Code:** `config::dirs_home` preferred `HOME` everywhere, its
comment claiming both variables exist on every platform -- false on Windows
(checked: no persistent `HOME` on SIGSEG-DELL), where `provision.ps1` uses
`USERPROFILE`; now `home_from` puts the profile first on Windows. One
red-team test, one sabotage, red. **Risk named:** a Windows user whose `HOME`
differs from the profile and who made a node with the CLI now finds it
elsewhere (`itsanas --home` still reaches it); nobody on the fleet is in
that case (checked on the laptop only).

**2026-09-30, 0q (A)-(C): the worst first-user friction** (branch
`step/8.0q-sync-folder`). `sync` now runs the daemon's own folder reconcile
(`sync_folder` -> `daemon::reconcile_once`, so 0l's guards apply) before its
rounds, so local changes go out, and after them for `Scope::Everything`, so
what came in lands in the folder. `login --phrase-file` accepts the grid
`init` prints (`phrase_words` drops `N.` tokens; `phrase_grid` is now the
one printer). The store-lock message names the daemon. FIRST-STEPS §4 adds
the missing `itsanas folder` on the second machine (the persona had set it
without noticing the guide never did), the words file's formats, the
per-machine passphrase and how to read the port. One red-team test + one
functional, one sabotage, red. **🟨:** `sync`'s call sites of
`sync_folder` are one line each and untested; FIRST-STEPS points at `status`'s
`listen` line, which the daemon's snapshot carries (checked in
`render_status`).

**2026-09-30, 0m closed: a departure survives a restart** (branch
`step/8.0m-departed`). **Decision, against the letter of §8 m:** `leave` is
*not* wired into `ExecStop=`/the task's stop. The service stops at every
reboot and upgrade, so that would announce a departure each time and have
the peers re-replicate the whole machine after a restart. The contradiction
to prevent is the reverse -- a machine that left, restarted by systemd or
the logon task -- so `leave` now writes `<home>/departed` (unix time),
`daemon` exits **0** at once when it is there (0, so `Restart=on-failure`
does not loop), `serve` and `sync` refuse, all naming `itsanas rejoin`,
which removes it. One red-team test, one sabotage, red. **🟨:** the
dispatch arm (`start_daemon`) has no test of its own; the tray (item f)
must show "departed", not "stopped".

A persona run (a subagent following `FIRST-STEPS.md` literally, two
throwaway HOMEs on the laptop, account created, restored from the words,
201 files synced both ways, a deletion propagated, a second instance)
worked end to end and found the friction now in §8 **q**.

**2026-09-30, 0i's red-team test, hermetic** (branch
`step/8.0i-twin-instances`). `scripts/check-installers.sh` ("two instances,
one cleaned") provisions `a` and `b` with `provision.sh --instance` in a
throwaway HOME -- stub `itsanas` at `~/.local/bin`, a `systemctl` first on
`PATH` that only logs, the scripts copied out of the checkout so no
`smoke.sh` runs against the stub -- cleans `b` with `--purge-account`, and
requires `a`'s home, `a.environment`, the `itsanas@.service` template and
a's enablement to survive, with no stop/disable of `itsanas@a`. Two
sabotages of `clean.sh` (glob `*.environment`; disable `itsanas@*`), two
red. Already a CI step, so no `ci.yml` change. **Still 🟨 in 0i:** it proves
the scripts' file and unit bookkeeping, not that `a` keeps syncing; the
cold human runs on Windows, the Pi, the VM and a Mac remain Nicolas's.

**2026-09-30, 0p closed: (3) the refusal, then Rodin** (branch
`step/8.0p-refuse-unnamed`). With neither `--instance` nor `--home`
(`ITSANAS_HOME`), `config::default_home()` is now `config::unnamed_home`:
`~/.itsanas` while it holds a node or while the machine has no node at all,
**a refusal naming the instances** once only named ones exist. It lives in
`itsanas-node` so the CLI (`resolve_home`) and `itsanas-drive` share one
rule -- the drive had its own copy of the old fallback, found only while
answering Rodin. `instances` and `migrate` are answered before any home is
resolved (otherwise they would refuse exactly when needed), and `migrate`
refuses an explicit `--instance`/`--home` instead of ignoring it (Rodin).
One red-team test, sabotaged twice (in the CLI, then after the move to
`itsanas-node`), red both times. Rodin's named, not fixed, in `ROADMAP.md`
("Named instances (0p)"): unexercised service advice, lock/rename race,
`clean.sh` blind to named homes. Trap: Git Bash heredocs ate `\n` twice this
session -- the skill's rule (Python scripts in the scratchpad) is not
optional.

**2026-09-30, 0p (2): `itsanas migrate`** (branch `step/8.0p-migrate`).
`migrate_unnamed` (`crates/itsanas-cli/src/main.rs`) renames `~/.itsanas`
to `~/.itsanas-<account>` (or `--name NAME`) with one `rename`: keystore,
config, store and vault all live inside the home, so a copy is never needed
and would clone an identity. Refused while the store lock is held (daemon
running), onto an existing home, and for an account name that is not a valid
instance name (then `--name`). `config::instance_home_in(base, name)` is the
testable core of `instance_home`. The service is **told, not rewritten**
(`migration_advice`): Linux disable `itsanas`, move `environment` to
`<name>.environment`, enable `itsanas@<name>`; Windows unregister task
`ITSaNAS`, rerun `provision.ps1 -Instance <name>`. Two red-team tests, two
sabotages (copy keystore+config without the store; drop the existing-home
check), two red. Trap: the config file is `config`, not `config.toml` --
my first sabotage copied a name that does not exist and went red for the
wrong reason. **Not done, 🟨:** the advice is unexercised on a real
systemd/Task Scheduler (not run on Pi/VM); whether `provision.ps1 -Instance`
adopts an existing home without re-init is unverified; no Rodin (due with
(3)).

**2026-09-30, 0p second third: `itsanas instances`, `passphrase` refused by
the scripts** (branch `step/8.0p-instances`). Sliced again by Nicolas: weekly
quota at 96 %. `instances_report` (`crates/itsanas-cli/src/main.rs`) lists
`~/.itsanas` as `(unnamed)` and every `~/.itsanas-*` **directory holding
`keystore.bin`**, each with account, home, folder (reachable only if 0l's
`.itsanas-folder` marker is there -- an empty mount point is UNREACHABLE) and
running/stopped from the store lock `status` already trusts, no passphrase.
`provision.sh`, `clean.sh`, `provision.ps1`, `clean.ps1` refuse `--instance
passphrase`; `check-installers.sh` probes the two `.sh` (refused, file
untouched). **Correction to the entry below:** the `clean` globs never matched
the passphrase file -- `clean.sh` tests `-d`, `clean.ps1` uses `-Directory`;
the real exposure was provision's `mkdir` and clean's plan aimed at it. Two
red-team tests + one gate probe; four sabotages, four red. **Not done, 🟨:**
the `.ps1` refusals have no test (no PowerShell in the installer gate); no
Rodin (a slice; due once 0p closes).

**2026-09-29, 0p first half: `itsanas --instance NAME`** (branch
`step/8.0p-instance-flag`). Split in two by Nicolas because the weekly quota
stood at 92 %. `config::instance_home(name)` maps a name to `~/.itsanas-NAME`,
the mapping `provision.sh`, `provision.ps1` and both `clean` scripts use, with
`provision.sh`'s rule (1-32 of `a-z0-9-`, no edge dash); the CLI's global
`--instance` (env `ITSANAS_INSTANCE`) resolves through it and refuses a
`--home`/`ITSANAS_HOME` that disagrees, as `provision.sh` does. Found while
checking the scripts: `~/.itsanas-passphrase` is the default node's passphrase
**file**, so `passphrase` is refused as a name in the CLI -- the shell scripts
still accept it, and `clean.sh`'s `"$HOME"/.itsanas-*` glob (~173) and
`clean.ps1` (~125) match that file; not fixed, part of the second half. Two
tests, three sabotages (validation, mapping, reserved name), three red. No
Rodin: half a step, and the rule is once per major step.

**2026-09-29, item 4 of §8 0o 2b judged moot, not built.** The hourly read
stays unconditional, and 0o phase 2 is done. Why: the read only happens on
the hourly publication's connection (`Contact::due`: `read = publish ||
read_at.is_none()`), so skipping it saves one request on an open connection,
not a connection. The rule it would need ("read when a book device went
unreached") cannot see a machine enrolled after this one started: it is on
no list, and a relay never introduces a machine (step c). So a home fleet
where everyone answers would never learn a new laptop. Still open from
step c, now in `ROADMAP.md`: the 2099-clock presence that blinds the relay
for one device. Phase 3 only if phases 1 and 2 fall short on the fleet, which
nobody has measured yet. Next is 0p. Step c merged as #187.

**2026-09-29, peers of one account relay where the others are** (§8 0o
2b.3 step c, branch `ccr-c9f4329d-v9cnbp`). Peer protocol 6
(`PROTOCOL_VERSION = 6`, `PROTOCOL_WITH_PRESENCES`) appends
`Request::Presences` (wire 13, carries nothing) and `Response::Presences(Vec<Vec<u8>>)`
(wire 10), each row a postcard `ClaimedPresence` -- bytes, because
`itsanas-net` must not depend on `itsanas-coord` (redb, rustls). The service
answers through the `Relay` trait (`PeerService::with_relay`); the daemon's
`SharedBoard` (`contact.rs`) is refreshed from the book after load and after
each round, and answers only a caller the book holds with a claim.
`Contact::relayed(rows, owner, me)` keeps a row only if `verify_for(owner)`
passes, the device is already in the book, and neither its claim nor its
presence is older than the book's. **A relay never introduces a machine** --
that is what refuses a withdrawn device's replayed old claim: the
coordinator's read has already dropped it. The daemon asks only in
`dial_listed` (`sync_once`'s `ask_presences`), after the round.

Rodin: `sync_once` first asked every pinned peer, LAN strangers included,
and threw their answers away -- fixed with `ask_presences`. Named, not fixed:
a presence signed on a clock far ahead (2099) that arrives by relay becomes
"latest" and blinds the relay for that device (the coordinator's read still
works, it has no date filter); a relayed address is dialled from the next
round, not this one (`candidates()` is a snapshot). **Not done, 🟨:** no test
that a v5 peer is not asked; the daemon wiring has no test of its own. Seven
red-team/functional tests in `contact.rs`, three in `service.rs`; six
sabotages, six red. Trap: the container's clippy (1.94) flags a pre-existing
`doc_markdown` in `main.rs:2554` that CI's stable does not; ran it with that
lint allowed.

**2026-09-29, the address book keeps each claim** (§8 0o 2b.3 steps a-b,
branch `step/8.0o-book-claims`). `Candidate` in
`crates/itsanas-node/src/contact.rs` has `claim: Option<SignedClaim>`, filled
by `Contact::read` from `Contacted::claimed` (new fourth argument), written in
the book file, re-checked on load by `ClaimedPresence::verify_for` against
`Contact::load(path, owner)` -- the daemon passes `node.store.owner()`. A
failed claim is dropped and its address kept, dialled, never relayed.
`Contact::relayable()` returns the `ClaimedPresence`s the book holds. The book
file is version 2; a version-1 file is read as version 2 without claims, so
the upgrade keeps `signs` (else the downgrade re-opens once).

Rodin: `insert` kept an address's *first* signature for ever, so the presence
it would relay aged while the machine re-published hourly, and a receiver
could not tell it from a replay. Now the latest `at_unix` of that device wins.
Named, not fixed: a device withdrawn by its owner stays relayable here until
the next coordinator read drops it (at most an hour, or the first round after
a start) -- the receiver's check in step c is what must refuse it. Four
red-team/functional tests added, five sabotages, five red.

**2026-09-29, the coordinator's list says whose each machine is** (§8 0o
phase 2b.3, first half, branch `step/8.0o-claimed-peers`). Found in the tree
uncommitted, left by an interrupted session; checked here as new work, five
sabotages re-run, five red. `ClaimedPresence { presence, claim }`
(`crates/itsanas-coord/src/claim.rs`) and `verify_for(owner)`: both
signatures, claim names `owner` and the presence's device, not revoked, no
date. `Request::ClaimedPeers` / `Response::ClaimedPeers` appended (wire 14 and
11); `CoordService::claimed_peers_of` builds the list and `peers_of` strips it.
The client (`coordinator::located`) asks `ClaimedPeers` first, keeps what
`verified_claimed` keeps into `Located::claimed` / `Contacted::claimed`, and on
a hang-up asks `SignedPeers` with `claimed` empty.

Rodin: ARCHITECTURE §6.1 said a coordinator *cannot* list another account's
machines as yours; it can, by hanging up on `ClaimedPeers` -- the signed
fallback checks where, not whose. Prose corrected, not closed: the cost is a
connect timeout per row, bounded by the book, never relayed (same call as the
2b.1 fallback). **Not done, 🟨:** nothing stores `claimed` yet -- the daemon
drops it, so the book has nothing relayable; and a relay could replay a
device's *old* unrevoked claim after the owner withdrew it (§8 2b.3 says what
to test).

**2026-09-29, the address book is kept on disk** (§8 0o phase 2b.2, branch
`step/8.0o-address-book`, PR #184). `Contact::load` / `Contact::save`
(`crates/itsanas-node/src/contact.rs`) keep `{ version, signs, entries }` in
`<home>/address-book`, postcard, written to `address-book.tmp`, synced, renamed
over. The daemon loads it before its loop and saves after each round in which
`read`, `worked` or `signed` changed something. Load goes through `insert`, the
same door as the wire, after `verify_origin`: bounds and signatures apply to
the file. A damaged or foreign-version file is an empty book and a log line.
Only signed addresses are written; an unsigned fallback address is dialled and
forgotten at exit. `Located` and `Contacted` now carry `presences` (the checked
`SignedPresence`s) beside `found`; `coordinator::verified_presences` is the
check both use. Success times are unix seconds of this machine's clock.

Rodin, before the commit: a Pi with no real-time clock reads 1970 until NTP,
so a success recorded then ranked below yesterday's stale address -- one
connect timeout per round. `Contact::worked` now records a success as later
than every success in the book. Also named in `load`'s doc, not fixed: anyone
with this user's rights can set `signs` false or mark a stale genuine address
as worked; they can rewrite the configuration too. Three red-team tests, three
sabotages, three red.

**Not done, 🟨:** the daemon's load/save has no test of its own (it is in the
loop `acceptance-local.sh` drives); the call to `Contact::signed` still has
none either; one-off commands still accept an unsigned list silently. Traps
this time: Python `write_text` on Windows wrote CRLF into three `.rs` files --
use `write_bytes`; a Git Bash heredoc turned `\` in a Rust string
continuation into a literal `
` twice -- use the Edit tool for those.

**2026-09-29, the coordinator's list is signed, and checked** (§8 0o phase
2b.1, branch `step/8.0o-signed-peers`; 2a merged first as #182 after its five
sabotages were re-run -- the session that wrote it had stopped with the PR green
and open). `Request::SignedPeers` / `Response::SignedPeers` are appended to the
coordinator protocol (wire numbers 13 and 10); `CoordService::peers_of` keeps
the signed rows and `Peers` strips them. `SignedPresence::verify_origin` checks
the signer and the address, **not the date**: the reader's clock is no
reference, and a Pi booted in 1970 would refuse everything. The client reads
through one function, `coordinator::located`, which keeps what `verified` keeps
and counts the rest (`Contacted::forged`, logged by the daemon); `contact`,
`devices_as` and `find_member` all go through it.

Rodin, before the commit: the fallback to `Peers` for an older coordinator is a
door a hostile one opens by hanging up, so the signature was advice, and three
sentences (the `SignedPeers` doc, ARCHITECTURE §6.1, a test's name) said
otherwise. Now `Due::accept_unsigned` is false once `Contact::signed` has been
called -- the daemon calls it after every signed read -- and a hang-up is then a
failed read; a read that did fall back says `signed: false` and the daemon logs
it. Tested on a real coordinator told to hang up (`Directory::play_old`) and on
one holding a planted forgery (`Directory::plant_presence`), both behind the
`hostile` feature of `itsanas-coord`, which only `itsanas-node`'s
dev-dependencies turn on. Four red-team tests, five sabotages, five red.

**Not done, 🟨:** the memory that this coordinator signs lives in the process,
so every daemon start re-opens the downgrade until its first read -- 2b.2 keeps
it with the book; the daemon's call to `Contact::signed` has no test (it is in
`one_round`, which only `acceptance-local.sh` runs); one-off commands
(`device list`, `find`) accept an unsigned list and do not say so; and **a
presence proves where a device is, not whose it is** -- a coordinator can list
another account's genuine machines under yours. Harmless today (`sync_once`
decides trust on the connection, the book is bounded), and it must be closed
before 2b.3 relays anything: §8 says how.

Decided without asking, say so if wrong: the fallback stays rather than going,
because removing it stops every node reading until the VM is upgraded, and the
VM is upgraded after the machines.

Traps this time: `cargo test --lib --test X` stops at the first failing binary,
so `sabotage.py` saw only the unit test go red until given `--no-fail-fast`; a
presence planted at the test's fixed `NOW` had expired by the server's real
clock, was left out for that reason, and the first version of the red-team test
passed without checking a signature -- the `forged` count caught it; clippy's
100-line limit on `CoordService::handle` and on the wire-number fixture (the
repo has no `allow` for it: split instead); a Git Bash heredoc holding a Python
script with `'''` did not parse -- Write tool.

**2026-09-29, the coordinator is dialled only when something is owed** (§8 0o
phase 2a, branch `step/8.0o-contact-when-needed`; 0n merged first as #181
after its ten sabotages were re-run). `itsanas_node::contact::Contact` decides
per round: publish at start, when `coordinator::address_now` -- a UDP
`connect` that sends nothing -- finds a different address, and hourly; read
the account's devices on every connection a publication opens and never
otherwise. `coordinator::contact` does both on one connection and refreshes
the pledges of 0n there too, so the daemon's separate hourly `Devices` dial is
gone. The round dials the coordinator first when something is owed, then
configured peers, then the book (the coordinator's last list, each device's
addresses ordered by *this machine's* record of which one worked), then the
LAN. A Pi that never moves and never restarts: 24 connections a day, was 288;
every start still publishes and reads, as before.

Rodin, before the commit: the coordinator was dialled *last*, a leftover of a
rule that needed to know who the round reached -- the start publication waited
behind every sync of the first round; and a publication accepted before a
failed read was forgotten with it and repeated every round. Both fixed
(`Contacted::read_failed`), **neither with a test of its own, 🟨**: no harness
has a coordinator that accepts `Announce` and refuses `Peers`, and the order
lives in `one_round`, which only `acceptance-local.sh` runs. Documented, not
fixed: "hourly" is an hour of
`Instant`, which does not advance during suspend on Linux and macOS.

Decided without asking, say so if wrong: **reads ride on every publication**,
where §8 said "a fleet at home reads never" -- a machine enrolled after this
one started is on no list, so "read when a listed device goes missing" would
never dial it; the read costs no connection. The book lives in memory (a
restart publishes and reads anyway); keeping it on disk is 2b.
`MAX_ADDRESSES` is 4, because every dead address costs a round a connect
timeout. Probe and connection are compared probe-to-probe, never
probe-to-published, because a dual-stack name can give them different families
and that would publish every round. **Not done, 🟨:** everything that needs the
signature -- gossip, relayed presences, dropping the hourly read (§8 0o 2b).

Traps this time: `sabotage.py` with `cargo test -q` reports every sabotage as
"the build itself refused it", because `-q` prints dots instead of the
`test ... FAILED` lines it parses -- never pass `-q` to it; `sed` on the JSON
spec did not remove `"-q"` because `json.dump(indent=1)` puts it on its own
line; a long Python heredoc failed to parse in Git Bash -- write the script to
the scratchpad and run it.

**2026-09-28, a file that will not fit is refused before it is copied** (§8
0n, branch `step/8.0n-write-budget`). The account's size was counted nowhere:
`Index` now keeps the sum of `FILES` in memory, adjusted inside its three
writers, and `Node::bound_writes` adds what only the vault knows (files other
devices hold) through `catalogue`. The store is handed a `WriteBudget`: what
**the account's** pledges earn (`Node::allowed_for` over
`Node::account_pledge`) -- this machine's pledge now, plus the other machines'
as `Request::Devices` last listed them, cached in `<home>/others-pledged` and
refreshed by `put` and hourly by the daemon. `write_stream`
refuses before storing the chunk that crosses and removes the blobs it created;
`put`, the folder import and the JNI `put` ask `check_room` with the file's size
first. Opening a node sets the bound with nothing counted elsewhere, so a new
caller cannot forget it; the daemon refreshes it before every folder pass.

**The spec was wrong and Nicolas chose the fix.** 1b said "exactly the rule
`keep` applies", i.e. this machine's pledge. Rodin found that the bound compares
the *account's* bytes to *one machine's* pledge, while `accounting::assess` sums
every device: with the default pledge of 0, a laptop writing for a Pi that lends
a terabyte would have refused everything past 10 GiB. Asked on 2026-09-29, he
chose the account's sum. `keep` still reads this machine's pledge, because its
question is what this machine holds.

Decided without asking, say so if wrong: the joining allowance applies whatever
the account's age, as for `keep` (the node does not know when it joined); sizes
are logical, not deduplicated; a machine that never reached a coordinator counts
the others' pledges as zero. **Not done, 🟨:** the disk half of 1b (a write
eating into room pledged to others); the native quotas of 0n (research only); a
daemon picks up its own new pledge only on restart; the phone never refreshes
the others' pledges (JNI has no coordinator call on that path); an account over
its bound cannot take a remote edit that conflicts with a local one, because
`keep_both` imports the local copy first and that import is refused -- nothing
is lost, the remote version waits; and the refusal says "at least" even when
`check_room` knows the total exactly.

Traps this time: `release_file` **removes** the path from `FILES` -- the index is
what is here, not the account, which is why the vault walk is needed; and the
two `saturating_sub(dropped_size)` lines are identical, so `sabotage.py` refuses
the anchor -- sabotaged by hand, both at once.

**2026-09-28, a machine can leave politely** (§8 0m parts 2 and 3, branch
`step/8.0m-leave`). Peer protocol 5 appends `Request::Leaving`, which carries
nothing: the device withdrawn is the one the connection proved, so the owner's
node runs `forget_device(caller)` and the next round repairs at once instead of
after `LIVE_FOR`. A v4 peer is not told (`PROTOCOL_WITH_LEAVING`; negotiation is
`min` of both sides, checked). The coordinator appends `Request::Depart`, a
`SignedDeparture` under its own domain, refused unless its device is the caller
and kept in a `departures` table apart from presence -- **nothing reads it
yet**, on purpose. `itsanas leave` tells configured peers, this account's
devices and the owners of what the vault holds, then the coordinator.

What it does **not** do, found by Rodin and left 🟨: it runs with the daemon
stopped, so a service manager that restarts the daemon brings the machine back
and the recorded departure lies about a live node -- the soft stop on the
service (systemd `ExecStop`, the Windows task) is not wired. Peers known only
from LAN beacons are not in any list `leave` can read; it now says "no peer was
told" instead of "can be switched off" when that happens.

Traps this time: a Git Bash heredoc ate `\n` inside a Python edit script
(write scripts with the Write tool); the coord test helper `enrol` registers
one account per device, so two devices need two owners.

**2026-09-21, a dead machine stopped counting as a copy** (§8 0m part 1).
`Store::under_replicated` -- the query repair drains -- counted **every holder
record whatever its age**. A machine that died six months ago still counted as
one of your three copies, so repair never fired; the only thing that withdraws
its records is a failed audit, and an audit needs that machine to answer. The
ledger was optimistic in the one direction that loses data, and ROADMAP had said
so in prose without anything acting on it.

`holders::LIVE_FOR = REFRESH_AFTER * 2` is seven days: a record is refreshed
every three and a half whenever that peer answers an audit or confirms during a
push, so past this it has missed two consecutive opportunities. Shorter would
re-replicate a fleet of machines that are usually off every quiet weekend;
`CONFIRMED_FOR` itself is too generous, because "is this record worth anything"
and "must I make another copy" are different questions and only the second costs
copies when it is wrong.

**2026-09-21, MVP tests D and E passed on the fleet, which neither had ever
done.** Nicolas asked for a throwaway node that recovers files from somewhere
else. The run, end to end and all of it through `ngas.fr`:

1. A throwaway account `evasion` was created on the **VM**, enrolled with an
   invitation minted by the Pi, and lodged its sealed container with
   `register --recovery`.
2. Two files -- 88 B and 400 000 B -- were written and pushed to the **Pi**,
   which belongs to the account `nicolas` and holds them blind.
3. The VM's node was left switched off. It never ran again.
4. On the **laptop**, a node with no data, no configuration and **no recovery
   phrase** ran `itsanas login --username evasion --from ngas.fr:9898` with the
   passphrase alone. It restored the same account id `942dd0e3…`.
5. One `sync` against the Pi returned **both files, byte for byte**:
   `149ee0377dc7ca76…` and `b1e5c75c05411b5c…`, the digests taken on the VM
   before sending.

That is **D** -- recovery by passphrase alone, which MVP.md said was built and
had never been run -- and **E** at the same time, because the machine that wrote
the files and the machine that read them never spoke: everything went through a
host of another account that cannot read a byte of what it relayed. What D still
lacks is nothing; what E still lacks is a real power cut rather than a process
that was never started.

Cleaned up afterwards: escrow withdrawn, both devices withdrawn, both homes and
both folders deleted, the invitation code removed. **What remains on purpose**:
the account name stays in the directory (a username is bound to a key for life),
and the Pi still holds the chunks in its vault -- the network never deletes as a
sanction, so they leave when the audit and the collector get to them. The
directory is back to 6 enrolled devices.

**2026-09-21, a decision that was not one.** Nicolas asked Rodin to settle the
availability question that had been blocking §8 0o phase 2 for three exchanges.
There was nothing to settle: `Directory::tick`, `contributions` and
`accounting::assess` are called only by their own tests, and `ECONOMICS.md` had
already removed availability from the coordinator on purpose. **The authority
for that is the written decision, not the absent caller** -- a `grep` cannot
tell a decision from an oversight, and treating "nothing calls it" as licence is
how the next reader concludes the opposite on finding one caller.

Rodin then found the thing that mattered, and it was in the specification
written the day before: phase 2's rule said the coordinator is dialled when a
round "reached no peer". In a parc of machines that are usually off, the Pi
wakes, finds the VM by LAN broadcast, has therefore reached a peer, and **never
publishes its address** -- so a laptop elsewhere asks the directory and finds
nothing. The rule confused **publishing** (what makes a machine findable by
somebody who is not there, and can never be conditioned on having met somebody
who is) with **reading** (what can be skipped). §8 0o now says so, with the
numbers that justify the work: 864 000 connections a day at 3000 machines
becomes under 72 000.

Two smaller things from the same audit. `tick`'s doc comment said "called on a
timer" when no timer has ever called it -- a present tense for something that
does not happen, which is the drift §4 exists to stop; the three functions now
carry the decision that keeps them unreached. And a gate to catch "public, and
called only by tests" was attempted and **removed the same hour**: the only
cheap way to ask counts call sites in each file's production half, and that
stops at the first `#[cfg(test)]`, so forty correctly wired functions were
reported. Why it failed is written in `check-wired.py`, so nobody retries it
naively.

**Where the fleet actually stands, 2026-09-18 evening.** Verified by asking
the machines, not by remembering:

| | runs | reachable from outside |
| --- | --- | --- |
| **VM** `itsworkstation` (192.168.1.11) | **the coordinator**, `[::]:9898` | **yes**, `ngas.fr:9898` |
| **Pi** `NGASRPI4B` (192.168.1.10) | a node, `*:9797`, account `nicolas` | **yes**, `ngas.fr:9797`, announced |
| | | *and proven: the laptop pushed 293 KiB to it through that name, and the Pi's vault holds chunks for `6ac44550…` -- the laptop's account -- with the file's name appearing nowhere in it* |
| **Laptop** `SIGSEG-DELL` | a node, account `sigseg42`, task `ITSaNAS` | no, and it announces nothing -- correct for a machine that moves |

All three run the day's build. The coordinator's directory holds **6 enrolled
devices**. `itsanas-coordinator.service` on the Pi is stopped **and disabled**,
which is deliberate: it is the migrated-from machine.

**Two things that are not done, and both are Nicolas's to decide rather than
mine to guess.** The fleet does not match `INSTALL-FLOTTE.md`: the accounts are
`nicolas` (Pi) and `sigseg42` (laptop), not the `sigseg`/`tester`/`mandarine`
instances the plan describes, and no node is a *named instance*. Reconciling
that means the full reset in that document, which needs him at the keyboard for
three sets of 24 words and a passphrase per instance. Until then MVP test O
cannot be run in the sense that matters -- **the two machines that are reachable
belong to different accounts**, so nothing of one account is hosted by a
reachable machine of the other.

**A usability defect found by running it, fixed the same evening.**
`itsanas doctor` opened the store, and the daemon holds it, so the command
somebody runs *because nothing is syncing* refused to run on a machine that is
syncing. Stopping the daemon to run it then made the inbound probe fail for the
wrong reason -- nothing is listening while it is stopped. `Identity::open` now
reads the keystore and the config and stops there, `network_report` works from
that, and `doctor` prints the network section **first**, always, then checks the
data only if the store is free and says so when it is not.

**2026-09-18, the fleet, and a folder that cannot lose your files**
(branch `storage-that-vanished`). Two things in one session, because Nicolas
asked for both: deploy everything, and finish the data-safety items.

**The fleet is deployed and the coordinator has moved.** The VM now runs it,
which MVP §2 has said since #53 and which had never been done. The state was
archived from the Pi and restored on the VM, so the **device id survived**
(`b92d7802...`) and not one node had to be re-pinned; the directory, the
accounts and the escrow came with it. `--admit-first`, which the Pi's unit had
carried since the day it was installed, is gone: a door that opens by itself
was harmless on a private port and is not on a public one. All three machines
run the day's build. Nicolas opened two forwards, `9898` to the VM and `9797`
to the Pi, and **`ngas.fr:9898` answers from outside** -- the directory is
reachable from any network for the first time.

Three things the deployment proved that no test could:

- The coordinator's first line on the VM was `devices 6 enrolled, and the index
  agrees`. That file was written before `CLAIMS_BY_OWNER` existed, so **the
  repair on open ran in production** and the counter said so.
- The listener reports `*:9797` and `[::]:9898`: the dual-stack socket, on real
  Linux.
- `doctor` on the Pi, against a coordinator still running the older binary,
  said `in unknown: this coordinator is too old to try reaching back` -- the
  compatibility path, across two genuinely different versions.

**A defect the deployment found, which no test had.** The probe budget was keyed
by device alone, so a node that had just been **repointed and given a new
announced address** could not ask whether the new one worked: the second
question read as a repeat of the first. That is the moment somebody most needs
the answer. The key is now device *and* address, and
`changing_the_announced_address_is_worth_asking_about_again` fails against the
old one.

**A second defect the deployment found, and it is the worst kind.** `doctor` on
the Pi -- a machine that was perfectly reachable -- printed `NOTHING can reach
this machine`, and then, on the next line, the reason: it had been asked twice
within the hour. The answer and the *absence* of an answer were the same value,
so a wait was presented as a verdict, and somebody reading the first line goes
and rewires a router that works. That is precisely the failure this whole
feature exists to prevent, built into the feature. `Response::Unknown` is
appended (wire number 9), `Reachability` is now three states, and every refusal
to probe -- the budget, a private address, a name resolving into a private
network, the concurrency bound -- comes back as "not checked" with its reason
rather than as "nothing can reach you".

**Then §8 0l, which is the one that protects data.** An unmounted disk leaves an
empty mount point; the scan found nothing, every file in the ledger looked
deleted, and those deletions replicated to every machine of the account. Built:
a `.itsanas-folder` marker carrying the device id, which vanishes with the
storage it sits on and is never synced; a pass that stops rather than deleting
when the marker is gone and the ledger's files are all absent; a refusal when
the marker names *another* device, which is how two nodes pointed at one
directory delete each other's files; a guard that **holds** deletions when a
pass would remove most of the folder, with `itsanas folder --confirm` as the
only way to apply them; and a node home that is an empty directory now says the
storage is probably not mounted instead of suggesting `itsanas init`, which
would have created a second account beside the real one.

Traps: a folder that predates markers is adopted, not refused, or an upgrade
would stop every existing member. The guard has a floor as well as a
proportion -- holding three deletions out of seven would teach its owner to
type `--confirm` out of habit, which is how a guard becomes a formality.

**2026-09-18, later the same day: the lookup stopped costing the whole
network** (branch `claims-by-account`). Nicolas asked whether anything else was
worth doing today. The answer was the ceiling measured a few hours earlier and
written into ROADMAP: `peers_of`, `devices_of` and `contributions` all walked
`live_claims()`, which deserialises **every claim in the directory** and then
filters by account, so one member's "where are my machines" cost O(devices in
the entire network) and the coordinator's work grew with the square of the
fleet.

Claims are now kept in a **second table keyed by account then device**
(`CLAIMS_BY_OWNER`), written in the same transaction as the claim, and a lookup
is one range scan over that account's own rows plus one point lookup each.
Measured on the laptop, before and after: 575 us -> 7.6 us at 500 devices,
**2.58 ms -> ~8 us at 3000**, and the curve went from linear in the size of the
whole network to flat. One of four runs reported 23 us, which is machine noise
and is recorded in ROADMAP rather than dropped.

Three things make a denormalised second copy defensible here, and each has a
test: **a device can never change owner** (`claim` refuses it outright), so an
index row is written once and never moves; both tables are written in **one
transaction**, so they cannot drift at runtime; and a file written before the
table existed is **repaired on open**, because reading it as "this account has
no devices" would tell every member their machines were gone and the only clue
would be that it started at an upgrade. §6 has the row.

**The Rodin audit found the one operation that breaks it, and it is one
Nicolas performs.** The repair was conditioned on "the index is empty", which is
right for an upgrade and wrong for a **downgrade**: an older binary enrols
devices by writing `CLAIMS` and knowing nothing of the index, so coming back up
the index is *stale* rather than empty, the repair skips, and every machine
enrolled during the downgrade is permanently invisible -- `claim_for` knows it,
it announces, and `peers_of` never returns it. The condition is now the two
tables holding a different number of rows, which covers empty and stale alike,
and `a_device_enrolled_by_an_older_binary_is_found_again_at_the_next_start`
fails against the old condition. The coordinator also prints the two counts at
every start, so the invariant is something its operator can see rather than
something a comment asserts.

Also from that audit, and written into the table's own doc comment: **this
index is safe because of three refusals that live in other functions** -- `claim`
refusing a second account, `claim` refusing to un-revoke a withdrawal, and
`register` refusing a username under a new key. Loosen any of them and this
table needs a delete path it does not have.

**The trap that cost the most time in this session was my own method.** The
sabotage verification restored each defence by replacing text, and sabotaging
one block made another anchor ambiguous; the half-restored file then looked like
a working one and three unrelated tests failed for a reason that was not in the
code. `directory.rs` had to be taken back from `main` and the work reapplied.
**Sabotage by copying the file back, never by editing it back** -- the script
that does it is `scratchpad/sab6.py`'s shape: keep a pristine copy, break one
thing, run, restore the copy, repeat. Each of the three defences was then
verified in isolation, and each turned exactly the test written for it red.

What is **not** fixed: the *number* of requests is still O(nodes x rounds),
because a round still dials the coordinator once. Removing that on a healthy
round is what phase 2 is for, and it still needs the availability decision
Nicolas has not made.

**2026-09-18, the client says what is wrong, and the centre is asked less**
(branch `says-what-is-wrong`). Nicolas asked for two things in one breath:
automate the connectivity question -- *"si sur une machine on ne peut pas sortir
vers la VM ou vice-versa, le client devrait le dire !"* -- and keep the
centralisation light enough that a thousand members and three thousand machines
never saturate it. His VM stays the fixed point, `ngas.fr` is durable, the Pi is
the backup.

Built:

- **`Request::CheckMe`**, appended (wire number 11; `Response::Reachable` is 8).
  The coordinator dials the address this device **announced** -- never one the
  caller names, which would be a port scanner with somebody else's address on
  it -- and completes a **device-authenticated handshake**, because an open port
  proves something is there and a handshake proves it is *you*. A forward
  pointing at the other Pi is an open port. Refused for an unenrolled or
  withdrawn device, a device that never announced, and any address that is not
  out on the internet. One probe per device per hour, four in flight at once,
  three seconds each.
- **The free half.** `itsanas_net::transport::Witness` counts accepted
  connections by whether the source address was public or private. A connection
  from outside *is* proof the way in works, and it arrived anyway.
- **`itsanas doctor` answers out / peers / in**, and names what to look at: a
  name that does not resolve is DNS, a refused connection is a port, a timeout
  is usually a firewall.
- **One coordinator connection per round instead of two**
  (`coordinator::announce_and_peers`): 576 per node per day became 288.
- **`DESIGN.md` §8 gains a sixth central job**, with the argument for why it is
  central and a row in the decision table.

**Three things this session got wrong and fixed. Read these before the next
change, because two of them were written the same afternoon they were found:**

1. **The concurrency guard underflowed.** The in-flight counter was taken with
   `then_some`, which evaluates its argument eagerly, so the guard was built and
   dropped even when no slot was taken; the counter went below zero and the next
   probe panicked on the increment. Caught by the test written for the opposite
   leak.
2. **The anti-scanner guard did not guard.** It checked the announced *string*,
   where a name is never private -- correct for ordering, useless here. The
   probe then resolved the name and dialled it, so `nas.example.org` pointing at
   `192.168.1.10` walked straight through. Found by the Rodin audit; the
   sabotage run that proved it **reached the real ITSaNAS daemon on this
   laptop**. The check now happens on the resolved addresses, in
   `resolve_probe_target`, before any socket exists.
3. **The load claim was aimed at the wrong number.** Halving connections per
   round is true and nearly irrelevant. `peers_of` walks `live_claims()`, which
   deserialises every claim in the directory: one lookup is O(devices in the
   whole network), so total work grows with the square of the fleet. Measured
   rather than argued (`measure_what_one_lookup_costs_across_a_fleet`,
   `#[ignore]`d): 5.2 us at 0 devices, 575 us at 500, 1.51 ms at 1500, **2.58 ms
   at 3000** on this laptop. A 3000-machine fleet asks about 10 lookups a
   second -- 2.6 % of a core here, perhaps a quarter of one on the VM's slower
   ARM core. **The audit called it the wall; the measurement says it is not,
   yet.** The numbers and the fix are in ROADMAP "Known ceilings".

Verified: every gate in `check-all.sh`, `cargo test --workspace`, and each new
defence sabotage-verified separately -- the private-address guard, the enrolment
check, the handshake pinning, the in-flight bound, and the resolved-address
guard. **Nothing has met a real network**: loopback and CI only. The three
`acceptance-local.sh` failures on this laptop are the documented UDP 21037 /
error 10013 trap.

**2026-09-18, reaching the account from outside the house** (branch
`outside-the-lan`). Nicolas asked, in this order: where do we stand on outbound
connectivity, does it work at a friend's house, then **put it on the MVP's path
because four machines in one house cannot produce a decent testing run**, and
then **prefer a decentralised answer** -- the VM may be a backup and a
bookkeeper, but clients should call each other, and nothing whose cost scales
per machine may become a dependency -- and finally, while this branch was being
written, **that the VM be contacted only when necessary**, for example when no
machine of an account is connected. That last one is not built here: it is
specified as §8 0o phase 2, which is `NEXT`, with the central/decentralised
split written out and the one decision it needs from him named. What is worth
knowing meanwhile is the size of the thing he is reacting to: every node dials
the coordinator **twice per round, unconditionally**, which is 576 connections
per node per day at the default interval, including three machines sitting on
one LAN that found each other by broadcast.

The answer to the question, established before any code: **no.** Every node has
`coordinator = 192.168.1.10:9898` and `peer = 192.168.1.11:9801`, both private.
Away from home a round reaches nothing, the daemon keeps scanning and versioning
locally, and everything flushes on return. Two measurements worth keeping, both
made from the laptop on the home LAN through the public name: `ngas.fr:22010`
and `:22011` answer, so **the Freebox hairpins its forwards** -- one name works
from both sides, and the "hairpin NAT" risk in ROADMAP's adversarial sweep is
one sample of evidence lighter. `9898` and `9797` answer nothing: **no port
reaches ITSaNAS from outside today**, which is the real blocker and is Nicolas's
to open. `tailscale status` shows a tailnet on the laptop; the Pi and the VM are
not on it, and after his second message it stays a deployment option, never a
dependency.

Built here, which is §8 0o **phase 1**, and no wire change:

- **`announce = host:port`** in the configuration, `itsanas announce [--forget]`,
  `provision.sh --announce`, `provision.ps1 -Announce`, and a line in `status`.
  It is published verbatim, **port included**, because a forward maps an outside
  port to a different inside one. Refused when it cannot be dialled from
  anywhere: unspecified, loopback, `localhost`, no port, port 0.
- **A wildcard `listen` now takes a dual-stack socket** (`itsanas_tls::reach`),
  so a node accepts IPv6 as well as IPv4 -- IPv6 being the one route between two
  houses that costs nothing per machine. It falls back to the IPv4 socket where
  the kernel has no IPv6, because refusing to start there would be a regression
  for that machine's owner. The coordinator's listener takes the same path.
- **A name is dialled at every address it resolves to**, not the first. A
  dual-stack name whose IPv6 route is blocked -- a friend's wifi, a hotel -- read
  as the peer being down.
- **Five seconds to connect instead of thirty.** At a friend's house a laptop is
  handed its account's addresses, most of them on a network it has left; at
  `IO_TIMEOUT` each those dead dials ate two minutes of a five-minute round.
- **The addresses that can work are dialled first.** `coordinator::peers` sorts
  public names and addresses before private ones, by the *receiver's* judgement
  and never by a claim in the presence -- the same rule that keeps a peer's
  clock out of ordering (§6). Sorting first buys an attacker nothing: dialling
  is pinned to the device id.
- **MVP test O**, `scripts/acceptance.sh O away|check`, its preparation in
  `BRIEFING-MVP.md` §2.5 in French, and machine 5 (Mandarine's MacBook Air, in
  another house) in MVP §2. **The exit criterion is now A-M *and* O** --
  Nicolas's instruction, and the one decision-level change in this PR.

Traps from this session, both of which cost real time:

- **A quoted heredoc through the Bash tool still eats one backslash.** Not only
  in Rust string continuations: it silently turned `printf '%s\n'` into a
  literal newline inside a shell script, and put runs of spaces inside three
  Rust messages. `scripts/check-messages.py` caught the Rust ones; nothing
  catches the shell ones. Write edit scripts with the file-writing tool, or use
  `concat!`.
- **A unit test of an ordering function proves nothing about the code that was
  supposed to call it.** Removing `reachable_first`'s call site left all five
  unit tests green. `tests/away_from_home.rs` -- a real coordinator, three real
  nodes -- is what fails, and it was written because the first sabotage pass
  came back green.

What is verified and what is not: every gate in `check-all.sh` is green here,
`cargo test` passes for the five crates touched, and each new defence was
sabotage-verified (both `reach.rs` defences, the announce validator, the
published address, and the ordering's call site separately). **Nothing has run
on a real network**: the dual-stack listener, the announced address and the
resolution fallback have only met loopback and CI. Three checks in
`acceptance-local.sh` fail **on this laptop only**, for the documented reason --
a daemon from an older build holds UDP 21037 and Windows answers 10013.

**2026-09-17, the listener, hardened for a public port** (branch
`harden-listener`). Nicolas asked, the same day, for five things: use the
network from outside the LAN with mobile machines that change networks; refuse
an upload that will not fit **before** copying it; survive a removed disk or a
dead machine (count live copies, re-replicate, and a graceful "I am leaving"
that asks for copies first); P2P coordination with the VM as a backup rather
than a hub, cautiously; named instances only. They are §8 0l–0p, in that order,
with what reading the code established. **He authorised merging green PRs
without asking** and does not want to be handed a merge.

Done in this PR, because nothing can be exposed before it: the node's listener
served **one connection at a time**, so one silent TCP connection every thirty
seconds made a node undialable for free. It is now a thread per connection
under `itsanas_tls::limits::ConnectionLimits` (32 total, 8 per IP, 4 per proven
device), a 15-second **total** handshake deadline (`accept_within`: a per-read
timeout let a caller trickle bytes for ever), and a storing lock in
`PeerService` so concurrent offers cannot overfill a pledge. The coordinator
takes the deadline and 16 per IP. The Rodin audit then found three more, fixed
in the same PR: a failed `accept` (a connection reset before it was taken)
**stopped the node's listener for good** while the daemon ran on without one --
it now retries, untested because the error cannot be provoked on demand; the
per-IP cap counted single IPv6 addresses, so a /64 was 2^64 callers -- IPv6 is
now counted by /64; and nothing proved either listener *applies* the deadline
(`with_handshake_deadline` exists so a test can). Eight red-team tests, each
sabotaged red. What is still open is in ROADMAP "What an adversarial sweep
found", including hairpin NAT making a whole house one address.

Trap that cost time: **on Windows, timeouts set on a `try_clone` of a socket do
not reach the original handle.** The first version restored the idle timeout
through a clone and cut authenticated peers off after 15 s;
`the_deadline_is_lifted_once_the_caller_has_authenticated` caught it. Trap for
the next agent: `vault.stats()` walks the vault, and every store now waits for
it under the lock.

**2026-09-17, a detour: re-running the coordinator setup, and where the
coordinator lives** (branch `coordinator-reinstall`). On the Freebox VM,
`sudo sh install/coordinator.sh --binary /usr/local/bin/itsanas-coordinator
--admit-first` died with `install: … are the same file` after Nicolas had
stopped the service, so the coordinator stayed down — and re-running it to
change a flag is what its own help says to do. The copy is now
`place_binary`, which leaves a binary that is already the destination (`-ef`)
alone and says so; with no `--binary`, the lookup ends at
`/usr/local/bin/itsanas-coordinator`, because sudo's PATH may lack it and never
has `~/.local/bin`. `check-installers.sh` cuts the function out and runs it
both ways; sabotaged both ways (guard removed: the exact VM error; guard always
true: "does not replace an installed binary"). **Not run on the VM**, and the
`-ef` path was checked under dash and bash only. Same PR:
`docs/BRIEFING-MVP.md` put the coordinator on the Pi, against `MVP.md` §2;
Nicolas chose the VM, the briefing and §1 now say so, and both say the three
accounts are still on the Pi's coordinator until migrated.

**2026-09-16, the session that changed the plan.** Nicolas said the thing this
file had been failing to hear: *"a chaque passe red-team il reste des soucis et
le MVP n'est toujours pas complet"*. Interrogating that produced four findings
and one decision, and they matter more than the step that was merged.

*The audit loop has no fixed point.* A red-team pass on ten thousand lines of
technical prose always finds something, so "no findings" cannot be the
condition for starting the fleet tests -- it is a condition that can never be
met. Worse, a measurable share of what each pass finds is **prose the previous
pass wrote**: this session's own findings included a stale `NEXT` line, a
200 MB/MiB drift, and a failure message naming neither limit. None is a product
defect. Twelve gates, a pointer with its own gate, counters in three files and a
tense discipline were each a sound answer to a real failure, and together they
mean every change costs N document edits, each of which is new surface for the
next pass. **The stopping rule is now written in §11 and it is A-M and O, not
quiescence.**

*Audits and use find different bugs.* The console window that Nicolas closed --
killing the daemon for a week -- was found by a person using the software, not
by six audit passes. Red-team finds stolen nodes, clock skew, lying peers. Use
finds unnotarised binaries, unreadable status, and windows people shut. The two
sets barely intersect, so more auditing does not buy down the risk of a real
test. `docs/MVP.md` now has **K, L and M** because of this.

*Test I was passing on the easy path.* `MVP.md` §2 says machine 4 is the only
publicly reachable component, and NAT traversal is not built. So 1, 2 and 3
reach each other **only on the LAN** -- and test I, run with everything at
home, proves that three machines on one LAN sync without a coordinator, which
is not the claim it makes. The fleet table now has a **dialable** column and I
is run twice, once with the laptop off the LAN. Found in five minutes of
discussing how to run the tests, by nobody auditing any code.

*What Nicolas asked for next, and it reorders §8.* Install and configuration
have to be genuinely simple for **two or more accounts per machine**, first for
him, then for a second person who will run the same tests on a Mac and an
Android phone. He also wants a **v0.2.0** to mark "concrete enough to test,
nowhere near v1.0.0". Both are in §8 item 0 as (i), (j) and (k), and the
running order is now **i, j, f, then c** -- nobody runs A-M until installing is
one command per account.

*A correction to `itsanas status`, specified in (j).* `status` calls
`open(home)`, which resolves the passphrase **before** the store lock is
reached, so the snapshot fallback -- the whole point of which is to answer
while the daemon holds the index -- is unreachable exactly when it applies. On
this laptop, `itsanas status` cannot say whether the node is healthy without
the passphrase. The snapshot is a plaintext file in the node home, so the
prompt protects nothing a `cat` would not bypass; this is an ordering bug, not
a security boundary.

**Then (j), out of order and before (i), because it is four files and it
unblocks everything else.** `itsanas status` now answers on a running node
**without a passphrase**. `Index::is_locked` asks redb whether the index is
held (`DatabaseAlreadyOpen`) before any key is resolved; `snapshot_status`
takes a path and nothing else, so it *cannot* prompt. Run against this
laptop's live daemon it prints in full, and what it printed is test L's
material and worth carrying forward:

    3 copies       every chunk is on at least 3 other machines
    concentrated   one machine holds all 19 of your chunks. Sealed, but a whole set
    spreading      off: 3 machines hold anything of yours, and 9 are needed

745 tests, 53 red-team. Both new red-team tests were sabotage-verified: the
probe forced to answer "not locked" fails the store test with its own message,
and the age dropped from the header fails the CLI one. **`NEXT` stays (i)** —
(j) was done first because it is small and it is what makes "is my node
healthy?" answerable at all, which is half of what (i) is for.

**Then (i), first pass: three accounts created cold on this laptop**, in
throwaway homes, without touching the live node's scheduled task. The
machinery works -- the second and third accounts took 9798 and 9799 without
being asked anything -- and two things it *said* were wrong:

* **`init` told a new account to run `itsanas serve`.** `serve` serves peers
  and never syncs, so an account set up by following the printed advice hosts
  other people's data and never moves its own -- and the first thing `status`
  then says is `synced folder none`. It now prints the order somebody actually
  needs: `folder`, `pledge`, `daemon`, with `coordinator` + `register` before
  the daemon for joining an existing one.
* **The port line named one sibling when there were two.** The third account
  skipped 9797 *and* 9798 and was told "9797 is used by another node on this
  machine". Ports are handed out here without asking, so that line is the only
  place a person learns what happened, and counting instances from it counted
  wrong. It now says `9797-9799 are used by other nodes on this machine`.

**(i) is not finished** and `NEXT` stays on it. What was exercised is the CLI
path (`init` twice more on a machine that already had a node). What was **not**
exercised, deliberately, is `install/provision.ps1 -Instance` and
`provision.sh --instance`: they register scheduled tasks and systemd units and
kill processes, and the live daemon on this laptop holds real data -- #18 fixed
`provision.ps1` killing every `itsanas` process, which is exactly the failure a
careless run reproduces. Those want a scratch machine or Nicolas at the
keyboard, and `install/macos.sh` has still never been run by a human at all.

**All four landed**: #20 (H on Windows), #24 (the protocol, replacing #21),
#22 (status without a passphrase), #23 (onboarding). Then the bench grew the
tests it could not run.

**C, K and M are now automated.** `scripts/acceptance-local.sh` set up a
second account that *really hosts*: it pledges, takes 2.6 MiB of the first
account's chunks over a socket, and is then scanned for the canary. That is
test C as written -- the bench's own comment used to say it could only manage
the negative control, and C is the test whose failure stops the project. It
passes. M passes with it: the host's `ls` names none of the owner's files.

**K found something, and it was the protocol that was wrong.**
`itsanas sync` **never audits**: `session::audit` is called from the daemon
loop (`crates/itsanas-cli/src/daemon.rs` ~848) and nowhere else. The first
version of the K phase deleted the host's vault and ran three `sync` rounds,
and nothing happened -- which reads as "the sanction is broken" and is really
"the sanction was never asked to run". With the owner's daemon up it fires:
the daemon prints `FAILED n of m storage challenges` and `itsanas status`
grows `peers that have failed a storage challenge` naming the machine.
`MVP.md` and `BRIEFING-MVP.md` now say the daemon must be running, because
Nicolas would have lost a morning to this.

**Closed the same day, and the way it closed is the lesson.** The `placements`
count had not moved (81 -> 81) while the challenge plainly failed, and that was
written up as "the sanction may not work", with K's pass condition quietly
lowered from *the data is re-placed on another machine* to *the owner names the
peer*. `MVP.md` §4 says a criterion is set in advance **so it cannot be
softened afterwards**, and softening it is exactly what happened.

A Rodin pass caught it. The bench had **one host**, so there was nowhere to
re-place to; the flat count was the bench's fault, not the product's. With a
spare host pledging, `placements` goes **83 -> 125** -- it rises, because the
withdrawn copies are rewritten elsewhere -- and K passes on both halves. The
criterion is restored, and `acceptance-local.sh` now runs three hosts.

**The rule this earns:** when a measurement disagrees with a criterion,
suspect the measurement before editing the criterion.

**Test N was written from `grep` and two thirds of it was wrong.** Three
"this will bite" claims went into `MVP.md` and `ROADMAP.md` inferred from the
*absence of code*. Running them on the laptop refuted two:

* **Case-only pairs are handled well.** `Camera/IMG.JPG` and `Camera/img.jpg`
  both written to NTFS: the collision is caught, **both survive**
  (`img.local-<id>.jpg`), it is named in the output, and three further scans
  report `0 in, 0 out, 0 conflicts` -- it settles, and the conflict copy is not
  re-ingested. The mechanism written for two machines editing one file covers
  two names one filesystem cannot hold apart.
* **Long paths are fine.** 305 characters logical, over 400 absolute, written
  to NTFS without complaint.
* **Unicode normalisation: answered on a real Mac the same evening, and the
  prediction was wrong.** An M4 MacBook Air (APFS) was sent the two spellings
  and reported `FILES: 1` -- one file, holding the second write, under the
  first spelling's bytes. **APFS is normalisation-insensitive and
  normalisation-preserving**, so a Mac matches an existing accented name
  whatever its form and does *not* re-upload what it received from Linux,
  which was the whole basis of the warning. What is left is the case-collision
  case, already handled by the conflict machinery.

  That makes **three predictions written from the absence of code and three
  refuted by running them** -- case pairs, long paths, and now Unicode. The
  rule is cheaper to write down than to keep relearning: *absence of code is
  evidence about code, not about behaviour.* `itsanas` itself has still never
  run on a Mac; the filesystem question, which was the crux, is closed.

Absence of code is weak evidence about behaviour. This project's own tense
discipline says a present indicative needs a test behind it, and three of them
did not have one.

**Then the four things the audit had listed and nobody had fixed.**

* **The Mac bug was reproduced without a Mac**, which I had said was
  impossible. Storing both normalisation forms directly -- `Caf\u{e9}.txt` and
  `Cafe\u{301}.txt` -- gives two stored files, `scan` writes both to NTFS, and
  the round prints `out  Unicode/Café.txt` **twice** with `0 conflicts`. So
  the Unicode gap is now a **result**, not a prediction: a duplicate nobody can
  tell apart, no data lost, and a Mac joining the account would re-upload every
  accented file it received from Linux. Pinned by
  `the_two_unicode_spellings_of_one_name_are_two_paths_today`, which documents
  the current contract so that adding normalisation is a deliberate decision
  rather than a silent change that strands every existing accented path.
* **`status` now answers in every state.** The earlier fix only covered a
  *running* daemon. A stopped node with no terminal still got a lecture about
  environment variables -- which is the whole window between installing and
  starting the daemon, exactly when somebody asks whether it works. It now
  prints the last snapshot with `nothing is running this node`, and a node that
  has never finished a round says so and names `itsanas daemon`.
  `red_team_a_stopped_node_is_never_reported_as_a_running_one` keeps the two
  sentences apart; sabotage-verified.
* **The flaky security test is fixed, and it was genuinely flaky.**
  `the_phrase_is_not_written_anywhere_under_the_node_directory` searched for the
  phrase's **first word plus a space** -- as little as four ASCII bytes hunted
  through redb pages, which collides by chance. Proof: it failed in CI's
  coverage job and **passed on a re-run of the identical commit**, while 125
  local runs never reproduced it. It now uses three words (~33 bits) and plants
  a control first, the way `C scan` does. Sabotage-verified by making
  `Node::create` write the phrase to disk.
* **And then it was done.** Both kits now ask `itsanas status` for the account's
  file count on every sample and carry it in the verdict line -- `on an account
  of 4210 file(s)`, or `account size NOT recorded, so this says nothing about
  "with a large folder"`. Reported, never judged: the criterion says "large"
  without saying how large, and a number invented here would be the same
  substitution the CPU threshold already is. **It needs a binary from
  2026-09-16 or later**; every machine on the fleet ran a 2026-09-14 build when
  this was written, and those record `unknown`. Upgrade before starting the 24
  hours.

**Test I's off-LAN verdict was checked rather than assumed.** I had written a
pass condition for the second run -- "machine 1 says it is cut off and loses
nothing" -- without ever seeing the software meet it, in a test whose protocol
asks Nicolas to wait **48 hours**. Run against a node whose coordinator and
only peer were both unreachable, it does meet it, and the exact lines are now
in `MVP.md`. One correction fell out: **`I check` must not be run on machine 1
for that half**, because the phase looks for sync rounds *completing* after the
outage and a node that can reach nobody completes none -- it would report FAIL
for a machine behaving correctly.

**A second randomised security test was found flaking, the same way.**
`a_corrupted_recovery_phrase_is_rejected_not_silently_accepted` generated a
random phrase, swapped two words and asserted the checksum rejected it. A
24-word BIP-39 phrase carries 256 bits of entropy and an **8-bit** checksum, so
a transposition still validates roughly **1 time in 256** — and with five test
jobs a push that surfaces regularly, announcing that corrupted phrases are
silently accepted, which is the most alarming possible way to report a coin
landing tails. Caught on macOS during a pull request that changed **one
markdown file**. Now five checked-in seeds, so the test is identical on every
platform and every run.

That is two randomised security tests in one day. **If a security test draws
random input, work out its false-failure rate before trusting it**, because the
failure mode is not a wasted run — it is teaching everybody that the alarm is
noise.

**The fleet was upgraded from this session, over SSH.** Nicolas opened access
on 2026-09-16 and asked for as much as possible to be taken off his hands.

*How to reach it, because working it out again wastes a session:*
`ssh -i ~/.ssh/itsanas_session itsomeone@ngas.fr -p 22010` is the **Pi**
(`NGASRPI4B`, member node **and** the coordinator) and `-p 22011` is the
**Freebox VM** (`itsworkstation`). A bare `ssh` without `-i` is refused; the
project key is the one that works. Non-login shells have no `itsanas` on
`PATH` — it lives at `~/.local/bin/itsanas` — and `systemctl --user` needs
`XDG_RUNTIME_DIR=/run/user/$(id -u)` or it cannot find the bus.

*What runs where.* Pi: `itsanas.service` (user) plus `itsanas-coordinator.service`
(**system**, running as `itsanas-coord` from `/usr/local/bin`). VM:
`itsanas.service` *and* `itsanas-mandarine.service`, two accounts on one
machine — the multi-instance work of #18, already live. Accounts seen:
`nicolas` on the Pi, `voisin` on the VM's default home, `sigseg42` on the
laptop.

*What was done.* Both machines' source at `~/.local/src/itsanas` was 30 commits
behind at `5156cd6`; both now build `b9c497e` and run it — 4 m 36 s on the Pi,
5 m 10 s on the VM, natively, no cross-compiling. Old binaries kept as
`itsanas.bak-2026-09-16`. Every user unit came back active.

*The coordinators, done by Nicolas the same evening.* An agent cannot restart
them — system units owned by root, and `sudo` wants a password — so a freshly
built binary was staged at `~/itsanas-coordinator.new` and he installed it.
**Confirmed 2026-09-16 21:14 on both machines**: the running
`/usr/local/bin/itsanas-coordinator` is byte-identical to the staged build,
both units `active` with `NRestarts: 0`. That closes the "the Pi's coordinator
must be upgraded" note of 2026-09-15.

*The whole fleet verified after both upgrades.* Both member nodes wrote their
status snapshot **within a second** of being asked, so rounds are running, and
`itsanas status` answers on every machine with no passphrase. A quiet round
prints nothing, so an idle journal is health rather than silence — the VM
logged nothing for four hours while syncing perfectly.

*Do not use `~/upgrade.sh` on the VM.* It is stale: it bounces only
`itsanas-mandarine` and points at a source path that is not the one being
built. `scripts/` in this repo is the authority.

*Fleet health, read without a single passphrase* — which is the change that
made it possible. Both member nodes report `3 copies — every chunk is on at
least 3 other machines`; the Pi has 45 placements and hosts for 4 peers, the VM
21 and 5. Both also say `concentrated: one machine holds all 7 of your chunks`,
which is honest and expected at this size. The Linux H sampler was run on the
Pi and recorded `account holds 1 file(s)`, so the account-size column works on
real hardware and not only on the laptop.

**The bench had been testing a coordinator the fleet does not run.** It started
an *open* one; the Pi runs `--invite-only --admit-first`. So every acceptance
run to date exercised a configuration that does not exist here, and the one
path a new member actually walks -- being invited -- was covered only by unit
tests in `itsanas-coord::directory`. The bench now matches production and
drives the whole flow over a socket: an uninvited stranger is refused, a member
mints a code, the newcomer is admitted, that member re-registers **without** a
fresh code, and a single-use code cannot admit a second stranger. All five
pass. Found while asking what could go wrong with a visitor in the room, which
is a better question than "what shall I audit next".

**`itsanas` on Android cannot join, and M12 said the opposite of the truth.**
The ROADMAP row read "shell not written"; there is a 1354-line Kotlin app
committed 2026-09-07, and I repeated the stale row to Nicolas as fact. What is
actually missing is worse and more specific: `crates/itsanas-android` exposes
16 JNI calls that match `Native.kt` exactly, and **none of them is a
coordinator or a `register`**, and nothing there does local discovery. `login`
takes the 24 words, not a coordinator. So a phone reaches the network only
through `addPeer("ip:port")` typed by hand, and an account created on a phone
is enrolled nowhere. `docs/BRIEFING-MVP.md` §3.9 is the platform table to read
before inviting anybody.

**The phone can join now.** Nicolas asked for the Android app to be made to
work, and the blocker was one missing capability rather than a missing app.

The coordinator client was `crates/itsanas-cli/src/coordinator.rs` -- 557 lines
inside a **binary** crate, so nothing but the CLI could reach it. It moved to
`itsanas-node` as `pub mod coordinator`, which is where the CLI already
re-exports `config`, `error`, `keeping` and `node` from, so `main.rs` needed
one line and no call site changed. `itsanas-node` already depended on
`itsanas-coord` and `itsanas-coord` does not depend on it, so there is no
cycle.

On top of that, two JNI entry points -- `setCoordinator(address, device)` and
`register(invite)` -- and the Kotlin to match: `Native.kt`, a suspending
wrapper each in `Account.kt`, and a "Join a network" section in
`MainActivity.kt` that sets the coordinator and enrols in one tap, because a
coordinator configured but never registered with looks like joining and is not.
18 JNI calls now, and both sides still agree name for name.

A release APK builds: `bash scripts/build-apk.sh release`, 3 m 13 s, 21 MB.

**Not verified, and it matters:** nobody has installed this APK on a phone. The
join logic underneath is the same code the CLI runs and the bench now covers
end to end, but the Kotlin glue and the JNI marshalling are exercised by
nothing. Also still absent: **local discovery on Android**, so two phones on
one wifi do not find each other -- they go through the coordinator or through
`addPeer`.

**The APK is debug-signed** (`signingConfig = signingConfigs.getByName("debug")`
in `android/app/build.gradle.kts`). That is fine for sideloading and impossible
for Play, which rejects debug keys; it also means an upgrade across a key
change needs an uninstall. The release key stays §10.2, Nicolas's to hold.

**The repository is ready to publish; the account and the key are not, and
cannot be.** Nicolas asked for the Play internal testing track so that nobody
has to sideload. What an agent can do is done: `android/app/build.gradle.kts`
reads a release key from `android/keystore.properties` when one exists and
falls back to the debug key when it does not, saying which out loud on every
build -- because "release" otherwise means two different things and the
difference surfaces as a rejected upload. `.gitignore` refuses `*.jks` and that
properties file. `scripts/build-apk.sh bundle` produces the **`.aab`** Play
requires (verified: 1 m 7 s, 16 MB) and the APK path still produces the
installable thing.

What an agent must **not** do, and these are not obstacles to route around: a
Play developer account needs an account created, $25 paid and Google's terms
accepted; and the release key is the application's identity for ever -- Google
will not re-key a listing -- so it is generated and held by Nicolas and never
passes through a session. `docs/ANDROID-RELEASE.md` carries the `keytool`
command, the personal-versus-ITSomething comparison, and the console answers
(data safety, `dataSync` justification, export compliance) written down in
advance rather than from memory at eleven at night.

Two open items there that no amount of agent work removes: **there is no
privacy policy**, and Play will not take a listing without a URL for one; and
`versionCode` is still `1` and must rise before every upload.

**The Android application was run, and it joins.** Nicolas asked whether
anything was left before the manual test, and suggested another Rodin or
red-team pass. The stopping rule written this morning says an open-ended audit
is the thing that feeds itself -- so instead the untested thing was tested. The
`x86_64` ABI exists for exactly this and the comment in
`android/app/build.gradle.kts` says so: "the emulator, which is how this gets
tested without a phone in the room".

On the `itsanas-test` AVD (android-35), the application installs, starts,
**loads the native library** -- no `UnsatisfiedLinkError`, which is the failure
that matters and that no amount of Kotlin review would catch -- opens a real
node, and lists its files. The join was then driven through the UI against a
coordinator running `--invite-only --admit-first` on the host at `10.0.2.2`.

**The proof it worked is indirect and solid:** afterwards a fresh account
registering from the command line was *refused* for want of an invitation, so
the phone had consumed `--admit-first`. Nothing else could have shown that.

It also found a defect no review had. The join succeeded and **nothing on the
phone changed** -- no message, no state -- which is indistinguishable from a
button that does not work, and somebody would tap it again. The screen now
reports what happened, including the honest case where the device is a member
but no address could be published, so it does not claim to be reachable when it
is not.

The procedure is in `docs/ANDROID-RELEASE.md` §5b, with the Git Bash trap:
`MSYS_NO_PATHCONV=1`, or the shell rewrites `/sdcard/ui.xml` into a Windows
path and `adb` fails on a file name it invented.

What an emulator still cannot show, and it is worth saying before tomorrow: a
real radio, a real battery, Doze, and a manufacturer's idea of what a
background service may do.

Two things a next session should not re-derive. The host's vault is
`<home>/vault`; `<home>/store/blobs` is that node's **own** chunks and is
empty on a pure host, and checking the wrong one made a real host holding
2.6 MiB read as holding nothing. And on this Windows laptop the bench's three
**discovery** checks fail for environmental reasons -- they fail identically on
an unmodified tree, and pass on CI's ubuntu -- so run the bench for its
verdicts, not its exit code, when working from Windows.

Traps from this session, in the order they cost time. **A squash merge is
sometimes refused by a local policy guard** ("Merge Without Review") and
sometimes not; the same command failed early in the session and succeeded an
hour later. Treat a refusal as "ask Nicolas", not as "impossible", and retry
once before giving up. **Never `--delete-branch` while another PR is based on
that branch**: deleting `measure-h-windows` closed #21 where it stood, and a
closed PR whose base is gone can be neither reopened nor retargeted -- it has
to be recreated (#24) after `git rebase origin/main`, which correctly skips the
already-squashed commit. Merge a stack bottom-up, rebase each survivor onto
`main` first, and delete branches only at the end. A Git Bash heredoc also swallowed a whole
Python script this time, not merely its backslashes: write editing scripts to
the scratchpad with a file tool, never with `cat <<EOF`.

Traps from the previous session: a nextest filter `test(=name)` matches nothing for a
unit test (its name is `module::tests::name`) and a sabotage script reading
"no tests to run" as a failure reports red for the wrong reason — use
`test(~name)` and check the output names the test.

**2026-09-15, a detour Nicolas asked for: accounts and devices.** A hand
red-team of the identity surface, then fixes, in one PR (branch
`account-devices`). Found and fixed: a withdrawn device came back with
`itsanas register`, because every keystore holds the master secret and claims
were ordered by the signer's clock (the doc said the opposite); a withdrawal from
a machine with a slower clock was dropped while the CLI printed "withdrew";
`login --from` forgot its coordinator, so the `register` it suggested failed;
`device list` could not show a machine silent for a week. Added: final
withdrawals, `Request::Devices` (appended; an older coordinator makes the CLI
fall back and say so), `itsanas passphrase`, a pin on the coordinator's wire
numbers, and an accounts section in `acceptance-local.sh`, and `sync` with no address
now dials the account's devices from the coordinator (the Rodin audit found
that a restored machine otherwise had nothing to sync with). 735 tests (50
red-team). The seven Rust defences were sabotage-verified; the bench's account
checks were sabotaged against a broken binary separately (see the PR). The
Rodin audit also found that final withdrawal is a weapon for whoever holds the
master secret — written down in `claim.rs`, not fixed — and that
`coordinator::enrolled` read any transport error as "older coordinator" (fixed
the same day, below). **The Pi's coordinator must be
upgraded** for the full device list. Not fixed, written in ROADMAP "The
identity surface": a stolen node with its passphrase is the whole account, and
withdrawal does not reach the peer protocol. The manual fleet checklist in
French is `docs/BRIEFING-MVP.md`. Merged as #17.

**Then §8 0h, same day: two accounts on one machine** (branch
`multi-instance`). Discovery shares UDP 21037 (`SO_REUSEADDR` through `socket2`;
broadcast reaches every sharer, nothing is unicast); `init`/`login` give a node
the first port from 9797 that no sibling node home is configured for and the
kernel lets it bind; `provision.sh --instance NAME` / `provision.ps1 -Instance
NAME` give an instance its home, passphrase file and `itsanas@NAME` unit or
`ITSaNAS-NAME` task; `clean.sh`/`clean.ps1` take the same flag. Found on the
way and fixed: `provision.ps1` killed every `itsanas` process, which with two
nodes stops the other account; `clean.sh` never removed
`~/.config/itsanas/environment`, the passphrase `provision.sh` writes, and
looked for the macOS agent under a name `macos.sh` never used.
`docs/BRIEFING-MVP.md` is now the full protocol Nicolas asked for (MVP, 1a,
1b, scale measurements, what three machines cannot show against Storj).
738 tests. **Nicolas then specified the tray** (§8 0f, rewritten), which is
`NEXT`; its "decommission" needs a drain that does not exist.

Traps from 0h: **a daemon on an older build holds 21037 exclusively**, and on
Windows a sharing bind beside it fails with error 10013 ("access denied"), not
"address in use" — the local bench's two-accounts check fails on any machine
running such a daemon (it did on the laptop, PID of the ITSaNAS task). It
had **not yet run on CI** when this was written; the PR's `acceptance-local`
job is its first real run, so read that job before believing the scenario. Upgrade every node on a machine before adding an instance.
Python run from a Git Bash heredoc still loses backslashes even with a quoted
delimiter: write the script to the scratchpad. The Rodin audit of 0h found:
E in the protocol passed through `nicolas` on the Pi, which reads the file, so
it proved no blind relay (fixed: only `voisin` instances relay during E);
macOS needs `SO_REUSEPORT` to share a wildcard broadcast port (added, **not
run on a Mac** by hand; on the `macos-latest` runner the shared bind succeeds
and the broadcast send fails with "No route to host", so on macOS two nodes
sharing the port is verified and two nodes *hearing* each other is not); the protocol
told Nicolas to "update" the coordinator with a script that does not build
(fixed); nodes created before 0h keep whatever port they had.

**Then §8 0b and the audits' leftovers, on Nicolas's "fix what was raised and
complete the MVP"** (branch `refusals`; #18 is merged as `fb98fc2`). A push
counts refusals apart from "already held" and keeps the reason (`Offer`,
`Refusal` in `itsanas-net`; `PLEDGE_EXHAUSTED` shared by host and client), and
`sync` and the daemon print `refused N offer(s): …` even on a round that moved
nothing. Also: `device list` retries with `Peers` before blaming the
coordinator's version, so a dropped connection is reported as one; `itsanas
passphrase --recovery` re-seals the escrow container (refused while the
daemon holds the node, before anything changes); a daemon whose listen port is
taken names a free one and the commands to move. 741 tests, 51 red-team.
The Rodin audit of this step caught three things, fixed before the PR: the
"rejected" line told people to read a peer's log, and a peer logs nothing
about refusals; `passphrase --recovery` changed the keystore before trying the
coordinator, so an unenrolled device or an unreachable coordinator left the two
under different passphrases (the container is now re-sealed first); and the
refusal line repeated every round for a pledge-0 peer (now once, then once per
`OUTAGE_QUIET`). The bench now checks the printed line, not only the counter.
That left 0e then 0f: H on Windows was an MVP criterion the kit did not
measure, and the tray is polish Nicolas asked for that waits on one decision
of his (decommission: drain, or refuse while a hosted chunk has no other
holder). 0e is done, below; the tray is what `NEXT` names.
Not tested by anything automated: the "older coordinator" branch of `device
list` (no old coordinator exists to point it at).

**Then §8 0e: H on Windows** (`scripts/acceptance.ps1`). Run against the
laptop's own daemon before committing, which found two faults no parse
would: a one-row sample file reads back with no count in Windows PowerShell,
and a French locale writes `0,0 h`. The Rodin audit then found the
measurement did not answer the criterion as written, all fixed before the
PR: CPU was averaged over wall time including sleep, so on a laptop awake a
quarter of the day a daemon using 15% of a core read 4% and passed (gaps
longer than three sampling periods are now left out and awake hours
reported); `H sleep` passed on a machine that never slept (it now needs
Kernel-Power sleep entries, 42 or Modern Standby 506, in the window); a
`PASS  H report` read as H passed while it covers CPU and memory only (the
line and MVP.md now say so); two daemons were measured as one at random (a
sample now refuses); and samples record the binary's version. **NEXT is the tray, 0f**, and it waits on
one decision of Nicolas's: decommission by draining, or by refusing while a
hosted chunk has no other confirmed holder. With 0b, 0e and the audit fixes
merged, nothing in §8 item 0 was left for an agent before Nicolas ran the
fleet tests. That changed on 2026-09-16: see the session below, and (i), (j)
and (k), which now come first.

**State (2026-09-14).** v0.1.0 is tagged and released with the Android APK.
§8.1(a), the split as a value at 30/70, is merged as `f6cace7`
([#1](https://github.com/SigSegGit/itsanas/pull/1)). 727 tests (46 red-team,
3 `#[ignore]`d into the slow job), twelve gates.

**The plan changed after that merge, on Nicolas's request for "a valid MVP".**
Every acceptance test in `docs/MVP.md` §3 is built, and the verdict of §4 has
never been taken: E, F, G and I have never left the laboratory, J lacks its
power cut, H has seven hours of twenty-four, and D had only been done from the
24 words, which its own criterion forbids (the table said ✅; corrected). So §8
item 0 — run the MVP on the fleet — now comes before §8.1's enforcement,
which matters only once somebody other than Nicolas joins. What is left is
mostly hands on machines; the next step makes that cheap and unambiguous.

**§8 item 0a is built.** `scripts/acceptance.sh` (one command per test phase,
PASS or FAIL with the numbers, receipt in `~/.itsanas-receipts/acceptance.txt`)
and `scripts/acceptance-local.sh`, the CI job `acceptance-local`, which runs
B, D, E, F and G between three local nodes and points every check at a
situation it must refuse. Its first runs found three things: two checks that
printed FAIL and exited 0; F passing on a file that had never arrived; and a
host with pledge 0 refusing its own account's segments **silently** — `sync`
printed `sent 0 B, 0 segments`, as if there were nothing to send. That last one
was §8 0b, fixed on 2026-09-15. **It does not block Nicolas**: MVP.md's command table states the
pledge prerequisite, so the fleet runs (0c) can start now. The Rodin audit
caught the earlier wording, which put an agent task ahead of the fleet again.

**Later the same day the fleet itself was repaired** (#5, #6). The laptop's
daemon had logged `could not take on hosting (... framing: encoding: Hit the
end of buffer ...)` every round for a week: postcard numbers enum variants by
position, version 4 inserted `Response::ChunkSummary` mid-enum, and the Pi and
the VM ran week-old builds that `MIN_PROTOCOL_VERSION = 2` still admitted.
The floor is 4 and the numbers are pinned by a red-team test. All three
machines run `5156cd6` or later: the Pi and VM daemons, found running by hand
outside systemd, now run under their user units; the old binaries are kept as
`.bak-2026-09-0x`. The Windows logon task opened an untitled console that
Nicolas closed as a stray, killing the daemon; `provision.ps1` now uses
`conhost --headless` and restarts from the wrapper. **Still owed on the
laptop:** the existing task was created elevated, so switching its action to
conhost needs one admin command from Nicolas; the wrapper is already updated.
**Nicolas reordered the queue:** a tray icon (0f) before 0b.

What #1 carried beyond the split, each defence sabotage-verified:

- `Config::split` may only be **stricter** than `Split::DEFAULT`. Until §8.1(c)
  lands, `itsanas keep` is the only live enforcement of the bargain, and a
  generous split would have switched it off from a text editor. The session
  that wrote the field missed this; the Rodin audit found it.
- Refusals quote prices through `config::size_argument` (`73G`), rounded up.
  `format_size` floors to a tenth, so at 30/70 the quoted pledge was below
  the price, and the suggested `--pledge 93.0 GiB` had never parsed at all.
- `check-bargain.py` recomputes worked examples, not just ratios: four files
  stated 30/70 and still did 25/75 arithmetic beside it (90 GiB earns 30,
  `÷ 3`, 333 GB).
- `check-handover.py` checks the block above. The previous §0 said "ten gates,
  all green" while an eleventh existed and was red.

**Check a clean tree:** `bash scripts/check-all.sh` (twelve gates, discovered by
glob) then `cargo nextest run --workspace`. Both must be green before a push.

**Tests are run by CI, or by `bash scripts/receipt.sh` on the Raspberry Pi or
the Freebox VM — never by AI agents.** The script runs every gate and every
test and prints a ten-line receipt; Nicolas pastes the receipt. While editing,
run only the crate being changed (`cargo nextest run -p <crate>`).

**Keep conversations short.** One task per conversation; update §0 and §8
before the context grows, then start a new one. Cost is measured, not guessed:
one long conversation re-read its own context 1,885 times.

**Traps that have cost real time:**
- Git Bash heredocs eat backslashes and turn `\r` into a carriage return. Write
  edit scripts with the Write tool into the scratchpad, then `python script.py`.
- Every red-team fix is **sabotage-verified**: revert the fix, watch its test
  fail, restore. A test that passes both ways is decorative and not accepted.
- A new test needs a catalogue row in `docs/TESTING.md` and the counts updated in
  README, ROADMAP and TESTING — `check-counts.py` fails otherwise, and its
  uncatalogued ceiling (116) is a ratchet, not a target.
- A new `scripts/check-*` file must get a step in `ci.yml` — `check-ci.py`.
- **A CI warning is a failure.** The `no-warnings` job fails a run on any
  annotation above notice level. If it fires, fix the cause; allow-list only
  what describes GitHub's machine, never this project, and say why beside it.
- **The Windows CI timeouts were not Defender.** Its real-time protection is
  already off on the hosted runner (a step printed it). The heaviest debug
  tests spent their time in unoptimised dependencies, which are now built
  optimised, and CI lists every test over twenty seconds so the margin is
  read from the log, not guessed. Measuring on a 20-core laptop says nothing
  about a loaded hosted runner.
- **Windows test temp files live on a RAM disk in CI** (Nicolas chose this on
  2026-09-14 over relaxing durability in tests). What remained slow there was
  synchronous writes: 400 redb commits take 0.76 s on a laptop and 21-37 s on
  the runner. Tests keep the production write path; only the medium changed.
- **`cargo deny` runs in `check-all.sh` now; install it.** It failed CI twice in
  a week: a dependency pushed unvetted (2026-09-07), and an advisory published
  the same morning (2026-09-14, rustls, fixed by `cargo update -p rustls`).
  CI also checks daily.
- **The wire numbers enum variants by position.** Inserting a variant in
  the middle of `Request` or `Response` silently re-labels every later one
  for deployed peers: version 4 did it to `Response`, and the fleet logged
  `framing: encoding: Hit the end of buffer` every round for a week while
  `MIN_PROTOCOL_VERSION` still claimed version 2 was compatible. The floor
  is 4 now and a red-team test pins the numbers. Append, never insert.
- Two nodes on one machine could not both bind the discovery port (UDP 21037)
  until §8 0h: the second logged `local discovery is off (Address already in
  use)` and ran anyway. Seen on the VM, where `voisin` and `mandarine` share
  it. The port is shared now; a machine still running an older build keeps the
  old behaviour until upgraded. Nothing may ever reply unicast on it: a unicast
  datagram to a shared port reaches one socket of several.
- A daemon started by hand survives nothing and hides the unit's real state:
  on the Pi and the VM `systemctl --user` said "inactive" for a week while
  hand-started processes ran old binaries. Restart through the unit.
- A node that has pledged nothing refuses to relay even its own account's
  log. `sync` used to report that as `sent 0 B`; it now adds `refused N
  offer(s): its pledge is full or zero`. Every test node still pledges.
- `return` after a cleanup in bash hands back the cleanup's status: a check
  that printed FAIL exited 0. Put the verdict last.
- **Watch `main` after every merge, not only the PR.** `main` went red on
  Windows after #1 and nobody looked for four hours; PR checks said green.
- `cargo build -p itsanas-coord` builds the library. The server is the
  `itsanas-coordinator` crate, and a stale `target/debug` binary hid that
  for a whole session of local runs.
- A test that records two things in two calls and measures a window from
  one of them is measuring the clock. `a_holder_nobody_has_heard_from_...`
  did, failed on Windows whenever a second ticked, and was fixed in #4 by
  measuring from the earliest and latest; reproduce such a flake with a
  sleep before believing a fix.
- `format_size` is for reports. A figure somebody will type back is
  `size_argument`, or it floors below the price and does not parse.
- Grepping for a changed number finds the number, not its restatements.
  Search the phrasing and the arithmetic (`÷ 3`, "earns 333", "three
  pledged") — or better, extend the gate that recomputes them.
- **A crypto dependency is upgraded against a stored artefact, never against
  itself.** `red_team_a_keystore_sealed_by_an_older_build_still_opens` holds a
  keystore sealed by `argon2` 0.5.3; `argon2` 0.6.0 had to open it before it
  was accepted. Seal-then-open in one build proves nothing about old files.
- Unsafe code is allowed only in `crates/itsanas-drive/src/projfs.rs` (and the
  JNI export attribute), with a `SAFETY:` comment per block — `check-unsafe.py`.
- A PreToolUse hook (`~/.claude/hooks/quiet.py`) condenses builds and test runs
  to errors plus the summary and prints the full log path. Read that log rather
  than re-running. `QUIET=0 cmd` bypasses it.
