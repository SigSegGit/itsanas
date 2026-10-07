# Handover

Everything needed to pick this project up cold. Read this, then
[ROADMAP.md](ROADMAP.md) for status and [ECONOMICS.md](ECONOMICS.md) for the
contract.

---

## 0. Resume here after `/clear`

<!-- ITSANAS-STATE
NEXT: 8.0w
TITLE: sub-step (7): clean removal, by drain (decided, §10 item 15)
WRITTEN-AT: 2026-10-07
BASE: dc09537
-->

Read this section, then §8. Nothing else is needed to continue. The block
above names the next step and `scripts/check-handover.py` keeps it honest;
whether CI is green and whether a PR is open are facts for `git` and `gh`,
never for this file.

**2026-10-07 midday, #245 verified and merged (dc09537); three stale
sentences fixed in a follow-up PR.** Checked again by a fresh session: crate
tests green, eleven sabotages (each defence of `update.rs`, plus the
signature and downgrade checks in `itsanas-release`) each turned its test
red, all gates clean. The three AI reviews' BLOCKERs are answered on the PR
(TLS pinning: not the trust root, the signature is). Found and not fixed:
the scratch directory sits beside the exe, so with a `--prefix` the daemon's
account cannot write, even the `notify` check fails every day (logged, §9).
Stale until now: ROADMAP "Signed releases", TESTING's `itsanas-release`
header and `install/README.md` still said no node checked a signature.
0w (7)'s specification leaned on 0f (f) item 4, which left "drain or
refuse" to Nicolas; asked as §10 item 15 and **answered the same day:
drain** -- removal always finishes. Also decided: a planned skip is not a
red check (AGENTS.md, "Merging with planned skips").

**2026-10-07, 0w (6), the self-update, built (branch `step/self-update`,
PR #245, merged).** `itsanas update [--check]` and the daemon's daily, jittered look
under a new `updates` setting (`notify` default, `auto`, `off`; setup's
"Updates" step, Settings, `--answers`). Trust goes through `itsanas-release`
(signature, format, no downgrade, size + BLAKE3 + SHA-256); only a binary that
is its own version's released one updates itself; the program is replaced by
two renames with rollback; fetched with `curl` (no HTTP client in the
workspace). Tray icon on Windows: `itsanas.ico` with a state dot; the web page
shows `icon.png`. Seven red-team tests, each sabotaged red. **Not verified:**
never run against the real GitHub; no release key pinned yet, so every build
says so and installs nothing; the real rename of a running `itsanas.exe` and
the service restarts never exercised; the new tray icon never seen on screen.
NEXT: 0w (7), clean removal.

**2026-10-06 late night, a batch: 0w (2)-(5) and 0t parts 1-3, merged in
branch `step/wizard-tray-release`, one PR (#244).** Built by four agents in
worktrees, then integrated: the setup engine and `itsanas setup`
(`crates/itsanas-cli/src/setup/`), the local web wizard and Settings
(`setup/web/`), the timed pause and trays on Windows, macOS and Linux, and
`crates/itsanas-release` + `release.yml` + `sign-release.*` + `install/get.*`.
**Not verified:** no GUI path ever ran on screen (secret windows, trays,
browser launch), macOS and Linux paths never ran at all, no real service was
installed by `setup`, `release.yml` has never run. **Nicolas must generate his
release key** (`scripts/sign-release.cmd`, §10 item 14) and paste the public
key into `RELEASE_KEY`. Batch mode (few commits, one PR) is temporary; the
next step goes back to one step per PR. NEXT: 0w (6), the self-update.
Red-team pass on the batch found and fixed (each with a sabotage-verified
red_team test): sign-release signed a draft without checking its binaries
(now `itsanas-release check`), a Windows `#`-leading passphrase read as the
placeholder, join-from-coordinator skipping the service-file check, no total
deadline per web connection, get.ps1 with no rollback and flattening
REG_EXPAND_SZ PATH, `%`/newline in the tray's .desktop path. Key rotation was
documented as working; the docs now say it is not built.

**2026-10-06 night, NEXT is the setup wizard and the tray (§8 0w), by
Nicolas's redirection** ("the BIG step-up ... to get more testers before going
further"). #242 (fleet wiped, 0t planned) was found open and green, and merged
first. 0w is planned in eight sub-steps; (1), the control channel, is built
(branch `step/control-channel`): `itsanas pause|resume|sync-now|interval` write
`<home>/control`, which the daemon reads every two seconds at most; the Windows
tray has the matching entries. The architecture of the wizard (a local web page
served by `itsanas` itself) is the session's decision, recorded in 0w with why,
and **reversible until (4) starts**. 0t is split: its parts 1-3 (a release
a tester downloads instead of building) are 0w (3), ahead of the web page,
after Rodin's audit; its self-update is 0w (6). Truth when this
and the code disagree: `crates/itsanas-cli/src/control.rs`. Traps this time:
a Python edit passed through a Git Bash heredoc lost the backslash of `\n` and
matched nothing (write the script to a file); PowerShell's `GetNewClosure()`
hides a script's own functions from the handler (the tray uses the item's
`Tag`); one Bash call starting a detached daemon with `sleep`s was refused --
the end-to-end run went through a Python script that kills its daemon in
`finally`. **Missed by the session, caught by its reviewers:** the vault
drain's pause guard was `match drain(..) { _ if paused => {} .. }` -- the
drain runs before any arm is chosen, so a paused node adopted what its own
devices pushed; and the first test written for it passed on nothing,
because a node with no pledge refuses even its own account's push. Both now
have a test that fails on them. Left on disk, harmless: the worktree `D:/GitHub/itsanas-wipe` still
holds the merged branch `docs/fleet-wiped` (`git worktree remove` it).

**2026-10-06 evening, the whole fleet wiped; NEXT is a self-updating
release.** After the live test (Pi, VM, laptop, Mandarine's Mac all saying
IT WORKS, files, renames and deletions crossing over the internet once the
laptop ran #241), Nicolas asked for a clean base everywhere before the next
version: every account was test data. Done 2026-10-06: the laptop (by the
session; the admin-only `ITSaNAS` task and the Pi/VM by Nicolas, running
`wipe-linux.sh` -- the classifier refuses this session stopping services on
those hosts), Mandarine's Mac (by her, from commands given). **Nothing
ITSaNAS is installed anywhere now: no node, no binary, no coordinator, no
directory, no `itsanas-coord` user, no ufw rule.** Kept on purpose: the four
Freebox forwards (9797 -> Pi, 9798 -> Pi, 9799 -> VM 9797, 9898 -> VM), Rust
on each machine, SSH and sudoers. Nicolas's private notes
(`Documents\ITSaNAS`, `TEST-VISUEL.md`, `essai-24-mots.txt`) describe a fleet
that no longer exists. So the next session starts with a coordinator to
install (`install/coordinator.sh` on the VM, `--admit-first`) as part of 0t's
first real run. Traps this time: zsh aborts a whole line on an unmatched
glob (`setopt nonomatch`); Windows `-Filter 'itsanas.exe.*'` also matches
`itsanas.exe` itself (the 8.3 wildcard rule) -- the session archived the
running binary that way; a pasted `*name*` loses its asterisks to markdown.

**2026-10-06 afternoon, one stalling peer no longer holds a round** (branch
`fix/peer-session-budget`). In the live test with Mandarine, the laptop's
`essai` daemon sat 45 minutes on one connection to `sigseg42` (an older
build, on the same laptop, reached through a VMware adapter): the 30-second
read timeout never fired, so the round never ended, and nothing was sent or
written -- not the laptop's new files, not the Mac's. Nicolas's verdict: it
has to work on a machine full of VMs without anyone switching adapters off.
Fixed two ways: `PeerClient::connect_within` with `PEER_SESSION_BUDGET`
(300 s for the whole conversation, handshake included, in
`itsanas-net/src/transport.rs`; the daemon's `sync_once` uses it, `connect`
for interactive commands is unchanged); and the daemon writes the folder
right after the account's own devices, before the hosts of other accounts.
Red-team: `red_team_a_peer_that_trickles_cannot_hold_the_caller_past_its_budget`
(a byte a second; sabotaged by ignoring the budget: still held at 20 s,
red). **Not fixed, still named:** why that older build answered so badly
(version skew; it is retired by upgrading it), and discovery on virtual
adapters (the same device "moves" between 192.168.19.1 and 192.168.117.1
every round): harmless now that a bad path costs at most one budget, still
noise worth a step. The folder-first reorder has no test of its own: it
needs the daemon loop, which no test drives.

**2026-10-06 afternoon, the test bed across the internet** (branch
`step/testbed-announce`). The first deployment worked only on the home
network: the bed never announced a public address, and the Freebox forwarded
only 9797 and 9898, so the laptop, away that day, dialled 192.168.1.x and was
refused. Nicolas added two forwards (9798 -> Pi 9798, 9799 -> VM 9797); the
Pi's and the VM's `essai` nodes ran `itsanas announce ngas.fr:9798` /
`ngas.fr:9799` and `register`. Verified at 14:11: the laptop, off the home
network, sent its 50 MB to both through `ngas.fr` and fetched theirs; the Pi
and the VM report **IT WORKS** with the laptop's files. `testbed.sh
--announce` / `testbed.ps1 -Announce` now do it at setup (README says why).
Found on the way, **not fixed, each worth a step**:
- `Stop-ScheduledTask ITSaNAS-essai` does not stop the daemon. The
  wrapper `run-daemon-essai.ps1` restarts it ("restarting in 10 s") and
  outlives the task, so a later start races a live process for the store.
  Seen on the laptop.
- The laptop's own nodes are discovered on VMware's virtual adapters
  (192.168.19.1 / 192.168.117.1). The log shows one device "moved" back and
  forth between them every round.
- Each round re-sends about 50 MiB to the same older-build nodes of other
  accounts (Pi 9797, VM 9799/9801) and delays writing the folder: 13 min
  from reception to file on the Pi.

**2026-10-06 afternoon, §8 0r deployed on the Pi, the VM and Windows;
two bugs found doing it** (branch `fix/testbed-ps1-phrasefile`). The Pi
founded `essai` (invitation minted as `nicolas`, its service stopped and
restarted for it, with Nicolas's approval); the VM joined over SSH with
`--phrase-file`, and showed **IT WORKS** with the Pi's files received. Found:
(1) `testbed.ps1` declared `$phraseFile = $null`, and PowerShell names are
case-insensitive, so `-PhraseFile` was always erased and an unattended join
hung on a prompt -- now `$tempPhraseFile`, and `check-installers.sh` refuses
any assignment to a parameter's name in another case (sabotaged: red);
(2) `windows.ps1` could not replace `itsanas.exe` while another node's daemon
ran (`sigseg42` on the laptop); it now renames the running file aside and
copies the new one in, which Windows allows. Not hermetic-tested: it needs a
running Windows process; verified by the laptop's own install. Still on the
VM, untouched: a default node (`itsanas.service`) and an old
`itsanas-mandarine.service` from September, both older builds -- Nicolas's
call whether they go.

**2026-10-06 (midday), NEXT is §8 0s, the vault on another disk.** 0r is
merged (#235) and waits only on Nicolas running it by hand (his private
`Documents\ITSaNAS\TEST-VISUEL.md`; the invitation needs a brief stop of the
Pi's `nicolas` service, which the session's auto mode refused). The VM's disk
was grown to 42 GB the same morning (`/` 39 GB, 16 GB free, ext4 reserve kept
at 5 %), so it can join the bed. 0s is next because Nicolas wants the VM to
host from his NAS later: the vault's path is hard-wired and a dropped mount
would silently become an empty vault. Planned in §8 0s, nothing built.

**2026-10-06 (morning), §8 0r built: `install/testbed.sh` and
`install/testbed.ps1`** (branch `step/8.0r-testbed`). One command per machine
installs, founds or joins the account `essai` (instance `essai`, 10G pledged,
3G kept), drops `bonjour-depuis-<host>.txt` plus a 50 MB file, and prints
✅/❌ lines ending in `IT WORKS` once another machine's greeting *and* one of
its 50 MB files have arrived whole. NEXT stays 0r: it is done only when the
greetings cross on real machines (Nicolas's Windows, Mandarine's Mac). What
the review of the WIP commit found and fixed: macOS `--clean` left the bed's
LaunchAgent loaded at every login (clean.sh `--instance` now removes
`net.itsanas.<instance>.plist`, the label was `fr.ngas.*`, renamed to the
`net.itsanas.*` family macos.sh uses); the Mac kept its passphrase in a
second place clean.sh did not know (now `~/.config/itsanas/essai.environment`
everywhere); `status` over `ssh host 'cmd'` read a running daemon as stopped
(no XDG_RUNTIME_DIR; same fix as provision.sh); `--phrase-file` /
`-PhraseFile` added for unattended joins and never deletes the caller's file.
**Facts that bound the real run, checked over SSH the same morning:** the VM's
disk is **100 % full** (133 MB free of 22 GB; 9.1 GB in
`/var/lib/containerd`, 2.9 GB in `~/micro-ai`) and it hosts the coordinator,
which needs room to write its directory -- pruning Docker images is Nicolas's
call, not a session's, so the VM is out of the bed until he frees space. The
bed is one more instance, so it takes the next free port (9798...); the VM's
ufw opens only 9797 and the Freebox forwards only 9797 (Pi) and 9898
(coordinator), so the bed is a **home-LAN** test: a Mac outside the house
reaches the coordinator but none of the bed's nodes. The plan below asked for
"coordinator answers" and "devices" lines in the verdict; they are not built,
`itsanas doctor` (printed by `status`) gives both.

**2026-10-06, NEXT moved to §8 0r, a test bed in one command per
machine** (asked for by Nicolas the same night: a visual test on
Mandarine's Mac, his Windows and maybe the Pi/VM/Android at ~10 GB, with all
preparation done by the script). It goes ahead of 3c's build, which stays
fully planned in §8 3c. Plan only, nothing built: the session ran out of its
5-hour window after #233. The routine decisions taken alone are listed in
0r; reverse any he objects to.

**2026-10-06, §8 3c measured: padding does not close the fingerprint;
Nicolas chose keyed chunking** (branch `step/8.3c-padding-measure`). The
measurement is `crates/itsanas-store/examples/padding_cost.rs` (production
chunker, five class sets, prints aggregates only). On 30.2 GiB of Nicolas's
real files (Ma musique, Mes vidéos, ComfyUI: 25 344 files, 452 668 chunks,
1 090 of two chunks or more, 9 min 26 s), the share of multi-chunk files
whose padded size sequence no other file shares: 98.2 % today, 77.8 % with
Padmé (+1.5 % disk), 19.2 % with powers of two (+59.8 %), 4.7 % with every
chunk at 256 KiB (+265.6 %, and the count still leaks). §10 item 9 said such
a result goes back to Nicolas before code; asked as a closed question, he
chose **keyed chunking** (details §10 item 9, plan §8 3c). No product code
changed. Traps: a first run over `D:\GitHub` too was killed at 30 min with no
output (the tool now prints progress to stderr); and the selective-CI
session had checked out its branch in the same `D:\GitHub\itsanas`, so this
step moved to the worktree `D:\GitHub\itsanas-8.3c` -- **two sessions on one
repo need two worktrees**.

**2026-10-06, outside §8: selective CI** (asked for by Nicolas; branch
`ci/selective-tests`). A pull request now tests the crates it changed plus
every crate depending on them; docs- and scripts-only PRs run no Rust test
job; push to `main`, nightly, the `milestone` label and Cargo.lock / manifest /
toolchain / `.config/` / workflow changes run everything. `scripts/ci_scope.py`
decides, the `changes` job publishes the plan, `check-ci-scope.py` tests it
hermetically and checks `ci.yml` obeys it. **The merge rule changed** (see
§4b, under `merge-when-green.sh`): skipped is green only when the last job
(`No warnings anywhere in this run`) passed, because it runs
`ci_scope.py verify`. Merged as #228, measured right after (ROADMAP
"Selective CI"): a leaf change 3.7 min / 16.5 runner-min against 6.5 / 25,
crypto unchanged (it reaches 15 of 17 crates, as it should), docs-only 1.4 / 2.1 (#225 cost 22.9)
instead. Trap: a matrix job skipped by a job-level `if:`
reports as "Test (${{ matrix.os }})", unexpanded, and the three required
"Test (...)" checks would never arrive -- hence the per-step gates on `test`.
The same happens if `changes` itself fails: `test` is skipped through `needs`
and the PR waits for checks that never come. Look at `changes` first.
Rodin's two points kept: `verify` proves a skip was planned, not that the plan
is right (that is `check-ci-scope.py`'s job alone); and the verifier is not a
required check, so a hand merge is not held by it -- §10 item 10.

**2026-10-05, late: §8 3e, the installers put `itsanas` on the PATH.**
`linux.sh` and `macos.sh` share one `path-line` block (byte-identical,
compared by `check-installers.sh`): under `confirm` / `--yes` they add
`export PATH="<bin>:$PATH" # added by the ITSaNAS installer` to the login
profile (`~/.zprofile` for zsh; `~/.bash_profile` if it exists for bash, else
`~/.profile`), once; Debian's stock `~/.profile` already covering
`$HOME/.local/bin` is left alone. `clean.sh` removes exactly lines with that
mark. `macos.sh` now waits (10 s polls, an hour) for
`/Library/Developer/CommandLineTools/usr/bin/clang` instead of dying. Tested
hermetically in `check-installers.sh`, four sabotages red. **Not verified**:
the wait loop has run nowhere (CI runners have the tools; it needs a fresh
Mac), and the real CI install job does not assert the second run / clean --
the hermetic test does. `~/.cargo/bin` stays a warning on purpose: `itsanas`
does not need cargo to run, and the installer finds it itself. Trap: WSL
(`wsl.exe -e bash -c ...`) runs `check-installers.sh` fine from this laptop;
it prints a networking error first and works anyway. NEXT is 3c, which
Nicolas unblocked (§10 item 9: pad, after measuring).

**2026-10-05, evening: a real Mac, and four of Nicolas's answers.**
`install/macos.sh --yes --no-service` ran on Mandarine's Apple-silicon Mac
from the `main` tarball: built (27 s), installed, smoke test PASS, native
arm64 (§10 item 5). Only the last 80 lines came back, and the run took 29 s,
so the Command Line Tools and Rust were most likely already there: the
fresh-Mac path is still unseen. It found one defect a newcomer hits at once:
`~/.local/bin` is not on the PATH, and the installer only warns. That and
the tools wait are §8 3e, now NEXT ahead of 3c because the pilot with
Mandarine is what the project needs most (Rodin, §0 below). Nicolas decided
§10 items 3, 7 and 9 the same evening (written there). Docs only.

**2026-10-05, §8 1c (iii) and (d): step 1 closed.** (ii) merged as #223
(Nicolas merged it: it added a §6 row). (iii) **not built, by decision**:
the claim book stays in memory; why is in §8 1c, in short no security gain
(an empty book refuses) against a wall clock that is wrong at boot on a Pi.
(d): `ECONOMICS.md` §1 is ✅ again -- hosts enforce the bargain -- with what
a rebuilt client still gets named under it. Docs only, no test changed.
Next, §8 3c, needs Nicolas's answer to §10 item 9 first; §8 5 needs his
phone and key. Nothing else in §8 is open. **If §10 item 9 is still
unanswered, do not wait and do not ask:** take the first open finding in
ROADMAP "What an adversarial sweep found" that needs no decision, or
the ceiling that `Instant` ignores a suspended host's sleep (ROADMAP,
the §8 1c (ii) entry), and say so in §0. Rodin's point,
kept: everything ✅ here is still untested against a second person (§10
item 1); that, not padding, is the question that decides the project. Also in this PR's predecessor:
five `SESSIONS.md` rows (#219-#223) the cloud sessions never wrote.

**2026-10-05, §8 1c (ii): hosts read the coordinator's withdrawals**
(branch `ccr-dc079ac8-rqnbeq`; (i) merged as #222). Decided by Nicolas the
same day, asked as a closed question: a coordinator that does not answer
means **no storing** for other accounts' devices (§6 row). Built:
coordinator `Request::Standing(claim)` / `Response::Standing(Option<claim>)`
(appended: request 15, response 12), answered only for a claim the owner
signed and only with a claim under that account (`service.rs::standing`);
`ClaimBook` (`owners.rs`) keeps each device's `Standing` (unconfirmed / live
at an instant / withdrawn, final), asks on presentation through an `Asker`
(`coordinator::asker`, at most `ASKS_PER_MINUTE` = 30) and each daemon round
(`coordinator::standing`, 64 due a round, ageing confirmations first,
`daemon.rs::check_standing`), answers taken only from a pinned coordinator;
`admits` refuses `WITHDRAWN` / `UNCONFIRMED`; this host's own account is
exempt. `itsanas serve` asks inline too. Acceptance: host2 and host3 now
have the coordinator configured; without that line the bench goes red on
three phases (checked). The `redteam` agent on the diff found four, all
fixed with a test each: the round read the book in id order, so junk devices
took every question (now ordered, with `ASK_AGAIN_AFTER`); 8 a round lapsed a
host with ~100 foreign devices (now 64); an unpinned coordinator's answer was
believed (now refused, `coordinator::pinned`); a `Refused` ended fresh
confirmations (now no answer). Verified: 20 tests (14 red-team), 26
sabotages red, acceptance-local passes. **Named, not fixed** (ROADMAP): a
confirmed device stores up to an hour past its withdrawal (two with the
coordinator silent); the inline ask holds that connection's
thread for a coordinator round trip; a coordinator can withhold answers to
cut an account off everywhere (the denial of service it always had).
**Merge left to Nicolas:** the PR adds a §6 row. Trap: the first wiring
asked only per round, which refused every first contact -- the bench's
single `sync` caught it before any code was pushed. Trap: the cloud
container's clippy is 1.97, CI's is 1.99 (its double-must-use and
assert-is-empty lints); `rustup toolchain install 1.99 -c clippy` and `cargo
+1.99 clippy` before pushing. And run the whole `check-all.sh` after a
docs-only edit too: a back-quoted snake_case word reads as a test name.

**2026-10-05, §8 1c (i): a pledge the host tested and found short counts
for what was proved** (branch `ccr-dc079ac8-rqnbeq`). #220 (1c first part)
and #221 (its missing paused-proof test) merged first. The test already ran:
the push path offers every dialled device what it lacks and stamps a
`PledgeFull` refusal (`note_peer_full`); the bound never read it. Now
`standing` (`crates/itsanas-node/src/owners.rs`) counts a device refused
within `FULL_RETRY` (made `pub` in `session.rs`) for `min(pledged, proved)`,
per device, not as a contradiction: siblings keep their pledges and an honest
full device earns at the same ratio. Verified: 3 tests (1 red-team), 4
sabotages red; `cargo test -p itsanas-node` green. **Named, not fixed:** a
device this host never dials (behind a router) is never tested -- it still
gets credit on its promise, bounded only by the 3/10 share; and when such a
device pulls this host's chunks itself (`take_on_hosting`), running out of
room there is never reported back to this host. No Rodin: 1c's plan had his pass (AGENTS.md, once per major step). Trap:
`scripts/sabotage.py` says "the build itself refused it" for any `cargo test
-q` failure; read the red test's name by hand.

**2026-10-04, §8 1c first part: hosts bound accounts** (merged as
#220 on 2026-10-05; the agents PR #219 merged first). Rule decided by
Nicolas the same day: credit immediately on the space offered, 30/70;
contradicted, 30/70 of what is proved; no claim, no storing. Built: peer
protocol 7, `Request::Claim` answered with the host's own claim
(`Response::Claim`, so a machine behind a router can bound what it pulls);
`Node::claim_bytes` signs a fresh claim per connection (no file);
`itsanas_net::Owners` injected like `Relay`; `ClaimBook`
(`crates/itsanas-node/src/owners.rs`); `holder_counts` table in the index
(`Index::held_by`, `Store::bytes_held_by` = records x mean chunk size);
`host_for_bounded` gates the hosting pull; `Vault::held_bytes_for` is a range.
Verified: 16 tests (14 red-team), 18 sabotages red, acceptance-local passes.
Re-verified 2026-10-05 before merging, sabotage by sabotage: one defence --
`standing` counts only *unpaused* devices' records as proof -- turned no test
red; `red_team_a_device_paused_for_its_audits_proves_nothing` now does.
(`scripts/sabotage.py` labels a `cargo test -q` failure "the build itself
refused it": read the test name by hand before believing either way.)

Rodin on the plan: the claim is self-signed (every node holds the account
key), so "not contradicted" means "nobody looked" -- hence the shared 3/10
share for credit on promises. The `redteam` agent on the code, all fixed but
3: the pull path skipped the bound; "proven" was a flag one audit set, which
re-opened the host to a petabyte claim (now bytes); the book could be
squatted (evicts empty accounts); `verify(now)` would refuse everything on a
1970 clock; per-store cost (cached total, owner range). **Named, not fixed:**
a withdrawn device re-signs; an account can keep a paused device away; the
book is in memory. Trap: the pull-path gate first broke reciprocal hosting
behind NAT -- the NATed side is never dialled, so never learned the peer's
claim; the two-way `Claim` exists for that.

Older §0 entries, 2026-09-14 to 2026-10-04 (the §8 4c entry and before), moved verbatim
to [HANDOVER-ARCHIVE.md](HANDOVER-ARCHIVE.md): history, not instructions.
The rules that still bind are in §3, §4b and §11.

**Where the truth is:** `docs/ROADMAP.md` "Known ceilings" and "What an
adversarial sweep found" (open findings, with arithmetic); `docs/ECONOMICS.md`
§1 (the 30/70 bargain: each host enforces it since §8 1c, limits named there); `docs/DESIGN.md` §6.5–6.7
(verification cost against the 100 MB/day budget).

---

## 1. Where things are

```
D:\GitHub\itsanas
remote: https://github.com/SigSegGit/itsanas   (public, AGPL-3.0-or-later)
branch: main          tags: v0.1.0
CI:     .github/workflows/ci.yml — Linux, Windows, macOS, ARM, Android core,
        cargo deny, installers run on each OS
```

The fleet: this Windows laptop (a scheduled task named `ITSaNAS` runs the
daemon; its passphrase file is under `%LOCALAPPDATA%\itsanas`), a Raspberry Pi
4B, and an aarch64 VM on a Freebox Delta. **The coordinator belongs on the VM**
(Nicolas, 2026-09-17; `docs/MVP.md` §2 row 4): it is the only machine with a
public address, and NAT traversal is not built. Both machines run one today,
and the accounts `nicolas`, `voisin` and `sigseg42` are still registered on
the **Pi's**, until they are migrated — a procedure nobody has written or run.

**Reaching the fleet from an agent session:** `ssh -i ~/.ssh/itsanas_session -p
22010 itsomeone@ngas.fr` is the Pi and `-p 22011` the VM (the LAN addresses
refuse SSH from the laptop; the public name works from inside, so the Freebox
does hairpin NAT for SSH). Checkouts are in `~/src-itsanas`; the member daemons
run as user services. Since 2026-09-17 both machines carry
`install/sudoers-itsanas` in `/etc/sudoers.d/itsanas`: the coordinator can be
stopped, upgraded, started and read **without a password**, in exactly the
forms that file lists (`install/README.md`, "Operating the coordinator without a
password"). Nothing else runs as root from an agent: re-running
`coordinator.sh`, dropping `--admit-first` and touching
`/var/lib/itsanas-coordinator` are Nicolas's. An agent never types a password,
even one given in chat. State on 2026-09-17: the Pi's coordinator active with
`--admit-first`; the VM's installed, enabled and stopped.
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
every figure README, ROADMAP and TESTING state against them — today **727 test
functions across 24 binaries** (3 of them `#[ignore]`d) **plus 2 doctests**, 46
of them red-team. This file is not among the ones it reads, so this sentence
is corrected by hand. It counts the source rather than `cargo test -- --list`
because that command's answer depends on the machine running it: one test is
`#[cfg(unix)]` and the ProjFS ones are `#[cfg(windows)]`, so the same tree lists
a different number on Linux and on Windows, and a count that moves with the
reader is not a count. This paragraph quoted 464 across 17 binaries — the old
command, on a tree that has since grown by half — and nothing read it.

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
cli        `itsanas` binary: commands + daemon
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
| Every refusal at the door reads identically | Distinguishing "no such code" from "expired" from "spent" lets anybody enumerate which codes exist, and the codes are what keeps strangers out | `every_refusal_reads_the_same_so_codes_cannot_be_enumerated` |
| **Nothing makes `has_chunk` true without proof** | The rule, in one line, because it existed as two methods with two different answers and the unverified one was on the ordinary path. A blob on disk under an address is how every other part of this system decides it need not go looking: write noise there and the repair scan stops searching, no other peer is asked, and the loss queue clears the entry `doctor` put in it. A recoverable loss made permanent, which is strictly worse than the peer refusing to answer | `red_team_a_relay_cannot_poison_a_chunk_on_the_ordinary_pull_path`; `red_team_a_host_cannot_answer_a_repair_request_with_rubbish`; one method, `Store::accept_chunk` |
| A chunk fetched to repair a local loss is verified before it is written | Unverified bytes make `has_chunk` true, the scan stops looking, no other peer is ever asked, and a **recoverable** loss becomes permanent. A host cannot read what it stores, so answering a repair request with noise is its one route to destroying data | `red_team_a_host_cannot_answer_a_repair_request_with_rubbish` |
| A test harness that runs a server stops it when the body panics | Otherwise `thread::scope` joins an accept loop nothing shut down and the suite reports a hang. Every red-team test in `two_nodes.rs` runs inside that harness, so a test catching an attack reported a timeout — which everybody retries and nobody reads | `a_failing_assertion_inside_a_server_scope_fails_rather_than_hangs`; `StopOnDrop` in `two_nodes.rs` and `coordinator.rs` |
| A replication target counts this device | Off by one means the repair loop keeps two copies while reporting three, invisibly, until two machines die instead of three | `a_target_counts_this_device_so_three_asks_for_two_elsewhere` |
| A peer is recorded as a holder only when it accepted or already had the chunk | Recording a refusal as storage is indistinguishable from safety until the local disk dies | `a_host_that_refuses_to_store_is_not_recorded_as_holding_anything` |
| A discovery beacon's address comes from the UDP source, never from the packet | A self-declared address lets any node redirect traffic to a machine that is not it | `a_new_device_is_recorded_with_the_address_it_was_heard_from` |
| The discovery table is bounded and confirmed peers are protected | Device ids are free keypairs, so a flood is cheap; without this it evicts the machines that matter | `a_flood_of_strangers_cannot_evict_a_known_peer`, `the_table_never_grows_past_its_capacity` |
| The sender's clock decides nothing in discovery | A Pi 4 has no RTC and boots in 1970; superseding by sender clock strands it at a stale address | `a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address` |
| The split is a value, and the one that grants entitlement is the coordinator's | It was `CONTRIBUTION_RATIO = 3`, and a constant cannot express 30/70 without becoming a fraction, which is where an `f64` wants to go. Two splits now exist and they are not the same thing: a node's configuration field decides only what that machine refuses its own owner, and the one `assess` is handed decides what the network grants. A `split` field on `DeviceContribution` would let a member widen their own entitlement by editing a text file. The node's field may only be stricter than `Split::DEFAULT`: `itsanas keep` is the one live enforcement, and a generous split would turn it off | `red_team_entitlement_follows_the_coordinator_s_split_not_a_device_s`; `red_team_a_node_cannot_grant_itself_a_more_generous_split`; `red_team_a_split_with_a_zero_part_is_refused_rather_than_dividing_by_zero` |
| A withdrawal is final for its device id and wins whatever the signing clocks say | Every keystore holds the master secret, so a claim signed after a withdrawal proves nothing about who signed it; and a signer's clock is an opinion. Timestamp ordering let a stolen machine re-enrol and let a slow clock cancel a withdrawal. A reused machine logs in afresh and gets a new device id | `red_team_a_machine_holding_the_master_key_cannot_bring_a_withdrawn_device_back`; `red_team_a_withdrawal_signed_on_a_slow_clock_still_withdraws`; `a_later_enrolment_does_not_supersede_a_withdrawal` |
| A host stores for another account's device only once its coordinator has confirmed the device live, within `STANDING_FOR`; no answer means no storing. Decided by Nicolas on 2026-10-05 ("Refuse") | Every node holds its account key, so a withdrawn device signs itself a fresh live claim; the coordinator's withdrawal is the only word against it. Letting devices through while the coordinator is silent would make every outage, and every coordinator that hangs up on the question, a window for withdrawn devices. The cost, accepted: newcomers wait for an answer, other accounts stop storing on a host whose coordinator is down past `STANDING_FOR`, a host with no coordinator stores for its own account only. Consistent with the coordinator's place: it can refuse a member, never admit one | `red_team_no_word_from_the_coordinator_means_no_storing`; `red_team_a_host_whose_coordinator_does_not_answer_stores_for_no_other_account`; `red_team_a_withdrawn_device_that_re_signs_stores_nothing_on_a_host` |
| An account has at most `MAX_DEVICES_PER_ACCOUNT` live devices -- 5, decided by Nicolas on 2026-09-30 ("5 max") | Enforced on the coordinator, the bound a rebuilt client cannot remove, and on the enrolling client so the refusal names the devices. A withdrawal frees its slot; re-signing a live device takes none; an account above the bound is never cut down, it only cannot add one. Raising it, or counting re-signings, is a decision for Nicolas | `red_team_a_sixth_device_is_refused_and_nothing_is_written`; `red_team_a_withdrawn_slot_lets_one_more_in_and_the_withdrawn_device_stays_out`; `red_team_re_signing_a_live_device_on_a_full_account_takes_no_slot`; `red_team_a_sixth_machine_refuses_to_enrol_itself_even_where_the_coordinator_would_not` |
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
  escrow of a sealed recovery container. The accounting *rules* exist; nothing on
  the network applies them (§8.1).
- **Machines that hold less than the account**: `itsanas keep` budgets with an
  ordering, selective fetch, `itsanas space` bounded by disk and pledge.
- **Front ends**: folder daemon with watcher; Android app (Compose over JNI,
  emulator only); Windows virtual drive over ProjFS (read-only).
- **Installers** for Linux, Windows, macOS, Termux and the coordinator, each with
  `--clean` delegating to one uninstaller. Run for real on the Pi, the VM and
  Windows; macOS in CI.

## 8. What is next, in order

0. **Pass the MVP on the fleet, before any more enforcement.** Decided
   2026-09-14, when Nicolas asked for a valid MVP. `docs/MVP.md` defines it as
   tests A–M and O passing unassisted on the fleet, and §6 there shows every
   one is built and most have never been run outside the laboratory. Verified
   facts:

   - Escrow recovery is built and unrun: `itsanas register --recovery` lodges
     the sealed container (`crates/itsanas-cli/src/main.rs` ~1358),
     `itsanas login --username <name> --from <coordinator>` restores from it
     (`login_from_coordinator`, ~1205). That is the whole of test D.
   - Nothing in §8.1 is on the MVP's path: the fleet is one person's
     machines, and MVP.md §4 makes "open it to others" the question *after*
     the verdict.
   - What an agent cannot do: cut power, reboot the Pi, leave a laptop asleep
     for a day. What it can do is make each of those a command that checks
     and prints a verdict, so a test costs Nicolas minutes and no judgement.

   a2. ✅ **C, K and M automated in `acceptance-local.sh`** (2026-09-16), with
      a second account that pledges and really hosts. C -- the test whose
      failure stops the project -- had never run anywhere but by hand. See §0
      for what K found about `sync` not auditing, and for the open question
      about `placements`.

   a. ✅ **An acceptance kit: `scripts/acceptance.sh <test> <phase> [args]`.**
      Run on the machine the phase is about; prints `PASS`/`FAIL` with the
      numbers and appends the line to `~/.itsanas-receipts/acceptance.txt`,
      in `receipt.sh`'s format. It checks, it does not orchestrate — Nicolas
      moves power and cables. Phases, each a function:
      `B write` (random bytes into the synced folder, SHA-256 printed) and
      `B check <sha>`; `C plant` and `C scan <canary>`, with the planted-copy
      control inside the scan so a grep that finds nothing is known to work;
      `D lodge` and `D restore` (`register --recovery`, then `login --from`
      on a fresh home, then a file compared by SHA-256); `E write`/`E check`;
      `F delete <name>`/`F check <name>` (gone, and still gone after a
      second round); `G edit <tag>`/`G check` (both versions, one named
      `.conflict-`, same answer on both machines); `H sample` (CPU, RSS,
      bytes written, meant for a five-minute timer) and `H report`;
      `I status` (the degraded line names the coordinator); `J check`
      (`doctor --deep` clean, file count unchanged). Linux first — the Pi
      and the VM — with the laptop phases as a `.ps1` only if B/E/F/G need
      it there. Tests: `bash -n` and shellcheck through
      `check-installers.sh`'s pattern, and one CI job running B, F and G's
      phases between two homes on one runner, so a phase that cannot pass
      is found before Nicolas spends a morning on it. MVP.md §3 gets the
      command under each test.
      **Built 2026-09-14**, as specified except: Linux and Git Bash only (no
      `.ps1` — nothing in B–G needed it); H, I and J are exercised locally only
      by their negative controls, since they need a day, a coordinator outage
      and a reboot; and C has only its negative controls locally, because a
      scan of the owner's own state finds the file name in `store/index.redb`
      — correct on the owner's machine, and the reason C is run on a host of
      another account, which the bench does not set up. MVP.md §3 has the
      command table. After the Rodin audit: the canary is also the file name,
      I's refusal says idle rounds are silent, and `D check` keeps the
      passphrase prompt visible. Not fixed: E cannot prove two machines never
      met, and F counts neighbours without comparing them to before.
   b. ✅ **Say why a peer refused what was pushed.** Built 2026-09-15 as
      specified, with the reason as a `Copy` enum rather than the peer's text
      (`PushReport` is `Copy`, and a hostile peer's sentence has no business in
      a log line). Verified facts, as found:
      `StoreChunk` and `StoreSegment` (`crates/itsanas-net/src/service.rs`
      ~138–153) answer `Refused("pledged capacity exhausted")` against a
      host's pledge for every owner, its own account included, which DESIGN.md's
      table ("your own log relayed between your devices | `pledge`") says is
      intended. But the push side maps every refusal to `false` (the comment in
      `session::push_scoped`, `crates/itsanas-net/src/session.rs` ~435 says so),
      and `itsanas sync` (`crates/itsanas-cli/src/main.rs` ~2121) and the daemon
      (`daemon.rs` ~904) then print `sent 0 B in 0 chunks, 0 segments` — the
      line for "nothing to send". Count refusals in `PushReport` with the first
      reason and print `refused N: <reason>`. Red-team test: a host with pledge 0
      is pushed to, and the report names the refusal; sabotage by dropping the
      count. Found by the acceptance bench, whose E, F and G failed until every
      node pledged.
   c. **Nicolas runs A–M and O with the kit.** Gated on (i) and (j) since
      2026-09-16. The next session pastes the receipts
      into MVP.md §6 and applies the verdict rule of §4 as written.
   d. **Whatever fails becomes the next item**, ahead of everything below.
   e. ✅ **Measure H on the machine it is about.** Built 2026-09-15 as
      `scripts/acceptance.ps1 H schedule|sample|report|sleep|unschedule`.
      Battery is reported (`powercfg /batteryreport`) and not judged; bytes
      written include network I/O, which the process counter does not
      separate; `H sleep` needs an administrator PowerShell for
      `powercfg /requests` and refuses a verdict without it. The existing
      sampler outside the repository was read and superseded. As specified: `H sample` reads `/proc` and
      `pgrep`, so it runs on the Pi and the VM, where nobody asked. The criterion
      is the Windows laptop: battery, CPU at idle, memory, and whether it sleeps.
      A sampler already exists outside the repository
      (`%LOCALAPPDATA%\itsanas\sampler.ps1`, seen, not read) — bring its
      measurement into `scripts/acceptance.ps1` with the same verdict line, plus
      `powercfg /requests` for whether the daemon holds the machine awake.
   i. 🟨 **Install and configure two accounts per machine, in one command
      each.** First pass done 2026-09-16: `init` no longer sends a new account
      to `serve` (which never syncs) and the listen-port line names every port
      it skipped rather than only the first. Three accounts were created cold
      on the laptop to find those. **Still open, and why `NEXT` is still here:**
      `provision.ps1 -Instance` / `provision.sh --instance` have not been run
      -- they touch scheduled tasks, systemd units and running processes, and
      the laptop's daemon holds real data -- and `install/macos.sh` has never
      run on a Mac. Both want a scratch machine or Nicolas at the keyboard.

      Asked for by Nicolas on 2026-09-16, and it gates (c): he
      will not spend a day running A-M and O until this is true. The target, in his
      words, is that setting a machine up with two distinct accounts is "as
      simple as it should be" -- for him on Windows, the Pi and the VM, and
      later for a second person on a Mac and an Android phone.

      Verified facts, so nobody re-derives them: `install/provision.sh
      --instance NAME` and `install/provision.ps1 -Instance NAME` already give
      an instance its own home, passphrase file and `itsanas@NAME` unit or
      `ITSaNAS-NAME` task (#18); `clean.sh`/`clean.ps1` take the same flag;
      `init`/`login` already pick the first free port from 9797; discovery
      already shares UDP 21037. So the mechanism exists and **what is missing
      is that nobody has run it cold, twice, on three operating systems, and
      written down where it stops being obvious.**

      Do it in that order: run it, keep a verbatim log of every place a person
      has to think, then fix those places. Expect the findings to be about
      wording, prompts and defaults rather than about code. `install/macos.sh`
      has **never been run by a human** -- only on a CI runner -- and that is
      the riskiest square in the table.

      **Hermetic half done 2026-09-30 (see §0).** Was: make that red-team
      test hermetic in `scripts/check-installers.sh` (already a CI step, so
      no `ci.yml` change): a throwaway `HOME`, a fake `systemctl` (and
      `loginctl`) first on `PATH` that logs its arguments, a stub `itsanas`
      binary if `provision.sh` needs one; run `provision.sh --instance a`,
      then `--instance b`, then `clean.sh --instance b --purge-account`, and
      assert `~/.itsanas-a`, `~/.config/itsanas/a.environment`, the
      `itsanas@` template unit and a's enablement are untouched while b's are
      gone. Sabotage: make `clean.sh --instance` remove `"$ENV_DIR"/*.environment`.
      Read `provision.sh` first for what it really calls (build from source?
      download?) and stub exactly that; if it cannot be made hermetic in
      reasonable size, say so here and move on to `k` prerequisites.

      Red-team test expected: two instances provisioned on one machine, the
      second `--clean`ed, and the first still syncing. `clean.sh` removing a
      sibling's unit or passphrase file is the failure this guards, and it has
      happened once already (#18 fixed `provision.ps1` killing every `itsanas`
      process).

   j. ✅ **The node's state without the passphrase.** Built 2026-09-16.
      `Index::is_locked` (`crates/itsanas-store/src/index.rs`) asks redb
      whether the index is held; `Store::is_locked` asks it of a store root;
      `Node::store_path` makes that root findable without opening the node;
      and `snapshot_status` (`crates/itsanas-cli/src/main.rs`) renders the
      daemon's snapshot from a path alone, so it cannot prompt. `status` probes
      the lock **before** resolving a passphrase, and still handles the race
      where the daemon takes the lock while one is being typed.

      Bounds deliberately **not** applied, and this is the open question for
      whoever does the tray: the snapshot is the full status text, so it
      carries the username, user id, device id, file counts and sizes. That is
      no worse than before -- it is a plaintext file in the node home, readable
      with `cat`, which is the argument for not demanding a passphrase to print
      it -- but the bounded view described below (alive, last round, peers
      reachable, replication state, **no names or ids**) is what a tray or a
      second person's machine should get, and it does not exist yet.

      The bug it fixed, for the record: `status()` in
      `crates/itsanas-cli/src/main.rs` (~1280) handles
      `StoreError::Locked` by printing the daemon's snapshot -- but it reaches
      that arm through `open(home)` (~610), which is
      `Node::open(home, &passphrase(false)?)`. The passphrase is resolved
      **first**, so on a machine whose daemon is running -- the normal state --
      `itsanas status` demands the passphrase and then would have printed a
      file it could have read without one. `redb` reports the lock as
      `DatabaseError::DatabaseAlreadyOpen` (`index.rs:229`).

      Fix: ask whether the index is locked **before** resolving a passphrase,
      and print the snapshot if it is. The snapshot is already a plaintext file
      in the node home, so this exposes nothing new -- anyone who can read it
      through this command can `cat` it -- and that is the answer to Nicolas's
      question of 2026-09-16 about unauthenticated state.

      Bound what an unlocked read may show, because that question is fair:
      daemon alive, age of the last successful round, peers reachable,
      replication state. **No file names, no sizes, no account name, no device
      ids**, and never over a socket -- a local file with owner-only
      permissions, which is the same boundary that already protects the
      keystore.

      Red-team tests, both sabotage-verified:
      `red_team_a_held_store_says_so_before_anybody_is_asked_for_a_key`
      (forced to answer "not locked", it fails naming the passphrase prompt it
      would bring back) and
      `red_team_a_running_node_is_reported_with_its_age_and_no_passphrase`
      (the age removed from the header, it fails). `an_age_never_reads_as_
      fresher_than_it_is` already covered the arithmetic; what was missing was
      that the arm was reachable.

   k. **v0.2.0, the marker release.** Asked for by Nicolas on 2026-09-16: a
      version that says "concrete enough to test, and nowhere near v1.0.0".
      **With Nicolas, not an agent step (2026-09-30):** re-pinning
      FIRST-STEPS before the tag exists breaks the documented install, so
      the whole of (k) happens at the moment he cuts the tag. Was: (i) is still 🟨 for
      its human runs, so this prepares the release without cutting it:
      bump `[workspace.package] version` (check `Cargo.toml` and every
      crate that does not inherit it; `Cargo.lock` follows), write
      `CHANGELOG.md` from `git log v0.1.0..main` in plain words -- what is
      testable, what is not (the 🟨 list of 0i, 0o, Android's "never on
      hardware"), and re-pin FIRST-STEPS where it says v0.1.0 (the first
      persona run flagged that it disagrees with install/README's `main`).
      **Do not tag and do not build a release APK**: the tag is Nicolas's
      (other machines pin to it) and so is the signing key (§10.2). Grep
      for every `0.1.0` first (`itsanas-docs-sweeper` does this cheaply).

      Cut it **after** (i) and (j) land, never before -- the point of the tag
      is that somebody can install the same feature set on any platform, so it
      is worth nothing until installing is the thing that was fixed.

      What it needs: the workspace version bumped, `FIRST-STEPS.md` re-pinned
      (it is pinned to v0.1.0), a changelog saying plainly what is testable and
      what is not, and a **freshly built APK** -- the only one that exists is
      v0.1.0, debug-signed and stale. The Android release key is Nicolas's to
      generate and hold (§10.2), so an agent cannot finish the phone half.
      Do not tag from an agent session without saying so: a tag is the one
      thing here that other people's machines will pin to.

   l. ✅ **A storage location that vanished never reads as a deletion.** Built 2026-09-18. Asked
      for by Nicolas on 2026-09-17: "a mount point that drops, a disconnected
      disk... are common cases, they must be handled". Verified by reading, not
      yet reproduced by a test: `scan` (`crates/itsanas-folder/src/scan.rs`
      ~132) checks only `root.is_dir()`. An unmounted disk leaves an **empty
      mount point**, every file in the ledger looks deleted, and the folder
      layer's next pass turns them into deletions that replicate to every
      device of the account. No marker file, no mass-deletion guard. The node
      home is safer by accident: `Node::open` fails with `NoNode` when the
      keystore is missing (`crates/itsanas-node/src/node.rs` ~260), and the
      message then suggests `init`, which would create a **second account** on
      the root filesystem.

      Build: a marker (`.itsanas-folder`, holding the device id) written by
      `itsanas folder` at the folder root, and one in the node home. A scan
      whose marker is missing or names another device **stops**, deletes
      nothing and makes the daemon say "storage unreachable: <path>" in
      `status`, and `NoNode` on a path that exists but is empty says the
      storage may be unmounted instead of suggesting `init`. Then a guard: a
      pass that would delete more than half of the folder's files (and more
      than a handful) holds the deletions until `itsanas folder --confirm`
      says so. An existing folder without a marker gets one on its first scan
      that finds its ledger's files present, so upgrading does not stop
      anybody. Red-team tests expected: an unmounted folder (an empty
      directory, a ledger with files) writes no deletion to the log; and a
      folder emptied by accident has its deletions held, not replicated.

   m. ✅ **Count the live copies, and let a machine leave politely.** Parts (1)-(3) built (2026-09-21, 2026-09-28); the restart half done 2026-09-30 as a `departed` marker rather than `ExecStop=` (see §0 for why). Asked for
      by Nicolas on 2026-09-17: each instance checks how many copies are live
      and asks for a new one elsewhere; a machine shutting down on purpose
      should first ask for copies, so a graceful exit can be told apart from a
      crash or fraud in future regulation. Verified facts: repair drains
      `Store::under_replicated` (`crates/itsanas-store/src/store.rs` ~1160,
      `index.rs`), **which counts every holder record whatever its age**; only
      `coverage` applies `holders::CONFIRMED_FOR` (14 days). So a machine that
      died is counted as a copy by repair until its records are withdrawn by
      a failed audit, which needs the machine to answer. Build, in this order:
      (1) `under_replicated` counts only records confirmed within a liveness
      window, shorter than `CONFIRMED_FOR` and stated with its arithmetic;
      (2) `itsanas leave` / a soft stop on the service: the node tells its
      peers and the coordinator it is going (appended `Request` and
      coordinator messages, **never inserted**: HANDOVER §6), peers stop
      counting it at once and re-replicate from the devices still online, and
      the node stops without waiting for that to finish; (3) the coordinator
      records graceful departures apart from silent ones, for the regulation
      Nicolas described, without using them for anything yet. Red-team test
      expected for (1): a holder silent past the window is not counted, and a
      chunk whose other copies are all silent is repaired; and for (2): a
      departure notice signed by another device is refused.

      **(1) is built, 2026-09-21.** `holders::LIVE_FOR` is `REFRESH_AFTER * 2`
      -- seven days, two missed opportunities to be confirmed -- and
      `Index::under_replicated` now takes `now` and counts only records
      confirmed within it. The arithmetic lives in the constant's own comment,
      and two `const _: () = assert!(...)` in `holders.rs` pin the relation at
      **compile time** -- stronger than the test that first held it, because a
      relation between two constants cannot be true at runtime and false at
      build time, and a test can be filtered out where a build cannot. The
      counting defence is sabotage-verified with `scripts/sabotage.py`.

      **(2) and (3) are the next step, and here is why rather than a shrug**: a
      departure notice is an *appended* peer request and an appended coordinator
      message, so it is a wire change, and §6 says those are never inserted.
      That is a different kind of work from a one-constant liveness window; it
      wants its own branch and its own sabotage of the "signed by another
      device" refusal, and folding it in here would mix a data-safety fix with a
      protocol addition in one review.


   n. ✅ **Refuse a file that will not fit, before copying it.** Built
      2026-09-28: see §0 for what and how. Red-team tests, all sabotage-verified:
      `red_team_a_write_past_the_budget_leaves_no_chunk_no_entry_and_no_log`,
      `red_team_what_the_account_holds_elsewhere_counts_against_it`,
      `red_team_an_opened_node_bounds_its_writes_by_what_its_pledge_earns`.
      **Remaining, 🟨:** the native quotas below, which are research and
      presentation; and 1b's disk check. The original text follows. Start
      from `Store::write_stream` (`crates/itsanas-store/src/store.rs`) and 8.1b
      below, which names the check; the in-process refusal is the deliverable,
      native quotas are research only. Red-team test expected: a write that
      would exceed the account's entitlement is refused before any chunk is
      stored, and the refusal names the numbers. This is 8.1b,
      pulled forward on 2026-09-17: Nicolas calls it a basic feature, and asks
      what happens when the disk has room but the account's quota does not.
      Read 8.1b for the specification. On 2026-09-17 he also asked that the
      operating system itself see the quota where it can (Explorer should not
      think a 3 GB file fits a 10 GB folder already holding 9 GB, even on a
      disk with 100 GB free), with a different mechanism per platform
      accepted. What to find out, per platform, before building: Windows —
      the ProjFS provider (`crates/itsanas-drive/src/projfs.rs`) and whether
      a virtualised root can report its own free space
      (`GetDiskFreeSpaceEx` on a ProjFS root answers for the volume); FSRM
      quotas exist only on Windows Server. Linux — project quotas (ext4/XFS
      `prjquota`) need root and a mount option; a loop-mounted image file is
      the portable fallback. macOS — an APFS volume with a quota
      (`diskutil apfs addVolume -quota`). Android — nothing native; the app
      checks. The in-process check of 8.1b comes first on every platform and
      is the one that is enforced; the native quota is presentation.

   o. 🟨 **Reach the network from outside the LAN, with machines that move.** Phases 1 and 2 built (2026-09-29, #182-#187). **Remaining, 🟨:** the daemon's relay wiring has no test of its own (judged not worth two full nodes in the CLI tests; the 2099-clock gap and the v5 test closed 2026-09-30); phase 3 only if the fleet shows 1 and 2 fall short.
      Asked for by Nicolas on 2026-09-17, put on the MVP's path by him on
      2026-09-18 ("I can't get enough machines to work in my own home for a
      decent testing run"), with a constraint he added the same day: **prefer a
      decentralised answer** -- the VM may be a backup and a bookkeeper, but
      clients should call each other, and **nothing whose cost scales per
      machine may become a dependency** (he would accept a Tailscale-equivalent,
      but not one that is expensive at scale).

      **Phase 1 is built (2026-09-18, this PR).** `announce` in the
      configuration and `itsanas announce`, published verbatim with its own
      port; a dual-stack listener, so IPv6 reaches a node at all; a name dialled
      at every address it resolves to; a five-second connect timeout; and a
      lookup that offers the addresses which can work from where the caller
      stands before the ones that cannot. `itsanas_tls::reach` holds the two
      socket decisions, because the coordinator needs both and must not grow a
      dependency on the peer protocol. MVP test O and `BRIEFING-MVP.md` §2.5 are
      the human half. **No wire change.**

      **What phase 1 does not do, and it is the thing to say out loud:** it does
      not make an unreachable machine reachable. It makes a machine that *can*
      be reached say so correctly. Two machines that both move still cannot meet
      without a third party.

      **Verified facts for whoever picks this up**, measured 2026-09-18 from the
      laptop on the home LAN through the public name: `ngas.fr` is 82.67.35.234;
      `:22010` and `:22011` (the SSH forwards) answer, so **the Freebox hairpins
      its forwards** and one name serves inside and outside; `:9898` and `:9797`
      answer nothing, so **nothing reaches ITSaNAS from outside yet**. Every
      node still has `coordinator = 192.168.1.10:9898`. Opening the port and
      switching the nodes is Nicolas's, and `BRIEFING-MVP.md` §2.5 is the
      procedure.

      **Phase 2 -- the decentralised half. 2a and 2b.1 are built
      (2026-09-29); 2b.2 is `NEXT`, specified at the end of this item.** Specified on 2026-09-18
      after Nicolas asked for it in his own words: *"j'aimerais que la vm
      centrale ne soit contactée que si c'est nécessaire, par exemple si
      aucune machine d'un compte n'est connectée"*. Until then every node
      dialled the coordinator **twice per round, unconditionally** --
      `announce` then `peers` -- which at the 300-second default was 576
      connections per node per day whether or not anything needed it. Three
      machines at home, all on one LAN, all finding each other by broadcast,
      still generate every one of them.

      The split to build, and the reasoning for each side:

      **Stays central, and it is small.** (1) *Bootstrap*: a device that knows
      nobody reachable needs one fixed point, and nothing decentralised removes
      that -- it is the same problem a DHT solves with hard-coded seed nodes.
      (2) *Escrow*: recovery from a passphrase needs a server that can rate-limit
      an offline attack, contacted at `login --from` and `register --recovery`
      and at no other time. (3) *Account registration, enrolment, withdrawal*:
      rare, owner-signed, and the coordinator is the thing that makes a username
      unique. (4) *Availability for entitlement*, which is the one that is not
      obviously small -- see the decision below.

      **Becomes decentralised.** Every node keeps a **persistent address book**:
      for each device it cares about -- its account's other devices, its hosts,
      and the guests it hosts for -- the signed presences it has seen, and its
      own record of when it last *successfully dialled* each one. Peers exchange
      those presences over an **appended** peer request (`WantHosted` is the
      model: an older peer answers `Refused`, and the caller reads that as "this
      one cannot tell me" rather than as a failed round).

      **The rule that decides when the VM is dialled at all**, which is the
      actual deliverable -- **and the first version of this rule was wrong in a
      way that would have made machines invisible.** It said the coordinator is
      dialled when a round "reached no peer". Apply that to the parc ROADMAP
      describes, *machines that are usually off*: the Pi wakes, finds the VM by
      LAN broadcast, has therefore reached a peer, and **never publishes its
      address**. A laptop at a friend's house asks the directory and finds
      nothing, or an entry seven days old about to expire. Found by the Rodin
      audit of 2026-09-21.

      The rule confuses two things that today share one connection:
      **publishing** where this machine is, and **reading** where the others
      are. They have opposite economics.

      * **Publishing is what makes a machine findable by somebody who is not
        here**, so it can never be conditioned on having met somebody who is.
        A node announces **at every start and at every change of its published
        address, always**, plus a long backstop so a presence never expires
        under a running node. That is the whole cost: a machine that is always
        on and never moves announces once an hour, not 288 times a day.
      * **Reading is what can be avoided.** `Peers` is asked only when this node
        needs a device it cannot already reach: the address book has no live
        entry for it, or every entry has failed. A fleet at home, all of it
        found by broadcast, reads **never**. *(2a departed from this, see
        below: until gossip exists, a device enrolled after this one started
        is on no list and cannot "go missing", so 2a reads on every
        publication's connection. That costs requests, not connections.)*

      Account events -- register, enrol, withdraw, escrow -- dial as they always
      did; they are rare and they are the point of having a coordinator.

      **What the change is worth, in numbers, because that is the only
      justification for building it.** Today every node makes 288 coordinator
      connections a day whatever happens. After: the Pi, always on and never
      moving, makes about 24. A laptop woken six times a day and changing
      network three times makes about 9 publications plus the reads of a node
      looking for peers it has not met -- call it 15. At the scale Nicolas
      asked about, 3000 machines, the centre goes from **864 000 connections a
      day (10/s) to something under 72 000 (under 1/s)**. If a design does not
      beat that on paper, it is not worth the wire change.

      Constraints, none of them negotiable:

      - **A peer's clock never decides ordering** (§6). For a *relayed* presence
        this is sharper than it looks: the receiver did not observe the
        announcement, only the relay. So do not order by any claimed time at
        all. Keep every candidate address for a device as a **set**, and order
        it by *this machine's own record of which one last worked*. Success is
        the evidence; a timestamp is an opinion.
      - **A relay cannot invent a presence**: `SignedPresence` carries the
        device's own signature, and `Response::Peers` currently throws it away
        (`CoordService::peers_of`, `service.rs` ~405, returns bare
        `Presence`; dispatched at ~300). Carrying the signature
        through is the first change, and it is what makes the gossip safe at all.
      - **Presences are not gossiped to strangers.** An address book handed to
        anyone who authenticates is a map of an account's machines. Exchange
        only with devices of the same account, or with a peer there is already a
        storage relationship with -- the `is_confirmed` notion the neighbourhood
        already has. Bound the table, as discovery's is bounded, because device
        ids are free keypairs.
      - Expect two red-team tests named for their attacks: a peer handing out a
        presence it forged, and a peer replaying a stale one to strand a machine
        at an address it has left.

      **There is no decision to take here, and saying there was cost three
      exchanges.** This item claimed that the per-round announce feeds the
      availability that ECONOMICS turns into entitlement, so that thinning it
      needed Nicolas to choose. That is wrong, and the authority is not that
      nothing calls those functions -- an absent caller is as much an oversight
      as a decision, and a `grep` cannot tell them apart. The authority is that
      **ECONOMICS already decided it**: "A member measures their own, and each
      pair measures each other. A coordinator may still publish a hint; nothing
      depends on it." A future caller of `Directory::tick` or
      `accounting::assess` from the entitlement path is therefore a *bug*, not
      a reason to keep the heartbeat.

      What the backstop is actually for is much smaller: `PRESENCE_TTL` is seven
      days, so an hourly announce leaves 168 chances to refresh before an
      address expires, and `device list` keeps saying how long a machine has
      been silent to within an hour. One unwritten dependency to keep in mind:
      `Response::Peers` orders by `last_seen`, so once everybody announces on
      the same hour that order flattens into the device-id tie-break, and the
      client's own `reachable_first` is what puts usable addresses first. Two
      mechanisms now lean on each other without saying so.

            **The index is built** (2026-09-18, after the measurement that found it):
      claims are kept in a second table keyed by account, so the lookup phase 2
      makes rarer is no longer also the expensive one. What phase 2 still has to
      do is make a healthy round ask for it at all -- the *number* of requests is
      still O(nodes x rounds), which is the part gossip removes.

      **Phase 2a ✅ (2026-09-29): the dial rule.** `itsanas-node/src/contact.rs`
      holds it and its seven tests; `coordinator::contact` and
      `coordinator::address_now` are the socket half, tested against a real
      coordinator in `tests/away_from_home.rs`. Publish at start, on a change
      of the probed address, and every `PUBLISH_EVERY` (1 h); read on every
      publication's connection; pledges refreshed on the same connection.
      Measured by the test that justifies it: 288 rounds make 24 connections.
      The book is in memory, fed only by the coordinator, bounded
      (`MAX_DEVICES` 256, `MAX_ADDRESSES` 4), and orders a device's addresses
      by this machine's `Instant` of last success. No wire change.

      **Phase 2b: signatures, a kept book, gossip.** In this order, each its
      own PR. 1 and 2 are done; **3 is `NEXT`**.

      1. ✅ *Carry the signature through* (2026-09-29, see §0 for what and
         how). Built as specified, except that the check is
         `SignedPresence::verify_origin` -- signer and address, never the
         reader's clock -- and that the fallback to `Peers` closes once the
         coordinator has signed in front of the process (`Due::accept_unsigned`),
         because a hostile coordinator can hang up on purpose. The original
         text: Append `Request::SignedPeers { user }`
         / `Response::SignedPeers(Vec<SignedPresence>)` to the coordinator
         protocol (appended, §6; `red_team_coordinator_messages_keep_their_wire_numbers`
         must stay green). `peers_of` already has the signed rows before it
         strips them. The client verifies each against its device id
         (`SignedPresence::verify`, `claim.rs` ~229) and drops failures; an
         older coordinator closes the connection, so fall back to `Peers` as
         `enrolled` falls back today.
      2. ✅ *Keep the book* (2026-09-29, #184; see §0). Built as below, plus
         success times that never go backward (Rodin). The original text:
         *Keep the book* in `<home>/address-book`: signed
         presences plus this machine's own last-success times as unix seconds
         of *its* clock, read at daemon start, written after a round that
         changed it. A file of its own, for the reason `others-pledged` is one
         (the config parser refuses unknown keys).

         What is there to build on, verified 2026-09-29: `Contact`
         (`crates/itsanas-node/src/contact.rs`) holds `book:
         BTreeMap<DeviceId, Vec<Candidate>>`, a `Candidate` being an address
         and `worked: Option<Instant>`; `signs: bool` is the downgrade memory
         of 2b.1. The daemon builds one `Contact` per process
         (`crates/itsanas-cli/src/daemon.rs`, `one_round`, the block that
         calls `coordinator::contact`). What reaches the book today is
         `(DeviceId, String)`: `coordinator::located` has the
         `SignedPresence` and throws it away after `verified`. So, in order:
         (a) carry the `SignedPresence` from `verified` through `Located` and
         `Contacted::found` into `Candidate` (an unsigned fallback entry has
         none, is dialled, and is never relayed or written); (b) replace
         `Instant` in `Candidate` with unix seconds of this machine's clock,
         compared only with each other -- the one clock that may order this
         machine's own records; (c) write `{ version, signs, entries }` with
         postcard to a temporary file and rename it over `address-book`
         (`others-pledged` is a bare `fs::write`, which a power cut can leave
         empty -- do not copy that), after a round in which `read` or
         `worked` changed anything; (d) at daemon start, load it through the
         same `read` path, so `MAX_DEVICES`, `MAX_ADDRESSES` and
         `verify_origin` apply to the file as to the wire, and treat an
         unreadable file as an empty book with a log line, never as a failed
         start. Red-team tests expected: **a restart does not re-open the
         downgrade** (a book that says the coordinator signs gives
         `accept_unsigned == false` on the first round -- sabotage: do not
         load `signs`); and **an address-book file edited to hold a forged
         presence loses it on load** (sabotage: skip `verify_origin` there).
         Update `ROADMAP.md`'s sentence "Until a restart: that memory is not
         on disk yet" in the same commit.
      3. ✅ *Relay* (2026-09-29, see §0): step (c) built as peer protocol
         6. The plan as it stood: First half ✅ (2026-09-29, #185): the
         coordinator's list carries each owner-signed claim
         (`Request::ClaimedPeers`, `ClaimedPresence::verify_for`), and
         `Contacted::claimed` holds the checked pairs. (a) and (b) ✅
         (2026-09-29, see §0): `Contact::relayable()` exists and survives a
         restart. For (c): the peer protocol is `crates/itsanas-net/src/protocol.rs`
         (`PROTOCOL_VERSION = 5` ~35, `MIN_PROTOCOL_VERSION = 4` ~68; model
         `WantHosted`, served in `service.rs`, sent in `transport.rs`); the daemon's `Contact` is in
         `one_round` (`crates/itsanas-cli/src/daemon.rs`), which is where a
         reply's rows go through `Contact::read`-like insertion with
         `verify_for(node.store.owner())` and the revocation rule below. The
         original plan, in order: (a) give the book `Candidate` a `claim: Option<SignedClaim>` beside
         `presence`, filled from `Contacted::claimed` in `Contact::read`,
         written and re-checked (`verify_for`) by `load`/`save` -- a
         candidate with no claim is never relayed; (b) `Contact::relayable`
         returns `ClaimedPresence`s only; (c) peer protocol 6 as below,
         relaying `ClaimedPresence` not `SignedPresence`, the receiver keeping
         a row only if `verify_for(asker's account)` passes. Red-team test
         expected besides those listed below: **a relay replays a claim made
         before the owner withdrew the device** -- `verify_for` has no date
         and cannot refuse it; the receiver must drop a relayed row whose
         device the coordinator's last `ClaimedPeers` or `Devices` read showed
         withdrawn, or whose `issued_unix` is older than a claim it already
         holds for that device. The original text: *Gossip.* What is there to build on, verified
         2026-09-29: each book `Candidate` holds its `SignedPresence`
         (`contact.rs`, `presence: Option<_>`, `None` = unsigned, never
         relayed); add a `Contact::relayable(owner)` that returns them.
         Start with the claim, as Rodin's finding below says: the
         coordinator protocol needs the owner-signed `SignedClaim` of each
         listed device (`Directory::claim_for`), so `SignedPeers` either
         grows a claim per row (append a new request, §6) or the book asks
         `Devices`. Then peer protocol 6. Peer protocol 6 appends `Request::Presences` (model:
         `WantHosted`; a v5 peer answers `Refused`, read as "cannot tell me").
         Answered only to a device of the same account or one
         `Neighbourhood::is_confirmed`; the answer is the signed presences of
         the asker's account that this node holds. Relayed presences join the
         book as candidates, never displacing one that worked.
         **Found by Rodin on 2026-09-29, and it comes first:** a
         `SignedPresence` proves where a device is, not *whose* it is, so a
         relay could fill the book's 256 places with other accounts' genuine
         presences and a receiver could not tell. Each relayed presence must
         travel with its device's owner-signed `SignedClaim` (the coordinator
         holds it: `Directory::claim_for`), and the receiver keeps it only if
         the claim verifies, is not revoked, and names the account it asked
         about. The coordinator's own list has the same gap and is harmless
         only because `sync_once` decides trust on the connection.
      4. ✅ *Moot* (2026-09-29, see §0): not built, the read stays hourly.
         Skipping it saves a request on a connection publication opens
         anyway, and "a book device unreached" never triggers for a machine
         enrolled later, which no list and no relay would then bring in.
         The original text: make the hourly read conditional: read when a
         book device was reached by no address this round *and* gossip had
         nothing newer. That is where "a fleet at home reads never" becomes
         true. What is there, verified 2026-09-29: `Contact::due`
         (`crates/itsanas-node/src/contact.rs`) sets `read = publish ||
         read_at.is_none()`; publication stays hourly (it is what makes this
         machine findable), so the saving is the read on that connection,
         not the connection. `dial_listed` (`crates/itsanas-cli/src/daemon.rs`)
         knows which book devices answered, and `Contact::relayed` returns
         `Relayed { kept, refused }`. Decide first whether skipping a read
         on an already-open connection is worth anything; if not, item 4
         reduces to "never learn a newly enrolled machine late", and the
         honest move is to mark it moot and go to phase 3 or 0p. Red-team
         test expected if built: **a relay that answers but withholds a
         machine's new address does not suppress the read** (the device
         still unreached must force it). Carry Rodin's 2099-clock finding
         (§0): "newer" from a relay must not be trusted past the reader's
         own clock plus `MAX_CLOCK_SKEW` when that clock is sane.

      Expected red-team tests, one per attack and named for it: a peer
      handing out a presence it forged (sabotage: skip the verify); a peer
      replaying a stale presence to strand a machine at an address it left;
      a stranger asking for an account's presences; a relay passing off
      another account's machine as one of yours; and ✅ a coordinator passing
      off a presence with no valid signature (2b.1: planted, and by pretending
      to be old).

      **Phase 3, only if 1 and 2 fall short: several addresses per device**,
      which is the wire change phase 2 will already have opened the door to
      (`Presence.address` is one string, signed; several means a signed list),
      and then hole punching with the coordinator as a rendezvous rather than a
      relay. IPv6 first at every step: Free provides it natively, it gives
      direct links between houses with no forward, and it is the only option in
      this list whose cost does not grow with the number of machines. Mesh VPNs
      (Nebula, MIT; Headscale with the Tailscale client, BSD) stay what they
      were: a **deployment option** anybody may use, never a dependency of the
      protocol, which already authenticates end to end. Nicolas confirmed that
      reading on 2026-09-18.

   p. 🟨 **Named instances only, each showing its account and storage.** Built 2026-09-30 (#189-#192); left: the service advice `migrate` prints is unexercised on systemd and the Task Scheduler (`ROADMAP.md`, "Named instances (0p)"), a job for 0i's human runs. Asked
      for by Nicolas on 2026-09-16 and again on 2026-09-17: no unnamed default
      instance, launch and list instances by account and storage location, and
      check that the location is reachable (0l provides the check). Today
      `default_home` (`crates/itsanas-node/src/config.rs` ~447) is
      `~/.itsanas`, and `--instance` exists only in the provision scripts, not
      in the CLI. Build `itsanas --instance NAME`, `itsanas instances` (name,
      account, home, folder, reachable or not, daemon running or not), and a
      migration for an existing `~/.itsanas` that names it after its account
      rather than breaking it. Keep 0i's red-team test in mind: cleaning one
      instance must not touch a sibling.

      Verified 2026-09-29: `default_home` is at
      `crates/itsanas-node/src/config.rs` ~560; the CLI's home is the global
      `--home` (`env = "ITSANAS_HOME"`, `crates/itsanas-cli/src/main.rs`
      ~92); `--instance NAME` is parsed only by `install/provision.sh` (~197)
      and `install/clean.sh`. Start by reading how `provision.sh` maps a name
      to a home, and reuse that mapping so the scripts and the CLI agree.
      Red-team tests expected: **a command without `--instance` or `--home`
      refuses rather than using `~/.itsanas`** once the migration has run;
      and **the migration names an existing `~/.itsanas` after its account
      and leaves its data readable** (sabotage: skip the rename, or rename
      without moving the vault).

      **First half done 2026-09-29:** `config::instance_home` and the CLI's
      global `--instance NAME` / `ITSANAS_INSTANCE` (`main.rs` `run()`), tests
      `red_team_an_instance_name_cannot_leave_the_home_directory` and
      `an_instance_lives_where_provision_sh_puts_it`. **(1) and (4) done
      2026-09-30** (`instances_report`, the scripts' `passphrase` refusal; see
      §0). **(2) done 2026-09-30** (`itsanas migrate`, tests
      `red_team_migration_names_the_node_after_its_account_and_keeps_its_data`,
      `red_team_migration_never_lands_on_an_existing_home`). **(3) done 2026-09-30, then Rodin (see §0). 0p is closed.** Kept for reference: (3) --
      in `run()` (`main.rs` ~542), the `(None, None)` arm falls back to
      `config::default_home()`; once `~/.itsanas` holds no node and some
      `~/.itsanas-*` does (i.e. `instances_report` finds named homes only),
      refuse with the list of instances instead of silently creating/opening
      `~/.itsanas`. Keep `init` on a machine with no node at all working
      (first install). Red-team test: after a migration, `status` with no
      `--instance` refuses and names the instance (sabotage: fall back to
      `default_home`). Test through a `base`-taking helper, as
      `migrate_unnamed` does -- never by setting `HOME` in a test. **Then
      Rodin on the whole of 0p.** Original
      plan kept for reference -- (1) `itsanas instances`: glob `~/.itsanas-*` **directories** holding a
      `keystore.bin` (not the `.itsanas-passphrase` file), plus `~/.itsanas`
      itself until migrated; per instance the account (`Config::username`),
      home, `folder` and whether it is reachable (0l's check), and whether a
      daemon runs (the listen port answers, or the pid the daemon writes, if
      any -- check first). (2) The migration: rename `~/.itsanas` to
      `~/.itsanas-<username>`; the vault is inside the home, so a rename moves
      it, but the systemd unit / scheduled task and its `ITSANAS_HOME` point at
      the old path -- the migration must say so or rewrite them. (3) Only then
      the refusal without a name. (4) Make `provision.sh`/`clean.sh`/`.ps1`
      refuse `passphrase` too and exclude the file from their `.itsanas-*`
      globs.

   q. ✅ **First-user friction, from a persona run of `FIRST-STEPS.md`.** Closed 2026-09-30 (#195-#197). Found
      2026-09-30 by a subagent playing a Linux user with two machines (two
      throwaway HOMEs, no coordinator); everything in the goal worked, these
      are the places a person has to guess, worst first:
      (A) `itsanas sync` by hand fetches into the store but the synced
      folder stays empty until `itsanas scan`, which FIRST-STEPS never
      names -- "it appears on the other" is false without the daemon. Fix
      in code: end `sync` with the folder reconcile when a folder is
      configured (read how the daemon's round calls it and reuse that, do
      not duplicate); red-team test: after `sync` fetched a file, it is in
      the folder without a `scan`.
      (B) `login --phrase-file`: the format is unstated, and `init` prints a
      numbered two-column grid that would not parse if pasted. Accept the
      grid as printed (strip `N.` tokens) and say in FIRST-STEPS §1/§4 that
      each machine chooses its own passphrase (`ITSANAS_PASSPHRASE` for
      scripts). Test: the exact text `init` prints parses back to the words.
      (C) FIRST-STEPS §4 `peer add 192.168.1.42:9797` does not say how to
      read machine 1's address and port (9797 is not always it: a second
      node gets 9798); name the command that prints it. And the store-lock
      message says "`itsanas serve` is running" when it is the daemon: say
      "the daemon (or `serve`)" and that CLI writes need it stopped.
      Lesser, for later: `sync` prints "no other machines found yet" and
      then syncs with the added peer; `scan` prints one line per file with
      the summary first; `instances` says "reachable, stopped" which reads
      as a contradiction; FIRST-STEPS never mentions `--instance` or
      `instances`; it mixes `pledge` and `space --apply`.
      **(A), (B), (C) done 2026-09-30 (see §0), and a second persona
      (Windows + Android) folded in the pledge, daemon-stop, both-daemons
      and `--instance` doc fixes and the `USERPROFILE` code fix.** Left,
      one small PR: `sync` exits 0 when every offer was refused (make it
      non-zero, or at least a closing warning); `scan`/`sync` folder lines
      naming files and a rename as one (✅ 2026-10-01, `tests/reports.rs`); `instances` says
      "reachable, stopped" (say "folder reachable; daemon stopped");
      `install/android.md` has no step for entering the laptop's address
      and port, nor Doze / LAN-only notes; install/README "Two accounts"
      does not say where `provision.ps1 -NoInstall` looks for
      `itsanas.exe` for a second *Windows* account.

   r. 🟨 **A test bed in one command per machine, and a visible answer.**
      **Built 2026-10-06** (`install/testbed.sh`, `install/testbed.ps1`, see
      §0 for what the review fixed and for the VM's full disk); three
      hermetic tests in `check-installers.sh`, each sabotaged red: real
      account untouched + earlier bed archived; `--phrase-file` keeps the
      caller's file (sabotage: unconditional `rm`); `clean.sh --instance`
      removes that instance's LaunchAgent and no other (sabotage: drop the
      `rm`). **Left:** the run on real machines, which is the "done when"
      below; the private `Documents\ITSaNAS\TEST-VISUEL.md` has the
      commands per device.
      Asked for by Nicolas on 2026-10-06, ahead of everything else: he wants
      to see it work with his own eyes on Mandarine's Mac, his Windows laptop,
      and if possible the Pi, the VM and an Android phone, about 10 GB each.
      In his words: **one command, two at most, per machine**. The script does
      all the preparation itself: it removes earlier versions, installs what
      is missing, and sets the machine up. Whether it works must be obvious at
      a glance. This is what 0i was missing, and it is what gates 0c.

      **Decided without him (routine; say so in §0, reverse it if he
      objects):**
      - The bed is its own account, instance `essai`, on every machine.
        `nicolas` and `sigseg42` are not touched.
      - Earlier *binaries* are replaced. An earlier `essai` node is
        **archived** (`~/itsanas-archive-DATE/`, as the private reset
        scripts do), never deleted.
      - The coordinator is the fleet's existing one. Its address and id are
        in Nicolas's private `Documents\ITSaNAS\INSTALL-FLOTTE.md`. The
        scripts take them as arguments and hard-code nothing.
      - The first machine creates `essai` with an invitation minted by an
        existing member instance on the same machine (`itsanas invite`,
        `crates/itsanas-cli/src/main.rs` ~412). Every other machine joins the
        same account, so files show up everywhere.
      - The 24 words reach the other machines by a paste at a masked prompt,
        never on a command line (shell history).
      - Each machine generates a random passphrase for its `essai` instance
        and writes it only to the protected passphrase file the service reads
        already (`provision.sh` / `provision.ps1` know where).
      - Pledge 10G and keep 3G: 30/70 allows 4.28G against 10G pledged, and
        the 10 GiB trial covers it in any case.

      **Facts checked 2026-10-06:**
      - `install/provision.sh` handles Linux only. Nothing in it branches on
        Darwin, and macOS has no named instances (README: "its launch agent is
        one"), so **the Mac is the real work**: either `provision.sh` learns
        Darwin, with a launch agent that is loaded, or the Mac gets its own
        path. Decide after reading `install/macos.sh`.
      - `install/provision.ps1 -Instance` and `clean.sh` / `clean.ps1
        --instance` exist (§8 0i).
      - Android has no script: there you install the APK
        (`scripts/build-apk.sh`) and restore from the 24 words in the app. The
        test bed's job there is to print those two steps, not to automate a
        phone.

      **To build:**
      - `install/testbed.sh` (Linux, macOS, Termux) and `install/testbed.ps1`
        (Windows), each a thin wrapper over the existing installers and
        provisioners, never a fork of them. In order: clean binaries (archive
        an old `essai`), install `main`, provision `essai` (found the account
        or join it), start the service. Then drop
        `bonjour-depuis-<hostname>.txt` and a 50 MB random file into
        `~/ITSaNAS-essai`.
      - It ends with a **verdict table**, one line each, ✅ or ❌, every ❌
        followed by its fix:
        - the daemon runs;
        - the coordinator answers;
        - the account's devices, and how many;
        - which other machines' `bonjour-*` files have arrived.
      - `testbed status` reprints the table without changing anything; it is
        the second command.
      - On Windows, the tray icon (0f) gives the same answer in colour.

      **Red-team test expected** (hermetic, in `scripts/check-installers.sh`,
      with the same fakes as 0i): the bed run on a home that holds a
      `nicolas` instance leaves its node, passphrase file and unit untouched,
      and an old `essai` ends up in the archive, not gone. Sabotage: have
      the clean step call `clean.sh` without `--instance` (red), and replace
      the archive with `rm -rf` (red).

      **Done when** Nicolas has run it on Windows and the Mac, and the
      `bonjour` files have crossed both ways. A run on the Pi and the VM over
      SSH (sudo without a password, private guide) may be done by the
      session itself.

   w. 🟨 **A setup wizard and a tray, friendly enough for testers who are
      not us.** Asked for by Nicolas on 2026-10-06, after the live test
      (several machines of one account): a simple step-by-step interface to
      set up the machine, the account, the pledge, updates, registration,
      the secret, a connectivity check and the daemon; a final verification
      that it all worked; then the app lives in the tray, where it opens the
      right folder and offers sign off, sync frequency, pause, sync now, a
      clean removal and account deletion. Put ahead of 0t by him. 0t stays
      the gate for testers outside his circle: every installer builds from
      source today (`windows.ps1` installs Rust and the Visual Studio build
      tools, `macos.sh` waits for Apple's), which no ordinary tester does.
      **Decided by the session, 2026-10-06, reversible until (4) starts:**
      the wizard is a local web page served by the `itsanas` binary itself
      -- `itsanas setup` opens the default browser on
      `http://127.0.0.1:<random port>/` with a one-time token -- the model
      Syncthing has used for years. Why: one interface for Windows, macOS and
      Linux desktops, and a headless Pi through `ssh -L`; no GUI crate,
      where the tray crates are already refused by `cargo deny` (the header
      of `scripts/itsanas-tray.ps1` says why) and a web view would be the
      same fight; and every step testable by plain Rust tests in CI, which
      has no display. Rejected: egui or iced (winit, already refused, and a
      large tree), Tauri (a toolchain of its own), a PowerShell-only wizard
      (Windows only). In order:
      1. ✅ **The control channel.** Built 2026-10-06:
         `itsanas pause|resume|sync-now|interval` write `<home>/control`,
         read by the daemon at most two seconds later
         (`crates/itsanas-cli/src/control.rs`; the home is already the
         boundary of trust, so a file needs no socket and works the same on
         every platform). Paused means no folder scan, no vault drain, no
         peer dialled -- but the coordinator publication and the standing
         check go on, so the machine keeps hosting. That departs from item 1
         of 0f's 2026-09-15 specification ("stop hosting"), on purpose: a
         machine that stops answering fails the audits others' copies rely
         on, and stopping everything is what Disconnect is for. The interval
         is 30 s to 1 day, held there by the daemon whatever the file says.
         A pause is read again before each machine a round dials, so the
         session under way finishes (five minutes at most, #241's budget) and
         no other starts -- Rodin's catch: "after the round" can be hours on
         a first sync, and a pause is for giving the bandwidth back now.
         `status --brief` says `paused` (a stale daemon still says `stale`).
         The Windows tray has Pause/Resume (with the consequence dialog),
         Sync now and Sync every. Red-team, each sabotaged red: the interval
         floor, one request one round, paused never a full round, an
         unreadable file never resuming. Then the review round (the CI
         reviewer, and two adversarial reviewers run by the session): the
         first tests that run the daemon loop at all, `tests/steering.rs` --
         a daemon started paused, a pause landing mid-round, and the
         account's own device pushing into a paused node, each sabotaged red.
         The third caught a real bug of this step: the vault drain's "guard"
         was a `match` arm that ignored the result of a drain that ran
         anyway. Also fixed from that round: the scan that closes a round
         re-reads the pause; an unreadable control file starts the daemon
         paused, is shown `unknown`, and is rewritten only by `pause` or
         `resume` (`interval` over it would have erased a pause); a file
         that grew is read to 4096 bytes, never whole. **Not tested:**
         `halted` stopping further dials (needs a second peer), Windows and
         macOS in CI (`slow-tests` is Linux only, §10 item 13), and the
         tray's new entries, never seen on screen.
      2. 🟨 **The setup engine, and `itsanas setup` in a terminal.** Built
         2026-10-06 in `crates/itsanas-cli/src/setup/`: `mod.rs` (the engine,
         `plan`), `steps.rs` (Machine, Account, Secret, Registration, Pledge,
         Folder, Connectivity, Service, Verify -- each check, apply, remedy),
         `answers.rs` (`--answers` TOML, `--text`), `secrets.rs` (a native
         window per platform fed on stdin, or the terminal), `service.rs`
         (task, unit, LaunchAgent and tray autostart under the installers'
         names), `verify.rs`, `sign.rs` (`signout`/`signin`). Proven by
         tests, each sabotaged red: a second run never remakes the account
         or touches the keystore; no event or `setup.log` line carries a
         recovery word; no secret window's argv or env carries a secret;
         wrong typed-back words write no account; signout forgets the
         passphrase and signin needs the right one; no service for a home it
         would not run; `tests/setup.rs` (ignored, release) runs the binary
         twice. **Not verified:** the macOS/Linux windows and services
         never ran; the Windows window never seen; no service ever really
         installed (only generated text tested); verify against a real
         coordinator; the terminal fallback never driven by hand.
      3. 🟨 **0t's parts 1-3: a release a tester downloads.** Built
         2026-10-06: `crates/itsanas-release` (manifest `itsanas-release 1`,
         Ed25519 over the exact bytes, size + BLAKE3 + SHA-256; signature,
         parse, no downgrade, file -- each refusal proven by
         `tests/release.rs`; `RELEASE_KEY` is `None` and pinned by a test),
         `.github/workflows/release.yml` (v* tag -> 5 targets -> draft;
         Linux on ubuntu-22.04 for glibc reach), `scripts/sign-release.cmd`/
         `.ps1`/`.sh`, `install/get.ps1` / `get.sh` (SHA-256 checked; a
         damaged download refused in `check-installers.sh`),
         `docs/RELEASING.md`. **Not verified:** `release.yml` never ran;
         `sign-release.*` never ran against GitHub; `keygen` never run
         interactively; `get.sh` never on real Linux/macOS, `get.ps1`'s
         success path never executed. Nicolas's key: §10 item 14.
      4. 🟨 **The same engine behind a local web page.** Built 2026-10-06
         (`crates/itsanas-cli/src/setup/web/`, std::net only). Settled: the
         words never go through the browser (the engine asks in `secrets.rs`
         windows; the page only says a window opened), and Settings is a
         short-lived `itsanas settings` process, not the daemon (a pledge,
         folder or coordinator change goes through the engine, which
         restarts the service). Proven by tests, sabotaged red: a rebinding
         Host with the right token refused; no response ever carries a
         recovery word or the passphrase; token, Origin, head and body
         limits, no-store, the window-open state, sign out. **Not verified:**
         a real native window opened from the page's engine thread; the real
         browser launch on any OS; Settings restarting a real service;
         macOS and Linux desktops; joining an existing account through the
         page; layout, contrast and keyboard-only use never seen.
      5. 🟨 **The tray, finished.** Built 2026-10-06: `itsanas pause --for`
         (1 min to 30 days, control key `until`; proven by
         `red_team_a_timed_pause_holds_until_its_end` and, with a real
         daemon, `red_team_a_timed_pause_ends_by_itself_without_resume`);
         trays `scripts/itsanas-tray.ps1`, `itsanas-menubar.js` (JXA),
         `itsanas-tray.py` (AppIndicator) with one menu, held equal by
         `check-installers.sh`; autostart on macOS/Linux through
         `scripts/tray-autostart.sh`. **Not verified:** no tray ever seen on
         screen; the macOS and Linux trays never ran on a desktop; a clock
         jump during a timed pause.
      6. 🟨 **0t's part 4, the self-update.** Built 2026-10-07
         (`crates/itsanas-cli/src/update.rs`): `itsanas update [--check]`; the
         daemon looks once a day (jittered, first look 5-65 min after start)
         under `updates = notify` (default: log + `status` line "update
         available: X") / `auto` (installs, then exits non-zero so its service
         restarts it on the new program) / `off`; the setting is in the node's
         config, set by setup's new "Updates" step, Settings and `--answers`
         (`updates`). Order: no key -> say so, fetch nothing; `target/` build
         or uncovered platform -> never; latest manifest + sig verified by
         `itsanas-release` (newer only); the running exe must match its own
         version's signed manifest (a source build never updates); download
         beside the exe, size + BLAKE3 + SHA-256; exe renamed to `.old`, new
         renamed in, rollback on failure; `itsanas update` restarts the
         service (service.rs stop/start). HTTPS through `curl` (no HTTP client
         in Cargo.lock). Windows tray: `itsanas.ico` (embedded, written beside
         itsanas.exe by setup's service step and provision.ps1) with a state
         dot; web page: `/icon.png` in the header and as favicon. Proven by
         `update/tests.rs` against a fake release in a temp dir, sabotaged red:
         another key, a modified manifest, a downgrade (check and install), a
         hash mismatch, a truncated download, a source build, a failed swap.
         **Not verified:** never run against the real GitHub; no key pinned
         (`RELEASE_KEY = None`), so it installs nothing yet; the rename of a
         running `itsanas.exe` and the restarts by the three services never
         run on a real machine; macOS/Linux trays keep their plain dots; the
         new tray icon never seen.
      7. **Clean removal. NEXT.** from the tray and as `itsanas uninstall`: wraps
         `clean.ps1` / `clean.sh` after a **drain** (§10 item 15, decided
         2026-10-07): stop accepting chunks, hand every hosted chunk to
         another host until each owner's ledger shows another holder or a
         stated timeout passes, then forget the device and delete. Removal
         is never refused: a member is never kept from switching off
         because the network is thin. What is said, before and after, is
         how many owners were left with one replica fewer, never who. Facts
         checked 2026-10-07: a host does not know who else holds a chunk it
         hosts -- the holder ledger is the *owner's* (`itsanas-store`
         `index.rs`, both key orders); so for hosted data "only confirmed
         holder" can only be learnt by asking owners, while for this
         account's own data `status` already computes it (`main.rs`, the
         `unconfirmed` line, `coverage.resting_on_memory()`). Existing
         pieces: `install/clean.ps1` (195 lines), `install/clean.sh` (394),
         `itsanas signout` (`setup/sign.rs:93`) as the model for a tray
         entry with a confirmation (`scripts/itsanas-tray.ps1:119,271`).
         Red-team expected (0f (f)): a machine that is the only confirmed
         holder of a chunk refuses to finish and names the owners affected
         by count, never by name.
      8. **Account deletion.** Needs Nicolas's answer first, as a closed
         question: what becomes of the account's data on other members'
         disks (a tombstone signed by the account, and hosts free it), of
         the data this account's machines host for others (re-homed first,
         as in (7)), and of the username at the coordinator
         (`--forget-account` exists, operator-only). Then a signed request.
   t. **A release that updates itself, signed with Nicolas's key.** Split
      on 2026-10-06 into 0w (3) (parts 1-3, the release) and 0w (6) (part 4,
      the self-update); the text below stays their specification. Asked
      for by Nicolas on 2026-10-06, after the live test needed every fix
      carried to every machine by hand. **Decided the same day (closed
      question): the signing key is Nicolas's, on his PC** -- passphrase-
      protected, with an offline copy; CI builds, Nicolas signs with one
      command, nodes verify. Rejected: a key in GitHub Actions (whoever takes
      the GitHub account owns every member's machine) and building on each
      machine from signed tags (Rust everywhere, 40 min on a Mac, impossible
      on Android).
      Facts checked 2026-10-06: CI uploads one artifact today (ci.yml ~552)
      and publishes no release; the only tag is `v0.1.0`; the workspace is
      version `0.1.0`; `ed25519-dalek` is already a dependency of
      `itsanas-crypto`. **Requirement, Nicolas 2026-10-06: one click,
      user-friendly, both ends.** For him: publishing a version is one
      command or one double-click (a script that asks the passphrase, signs,
      uploads, and says what it published); generating the key once is the
      same, and it says where the offline copy goes. For a member: nothing
      to do -- the daemon updates by itself and says so in the log and the
      tray; `itsanas update` exists for the impatient. An error says what to
      do in one line, never a stack of crypto jargon. Judge the step by
      that, not by the tests alone.
      To build, in this order:
      1. A release manifest: version, and per target (x86_64/aarch64 Linux,
         x86_64 Windows, aarch64/x86_64 macOS) the binary's BLAKE3 hash and
         size; signed Ed25519 by a release key. The public key is compiled
         into the binary (`itsanas-crypto`, a constant pinned by a test, like
         the gear table), and rotation is a signed manifest that names the
         next key.
      2. `scripts/sign-release.*` for Nicolas: generate the key once (sealed
         under a passphrase, the keystore's own scheme), then sign a
         manifest; nothing secret ever reaches the repository or CI.
      3. A `release` workflow on a `v*` tag: build the five targets, attach
         binaries and the unsigned manifest to a GitHub release. Nicolas
         downloads the manifest, signs it, uploads the signature.
      4. `itsanas update` and a daemon check (once a day, jittered): fetch
         the manifest and signature, refuse anything not signed by the pinned
         key, refuse a version not newer than the running one (no downgrade),
         download, verify hash and size, swap the binary by rename (the
         `install/*` rule from #240: never write over a running file), restart
         through the service manager. An install that is not from a release
         (built from source) reports and never updates itself.
      Red-team tests expected: a manifest signed by another key, a valid
      signature over a modified manifest, an older version signed by the
      right key (downgrade), a binary whose hash does not match, a truncated
      download -- each refused with the running binary untouched; sabotage
      each check. First real run doubles as the fleet's reinstall: the
      coordinator on the VM, then the Pi, the VM, the laptop and Mandarine's
      Mac from a signed release, then a second release reaching all of them
      by itself.
   u. **Reachable without port forwards: UPnP/NAT-PMP/PCP and IPv6.**
      Asked for by Nicolas on 2026-10-06 ("make the redirection unnecessary").
      A home node asks its box to open its port and announces what it got;
      a node with a global IPv6 address announces it too. Not planned in
      detail; after 0t. Honest limit, to keep saying: something must be
      reachable, the coordinator at least -- one forward (or one mapping
      opened by UPnP), or a coordinator on a public server.
   v. **A relay for two machines that cannot reach each other** (0o's
      phase 3): through a reachable member, data still sealed end to end.
      After 0u, and only if the fleet shows 0u falls short.
   s. **The vault on another disk (a NAS), the default unchanged.** Asked
      for by Nicolas on 2026-10-06, after growing the VM's disk: the VM is
      meant to host for the network from his NAS later, not from its image.
      He does **not** want it set up yet -- he wants the system *able* to
      run that way, with today's location (`<home>/vault`) staying the
      default, so no existing node moves.

      **Facts checked 2026-10-06 (`main` at 6acd1f4):**
      - The vault's place is hard-wired: `Vault::open(home.join("vault"))`,
        `crates/itsanas-node/src/node.rs` ~645; `main.rs` ~1293 adds up its
        databases from the same `home.join("vault")`.
      - `Vault::open` (`crates/itsanas-store/src/vault.rs` ~281) starts with
        `create_dir_all(&root)` and `Database::create(root.join("vault.redb"))`.
        On a mount that has dropped, that **silently makes a new, empty vault
        on the local disk**: every chunk this node hosts looks lost, audits
        fail, and the owners re-replicate everything -- the vault-side twin of
        the folder bug `itsanas-folder/tests/storage_vanished.rs` guards
        (marker `.itsanas-folder`, `scan.rs` ~87, written and checked in
        `itsanas-folder/src/lib.rs` ~258/276). The vault has no such guard.
      - Free space is measured on the node's home, not the vault's disk:
        `fs4::available_space(&node.home)` at `main.rs` ~2991 and ~3120, and
        `node.rs` ~410. With a vault elsewhere, `space --apply` would accept a
        pledge the NAS cannot hold, or refuse one it can.
      - Moving the whole node with `ITSANAS_HOME` works today but puts the
        keystore and the store's index on the share; not what is wanted.
      - The vault's own index, `vault.redb`, lives inside the vault directory,
        so it moves with it. redb relies on a file lock and on `fsync`; on SMB
        and older NFS both are weaker than on a local disk.

      **To build:**
      - `Config::vault: Option<PathBuf>` (`crates/itsanas-node/src/config.rs`,
        modelled on `folder` ~117); `None` = `<home>/vault`, so nothing
        changes for an existing node. One accessor `Node::vault_path(home,
        &config)`, used by `node.rs` ~645 **and** `main.rs` ~1293.
      - `itsanas vault [PATH]` prints or sets it. Setting it while the current
        vault holds anything is refused, with the manual move spelled out
        (stop the daemon, move the directory, then set the path) -- no
        automatic copy of tens of GB in a first version.
      - A marker `.itsanas-vault` holding the device id, written when a vault
        is created. Opening a vault whose directory exists without the marker,
        or with another device's, or whose configured directory is missing,
        is a **refusal to open**, never a fresh vault: the daemon exits with
        the reason ("the vault at PATH is not there -- is the disk
        mounted?"), and systemd's restart brings it back once the mount
        returns. An existing `<home>/vault` with no marker (every node today)
        gets one written on first open: only a *configured* path is suspect.
      - The three `available_space(&node.home)` calls measure the vault's
        disk for what is pledged and the home's disk for what is kept.
      - Docs: README / FIRST-STEPS (one paragraph, the command and the
        lock caveat: a local disk, iSCSI, or NFSv4 with locking; SMB is not
        supported until measured), ARCHITECTURE, and a line in
        `install/README.md` that a unit needs `RequiresMountsFor=` on the
        mount (provision does not write it in this step).

      **Red-team test expected** (in `itsanas-node` or `itsanas-store`,
      hermetic): a node whose configured vault directory is empty and
      unmarked (the mount dropped) refuses to open and creates nothing
      there; with a marker of another device it refuses too; the default
      `<home>/vault` of an old node opens and gains its marker. Sabotage:
      skip the marker check (red: an empty vault is created), and write
      the marker on every open (red: the dropped mount is accepted).
      A second test: `space --apply` with a vault path measures that path.

      **Done when** a node can run with its vault on another directory,
      refuses a dropped one, and an existing node is untouched -- CI green.
      Not in scope: actually moving the VM's vault to the NAS (Nicolas, later),
      and putting the vault's index on local disk while its blobs live on the
      share (a larger change to `Vault`; decide after measuring redb on the
      NAS that will hold it).

   f. 🟨 **A tray icon for the Windows daemon.** First half built 2026-09-30
      as `status --brief` + `scripts/itsanas-tray.ps1` (see §0 for why not a
      crate); started at logon per node since the same day, through a
      Startup-folder shortcut rather than a task (see §0 for why), removed
      per instance by `clean.ps1 -Instance`, checked in
      `check-installers.sh`. **Pause / resume, Sync now and Sync every are
      in its menu since 2026-10-06 (0w (1))**, and pause keeps hosting -- a
      deliberate change from item 1 of the specification below, said in 0w.
      **Left:** Nicolas sees the icon once and says what it gets wrong; then
      disconnect and decommission, each behind its confirmation, now planned
      as 0w (5) and (7). The text below stays the specification for them. Original
      plan, kept for reference: do it in two PRs. First, alone: pick the crates (`tray-icon` + `tao`/`winit`,
      or `windows`-crate `Shell_NotifyIcon` directly) by running `cargo deny
      check` with them added and confirming `scripts/check-unsafe.py` still
      passes (unsafe inside dependencies is allowed, in this workspace not);
      write the snapshot reader (`status.snapshot` -> state + age) as a pure,
      tested function in a new `itsanas-tray` crate, with the red-team test
      below (a snapshot older than two intervals is stale, never healthy).
      Second: the icon, left-click, right-click menu with the confirmations
      Nicolas specified. Build it only on Windows (`cfg(windows)`), so the
      other CI targets stay unaffected. Asked for by Nicolas on
      2026-09-14 after the untitled console: "a minimum of polish", dark or
      following the system theme. Verified facts: no desktop UI exists
      (ROADMAP.md, the table of deferred work, "Tray / desktop GUI"); the
      daemon already writes `status.snapshot`, which `itsanas status` prints
      while the daemon holds the index (`crates/itsanas-cli/src/main.rs`
      ~1180); the index lock means a second process cannot read the store
      live (§9, "One process per node"). So the tray reads the snapshot and
      nothing else: state and age of the last round, open the synced folder,
      open the log, restart the task, quit. A new crate (`itsanas-tray`,
      Windows first); candidate crates `tray-icon` with `tao` or `winit`,
      **not yet checked against `cargo deny`** or the unsafe gate, which must
      stay satisfied (dependencies may use unsafe; this workspace may not).
      Red-team test expected: a snapshot older than two intervals is shown as
      stale, never as healthy -- a green icon over a dead daemon is the failure
      the tray exists to prevent. A file list, login and account switching are
      later: switching is a different `ITSANAS_HOME` and works today; a live
      file list needs the local control socket first.

      **Nicolas's specification, 2026-09-15, "like Google Drive":**
      - *Left click* opens Explorer on the account's files: the synced folder
        (`config.folder`) where one is set; otherwise the ProjFS drive
        (`itsanas-drive`, read-only today) if it is mounted; otherwise say that
        no folder is configured and offer `itsanas folder`. One icon per node
        home, so two accounts on one machine (0h) show two icons, each named.
      - *Right click*, every destructive entry behind a confirmation dialog
        that states its consequences in plain words, with Confirm and Cancel:
        1. **Pause / resume syncing** — nothing is lost; hosts keep your data;
           you stop receiving changes and hosting until resumed.
        2. **Disconnect** (sign out of this machine) — stops the daemon and
           removes the passphrase file the task reads, so nothing starts at
           logon; the keystore, store and hosted data stay, and signing back
           in needs the passphrase. Say that others' data stays on this disk
           and the machine stops being audited-healthy for them while off.
        3. **Quit** — stops the daemon until the next logon or manual start.
        4. **Decommission this machine** — frees the space. Consequences to
           state: this device is withdrawn from the account for good (§8 0g:
           final), its local copy of your files is deleted, and **other
           people's data it hosts is released only after it is re-homed** —
           deleting hosted chunks outright silently drops somebody's replica
           count. (ECONOMICS.md §5 forbids deletion *as a sanction*; a member
           leaving is not one, so the reason is the other members' durability,
           not §5.) The Rodin audit's cheaper alternative, to decide with
           Nicolas first: refuse to decommission while any hosted chunk has no
           other confirmed holder, instead of building a drain.
           Needs a drain that does not exist yet: refuse new hosting, push
           or hand off every hosted chunk until each owner's ledger shows
           another holder (or a stated timeout), then `device forget`, then
           delete the home. The dialog shows the estimate (bytes hosted,
           upload speed) and warns if this is the account's last machine
           holding something no host has confirmed (`status` already knows).
      Red-team expected on decommission: a machine that is the only confirmed
      holder of a chunk refuses to finish, and says which owner is affected
      by count, never by name.

   g. ✅ **Accounts and devices, red-teamed and repaired.** Asked for by
      Nicolas on 2026-09-15 as a detour before 0f. See §0 and ROADMAP.md,
      "The identity surface, examined 2026-09-15". Left open on purpose:
      identity rotation (a stolen node with its passphrase is the account),
      withdrawal reaching the peer protocol (belongs with 1(c)'s attribution
      through `NodeClaim`), dropping the device seed from the escrow container,
      and a per-device name in `device list` (a claim field would change a
      signed payload; the list shows address, pledge and silence instead).
      From the Rodin audit, also open: `COORD_VERSION` stayed 1, so the CLI
      detects `Request::Devices` support by a closed connection and cannot tell
      it from a timeout — a capability list in `Welcome` is the fix, and
      appending a field there is itself a wire change to pin; after
      `itsanas passphrase` nothing records that a lodged escrow container is
      still under the old passphrase, so recovery can fail months later — a
      line in `status` would say it; and several accounts on one machine
      (needed by BRIEFING-MVP.md for B/E/F/G on a mixed fleet) still collide on
      the discovery port.

   h. ✅ **Two accounts on one machine, then the full fleet protocol.** Built
      2026-09-15 on branch `multi-instance`; see §0. Asked for
      by Nicolas on 2026-09-15, ahead of 0f, to test on his three machines
      the same week. Verified blockers: `Lan::bind` takes UDP 21037 with no
      address reuse (`crates/itsanas-discover/src/lan.rs` ~114; a test near
      line 349 asserts a second bind fails), so a second node on a machine has
      no discovery; `provision.ps1` names one task `ITSaNAS` (~158) and one
      passphrase file under `%LOCALAPPDATA%\itsanas`; `provision.sh` writes one
      `itsanas.service` (~433); every node defaults to listen port 9797.
      Build: discovery shared between instances (`SO_REUSEADDR`, and
      `SO_REUSEPORT` where it exists, through a small dependency that must pass
      `cargo deny` — broadcasts reach every bound socket; a unicast reply does
      not, so check which the beacon uses); named instances in both
      provisioners (`itsanas@<name>` unit, `ITSaNAS-<name>` task, passphrase
      file per instance), a free listen port chosen at `init`/`login` when
      9797 is taken; an acceptance-local scenario with two accounts on one
      host that find each other by discovery and host each other blind.
      Red-team expected: a second instance's beacons do not let it answer as
      the first (device pinning already covers it; prove it with both bound).
      Then rewrite `docs/BRIEFING-MVP.md` as the full protocol: A–M with the
      verdict rule, 1a and 1b, the measurements that bear on scale (throughput,
      idle writes, restore time, battery), and a section on what three machines
      of one person cannot show — strangers, NAT, bandwidth, a terabyte, the
      bargain enforced only locally — so a green run is not read as "viable
      like Storj". Android: the only APK is v0.1.0 debug-signed and stale; a
      phone test needs a fresh build (`scripts/build-apk.sh`). macOS: source
      install only.

1. **Enforce the space split. Asked for by Nicolas on 2026-09-14.** Step (a) is
   built; (b), (c) and (d) are what is left, and **nothing on
   the network enforces the split yet**. Verified facts:

   - The split is `itsanas-coord::accounting::Split`, and its default is
     **30/70** — keep three parts of every ten a machine commits. It replaced
     `CONTRIBUTION_RATIO = 3`, a **25/75** split, on 2026-09-14, on Nicolas's
     decision. The reasoning, so nobody re-derives it: `REPLICATION_TARGET = 3`
     counts *machines*, including the owner's own (`store/src/lib.rs`,
     `holders.rs`). For data the owner keeps locally the network holds **two**
     copies, so the break-even ratio is 2 (33/67) and 30/70 (7/3) leaves about
     17 % slack for machines that are asleep and for sealing and log overhead.
     33/67 leaves none. For data a device has *released* (a phone under `keep`),
     the network needs three copies and only 25/75 breaks even — so the value may
     later need to depend on how much of an account is kept locally. That
     derivation is now in `ECONOMICS.md` §1, which used to state capacity as
     `R × S` with the owner's copy wrongly counted as network capacity.
   - **Two splits exist.** `Config::split` — a `split = 30/70` line in the node
     file — governs what *this machine* refuses its own owner, and is read by the
     CLI `keep` and `space`. It may only be *stricter* than `Split::DEFAULT`; a
     more generous one is refused when the file is read. `Assessment::split` is the *coordinator's*, and is
     what `assess` grants entitlement by. Nothing a device sends may reach the
     second; `DeviceContribution` deliberately carries no split.
   - `itsanas pledge` and the Android JNI `setKeep`/`setPledge` consult the
     split since 2026-09-30 (item 3a below, `Node::check_split`). The coordinator
     claim (`crates/itsanas-cli/src/coordinator.rs:180`) and the daemon's
     `Pledge` (`crates/itsanas-cli/src/daemon.rs`) carry `pledge_bytes` and never
     read a split, which is correct.
   - **Writing consults it since 2026-09-28** (0n): a write past what the
     pledge earns is refused, on the honest client. Before, `write_stream` and
     the folder import accepted any amount.
   - **Hosts bound themselves, not owners** -- until 2026-10-04, see 1c
     below and §0. The original text: `would_exceed_pledge`
     (`crates/itsanas-net/src/service.rs`) stops a host exceeding its own
     pledge; nothing limits what one owner stores on a host, so a rebuilt client
     that pledges nothing is served until every host is full.
   - `accounting::assess()` and the coordinator's usage path run only in tests.

   Build in this order, each step with a red-team test that is sabotage-verified:

   a. ✅ **The split as a value.** Built 2026-09-14; see §0 for what the PR
      carries beyond what follows.
      `Split { own, network }` in `accounting.rs`: `DEFAULT` of 30/70,
      `new`/`parse` refusing a zero part, `room_earned` and `pledge_needed_for`
      as methods over `u128` intermediates (`pledge_needed_for` rounds **up**,
      or the quote names a figure that is refused when supplied),
      `Assessment::split`, a `split = 30/70` line in the node configuration
      file, and `keep`/`space` reading it. Six new tests, three of them
      red-team. `CONTRIBUTION_RATIO` is gone rather than deprecated, so a new
      call site cannot bypass the config field.

      This was **not** the inert refactor the old text of this step described.
      That text said "no behaviour change; tests prove the default equals
      today's numbers" while the bullet above it recorded a decision to default
      to 30/70, and the two cannot both hold: at 30/70 an always-on node
      pledging 300 GB earns 128 GB where it earned 100. Nicolas chose 30/70 on
      2026-09-14 when the contradiction was put to him, so the fixtures in
      `accounting.rs` moved to figures the split divides exactly (pledge 700,
      earn 300) rather than being left to assert the old arithmetic.

      **To sabotage-verify, one at a time:** drop `+ u128::from(needed % own != 0)`
      from `pledge_needed_for` and
      `the_limit_and_the_price_quoted_for_exceeding_it_never_contradict` must go
      red; drop the `own == 0 || network == 0` guard from `Split::new` and
      `red_team_a_split_with_a_zero_part_is_refused_rather_than_dividing_by_zero`
      must go red; make `assess` read `Split::DEFAULT` instead of `input.split`
      and `red_team_entitlement_follows_the_coordinator_s_split_not_a_device_s`
      must go red. A test that passes both ways is decorative and not accepted.
      All went red on 2026-09-14, as did the two tests the audit added
      (`red_team_a_node_cannot_grant_itself_a_more_generous_split`,
      `a_quoted_price_parses_back_to_no_less_than_the_price`).
   b. ✅ **Bound writes on the honest client.** Disk half built 2026-09-30, pull half the same day (see §0); what the pull bound leaves open is named in `ROADMAP.md`, "The disk bound (1b) on pulls". The account half is built
      (§8 0n, 2026-09-28); **left: the disk half** -- refuse when this
      machine's own store plus what its pledge still has to receive would
      exceed the disk. It wants the vault's size, which the store cannot see,
      so it belongs in `Node::bound_writes` beside the vault walk, using
      `fs4::available_space` as `space` does. **Correction to the original
      text below:** the bound is what the *account's* pledges earn, not this
      machine's -- see §0, 2026-09-28. The original text:
      `Store::write_stream` and
      `write_file` (`crates/itsanas-store/src/store.rs`, ~246 and ~310) and the
      folder import (`crates/itsanas-folder/src/lib.rs`, ~259) refuse when the
      account's bytes plus the incoming file exceed what the pledge earns —
      `config.split.room_earned(pledge).max(JOINING_ALLOWANCE)`, exactly the
      rule `keep` applies in `crates/itsanas-cli/src/main.rs` ~1773 — and when
      this machine's own store plus its pledge would exceed the disk. The
      error names the numbers in the wording `itsanas space` uses, with any
      price through `size_argument`.

      **Not verified yet; find these before writing:** where an account's
      total bytes are already counted (if nowhere, that is the first piece of
      work), and the crate dependency direction — the rule lives in
      `itsanas-coord` and the split in `itsanas-node`'s `Config`, so the limit
      most likely enters the store as a parameter rather than being read
      there. `keep` ignores the thirty-day clock and applies the allowance
      unconditionally; decide whether writes should do the same, and say so.

      Red-team test expected: a write that would take the account past what
      its pledge earns is refused **and leaves nothing behind** — no chunk,
      no index entry, no log segment. Sabotage by removing the check; a
      second test that a write inside the limit still succeeds keeps the
      first from passing on a store that refuses everything.
   c. ✅ **Bound owners on the host — the part a rebuilt client cannot delete.**
      **First part ✅ 2026-10-04 (see §0). (i) ✅ 2026-10-05. (ii) ✅
      2026-10-05 (see §0).** **(iii) judged not worth its table, 2026-10-05.**
      Security, nothing: since (ii) an empty book confirms nobody, so a
      restart fails closed, and a withdrawal it forgot is heard again at the
      first question (with the coordinator down, no other account stores
      anyway). Availability, minutes: other accounts' devices are refused
      until re-asked -- 30 a minute inline plus 64 a round, about five
      minutes for 200 -- and retry on their next round. The fleet is three
      instances of one account, so today that is nothing. Smaller points:
      a Pi rebooted with no RTC reads 1970 until NTP, so what the table
      would hold has to count as lapsed there (it would help after an
      update, not a reboot); and it is one more persisted state for
      `Index::open` to read and trust. Left open with it (Rodin): holding
      standings as wall-clock time is also what would end the ROADMAP
      ceiling that `Instant` ignores a suspended host's sleep -- that one
      does not need the table and stays open. Reopen (iii) if a host's
      logs show those minutes mattering; the plan stays below.
      The (iii) plan as it stood: the book is a process
      static in `daemon.rs::owners` and a local in `main.rs::serve`; a
      restart empties it. Since (ii) that is no longer a hole -- an empty
      book confirms nobody, so nobody stores until re-asked -- but a cost: a
      host serving 200 devices of other accounts takes about five minutes
      (30 inline a minute plus 64 a round) to re-confirm them, refusing
      meanwhile, and forgets
      every withdrawal it heard. Keep `Held` (owner, pledged, issued, claim
      bytes, standing as a unix time, not an `Instant`) in a table of the
      node's index (`itsanas-store/src/index.rs`, beside `peer_full`),
      written on `take`/`note`, read on open; `MAX_CLAIMS` bounds it. Red-team
      test: a host reopened from disk refuses a withdrawn device without
      asking (sabotage: do not read the table); and a confirmation read back
      still lapses after `STANDING_FOR` measured on the wall clock (decide
      what a clock that went backwards does -- the cautious side is "lapsed").
      If (iii) is judged not worth its table, say so here and move to (d).
      The (ii) plan as it stood, **done:** `ClaimBook::take` (`owners.rs`) accepts any live
      claim the account key signed, and every node holds that key, so a device
      its owner withdrew re-signs and stores. The coordinator's withdrawals
      already reach the daemon: `coordinator::contact` returns
      `contacted.claimed` (`ClaimedPeers` rows, checked by `verify_for` in
      `crates/itsanas-cli/src/contact.rs`), read into `Contact` in
      `daemon.rs::one_round` -- but only for *this* account's devices. A host
      needs other accounts' withdrawals: find whether the coordinator can be
      asked per device (a `CoordRequest` that exists, or one to append --
      §6, appended never inserted), cache the answer in the book, and refuse
      in `take`/`admits`. Red-team test: a device the coordinator lists as
      withdrawn stores nothing even with a fresh live claim (sabotage: skip
      the list); and a coordinator that is down must not refuse every
      device (fail open, stated, or closed, decided -- if it is a judgement
      call, it is Nicolas's). Then (iii).
      The original list, each with a
      red-team test: (i) **test the space a claim pledges** -- today a pledge
      is believed until this host's own audits pause a device, and a host
      only audits devices it pushed to; give the host a way to contradict a
      claim it has not tested (offer the claiming device chunks of its own
      when the account's held bytes pass what proof earns, and treat a
      `PledgeFull` refusal while the claim shows room as a contradiction --
      `Store::note_peer_full` already records refusals); (ii) read the
      coordinator's withdrawals (`ClaimedPeers` rows are already checked by
      `verify_for`, `contact.rs`) so a withdrawn device that re-signs is
      refused; (iii) keep the book on disk, or a restart re-opens every
      account's share until it presents again. Expected red-team tests: a
      claim of a terabyte from a device that refuses a gigabyte loses its
      credit (sabotage: ignore `PledgeFull`); a device the coordinator lists
      as withdrawn stores nothing (sabotage: skip the list). The plan as it
      stood:
      In `service.rs` `StoreChunk` and `StoreSegment`: a host stores for owner O
      at most an allowance plus `k ×` the bytes of this host's own data that O's
      devices have **proved** they hold (a passed storage challenge, as
      `Store::release` already requires). Attribute a device to its owner through
      the owner-signed `NodeClaim`, never the unauthenticated `Hello` field. Tests:
      a peer hosting nothing is refused past the allowance; a peer that hosts and
      passes audits keeps being served.
      What is there, verified 2026-10-04: the gate is `would_exceed_pledge`
      in `StoreChunk` / `StoreSegment` (`crates/itsanas-net/src/service.rs`,
      `handle`, ~180-215), which knows the caller's `DeviceId` (TLS) and
      nothing about its owner; `PeerService` already takes an injected
      `Relay` for presences (8.0o 2b.3), the model for handing it the
      daemon's checked `NodeClaim`s; proof of hosting lives in this host's
      own ledger (`Store::holder_evidence`, `HolderEvidence::proved`, and
      `Store::reliability`). Decide before code: whether a device with no
      claim this host can check gets the allowance or nothing (the bargain
      is a §6 decision, so the merge goes to Nicolas).
   d. ✅ `ECONOMICS.md` §1 back to ✅ built when (c) lands. Done 2026-10-05,
      with what a rebuilt client still gets named under the heading. Its §8 constants row, the catalogue rows and the counts in
      README, ROADMAP and TESTING were all done in (a).

2. **Finish the red team, one surface per session, by hand.** Three surfaces have
   never been examined — every multi-agent attempt died on usage limits:
   *integrity* (a hostile peer: forged or replayed segments, version vectors that
   win or resurrect deletions, chunks whose id does not match their bytes,
   downgrade past a later defence), *confidentiality* (convergent ciphertext, what
   two hosts learn by comparing notes), *identity* (many devices, claiming someone
   else's device, LAN discovery eclipse). Git history was checked for secrets on
   2026-09-14 and is clean. *Identity* was examined by hand on 2026-09-15
   (ROADMAP, "The identity surface, examined 2026-09-15"; §8 0g); integrity on
   2026-09-30 (§8 2a); confidentiality on 2026-09-30 (§8 2b).

   a. ✅ **Integrity, by hand: what a hostile peer can make this node believe.**
      Done 2026-09-30 (see §0; ROADMAP "The integrity surface, by hand" has the
      table). Open from it: the lifted-body re-signing (protocol change, the
      device id into `SealContext` for new segments or an account device
      roster), a hostile host stalling a round with any refused segment, and the
      two `itsanas-redteam` residues. Original text follows.
      Chosen 2026-09-30 as `NEXT` because every other open §8 item is
      host-side enforcement (1c, deferred by 0), or waits on Nicolas (0c, 0f's
      menu, 0i, 0k, 3c, 5). Not yet read for this: start from where a pulled
      segment is opened and applied -- `crates/itsanas-store/src/oplog.rs`
      (segments, `VersionVector`, ~66-125) and the merge in
      `crates/itsanas-store/src/catalogue.rs` (`CausalOrder`, ~64-93) -- and
      where a pulled chunk is checked against its address
      (`UserKeys::open_chunk`, `crates/itsanas-crypto/src/identity.rs`).
      Attacks to try, each as a red-team test that fails today or a named
      reason it cannot: a segment replayed from an older state; a version
      vector that wins over, or resurrects, a deletion; a chunk whose bytes do
      not match its id; a peer serving a prefix (compare §9 "Tail
      truncation", deliberately open -- do not re-find it). One finding per
      PR; the survey's list goes in ROADMAP "What an adversarial sweep found".

   b. ✅ **Confidentiality, by hand: what a host learns, alone or comparing notes
      with another.** Built 2026-09-30 (see §0; ROADMAP "The confidentiality
      surface, by hand" has the table). Open from it: reads not scoped to the
      account (§10 question 7, option 2 would close it). Original text follows.
      Next because it is the last unexamined §8 2 surface and
      every other open item is host-side (1c) or waits on Nicolas. Not yet read
      for this. Start from the plaintext a host sees: `SegmentEnvelope`
      (`crates/itsanas-store/src/oplog.rs` ~160: owner, device, sequences,
      `previous`, body length), the blinded chunk address and the deterministic
      seal (`UserKeys::chunk_id` / `seal_chunk`,
      `crates/itsanas-crypto/src/identity.rs`, `seal.rs`), and what the wire
      carries in the clear (`crates/itsanas-wire`, have/missing exchanges in
      `crates/itsanas-net/src/session.rs`). Already named, do not re-find: the
      chunk-size fingerprint (ROADMAP, same section), the device id in the LAN
      beacon, blinded addressing's purpose. Questions to answer, each with a
      red-team test that fails today or a named reason: can two hosts of
      different accounts tell that they hold the same plaintext (convergent
      ciphertext across accounts); can one host link two accounts' devices;
      does a segment's size or cadence reveal file count or edit size; does
      any error message or log line a peer can provoke echo a path. Cap: ~5
      tests, one PR.

   d. ✅ **At most 5 live devices per account.** Built 2026-10-01 (see §0;
      ROADMAP "At most 5 live devices per account"). Decided by Nicolas on
      2026-09-30, verbatim: « Il faut limiter le nombre d'appareils par
      compte. 5 max me semble bien. » Lettered d and placed before c because
      it was decided after c was written and went first. Open from it: a
      withdraw in the Android app; a cap on withdrawn rows (5 live, any
      number withdrawn); the re-sign corner named in ROADMAP.

   c. ✅ **A hostile host cannot stall a whole pull with one refused segment.**
      Built 2026-10-01 on `pull_scoped` only (see §0; ROADMAP "One refused
      chain no longer stalls a pull" lists what is left). Original text
      follows. Named by 2a (ROADMAP "The integrity surface, by hand", "Not examined or
      not tested here"): one segment that fails any check -- signature,
      `open`, `validate_chain` -- errors the whole pull, so one bad host
      stops a round that honest segments from other devices' chains could
      finish. Client-side, not enforcement, needs nobody. Not yet read for
      this: where the pull in `crates/itsanas-net/src/session.rs` collects
      segments per device and hands them to `itsanas_sync::apply_replaying`
      (and `validate_chain` in `crates/itsanas-store/src/oplog.rs`). Shape
      expected: refuse that device's chain for this round, keep the others,
      report it. Red-team test: a peer serving one forged segment on device
      A's chain and genuine ones on device B's; B's file is adopted, A's is
      deferred, the round says so; sabotage by propagating the error again.
      Do not hide the failure: a chain refused must still be visible.

   e. ✅ **The Android app can withdraw a device.** Built 2026-10-01 (see §0;
      ROADMAP "At most 5 live devices per account"). Open from it: the
      Kotlin screens untested; the node lock held across the dials of
      `withdrawDevice`; `complete` is not false when the coordinator's list
      is truncated at `MAX_PEERS_RETURNED`. Original text follows.
      Open from 2d: at the cap
      (`MAX_DEVICES_PER_ACCOUNT` = 5, `crates/itsanas-coord/src/claim.rs`
      ~106) the refusal tells the person to run `itsanas device forget <id>`
      (`crates/itsanas-node/src/coordinator.rs` ~257), and an account whose
      machines are all phones has no CLI. The withdrawal itself is already
      in `itsanas-node`: `coordinator::forget_device(node, device, now)`
      (~1030), which the CLI's `device forget` (`crates/itsanas-cli/src/main.rs`
      ~2221) calls. Missing: a `Java_fr_ngas_itsanas_Native_forgetDevice`
      beside `register` (`crates/itsanas-android/src/lib.rs` ~720) taking a
      full device id, a way for the app to list the account's devices with
      full ids (not yet read: whether `status` or `register`'s refusal already
      carries them), the Kotlin button under `android/app`, and the refusal
      text naming the app on Android. Red-team test expected: the native call
      refuses an abbreviated or unknown id and withdraws nothing; sabotage by
      accepting a prefix. Client-side, needs nobody but a phone for the by-hand
      check (§8 5).
3. **The open findings** listed in ROADMAP.md, one per session.

   a. ✅ **`pledge` and the Android setters keep the split.** Built
      2026-09-30 as written (see §0); the original text follows. Verified
      2026-09-30: `pledge` (`crates/itsanas-cli/src/main.rs` ~3033) checks the
      free disk but not that the configured `keep_bytes` still fits the new
      pledge -- `keep` does (~2800, `Node::allowed_for` and
      `split.pledge_needed_for`), so `keep 70G` then `pledge 1G` leaves a
      node keeping far more than it earns, found by nobody until a
      coordinator refuses it. It also reads `held` as `vault.stats()?.bytes`,
      which counts our own account's chunks -- the bug 1b fixed with
      `Node::held_for_others` (`crates/itsanas-node/src/node.rs` ~355, today
      private). `Java_fr_ngas_itsanas_Native_setPledge` and `setKeep`
      (`crates/itsanas-android/src/lib.rs` ~579, ~619) check neither. Put the
      rule in one place in `itsanas-node` (a `Node` method returning the
      refusal) and call it from the three setters; keep the message's
      `space --pledge .. --keep .. --apply` hint. Red-team test expected: a
      pledge lowered under what the current keep needs is refused and the
      config is unchanged; sabotage by skipping the check. Not host-side
      enforcement: a rebuilt client skips it, which is 1c's job, deferred by
      §8 0.

   b. ✅ **A store the host will refuse costs no walk of its vault.** Built
      2026-09-30 (see §0); the original text follows. `would_exceed_pledge` (`crates/itsanas-net/src/service.rs` ~445) reads
      `Vault::stats()` (`crates/itsanas-store/src/vault.rs` ~586), which lists
      every owner's blobs and stats each file, under the storing lock -- so a
      peer spamming offers it knows are refused delays honest stores. Segment
      bytes are already a running total (`CHAIN_BYTES`); keep one for chunk
      bytes beside `vault_chunks` (`CHUNKS`, ~83), updated in the same
      transaction as each put and delete, and read it here. Measure first
      (ROADMAP, "A refused request still costs the host a full walk"): time a
      refused `StoreChunk` on a vault of many chunks, before and after.
      Red-team test expected: the total equals the sum over the blobs after
      puts, a re-put of the same address and deletes; sabotage by skipping the
      update on delete. Not enforcement: same rule, cheaper to ask.
   c. Chunk-size sequences fingerprint files (ROADMAP). **Keyed
      chunking, decided 2026-10-06** (§10 item 9). Padding was measured
      first and does not close it at any sane cost (§10 item 9, the table;
      `crates/itsanas-store/examples/padding_cost.rs` re-takes it). To build,
      in this order:
      1. **Read before coding.** Published work attacks keyed content-defined
         chunkers (Borg, Restic, Tarsnap and others; look for "Breaking and
         Fixing Content-Defined Chunking", 2025, and what it recommends).
         Not read yet -- the title and scope are from memory, verify them.
         If it shows a Gear table keyed this way is recoverable by a host
         from what it sees alone (sizes of chunks it stores, no chosen
         plaintext), stop and take that back to Nicolas before any code.
      2. **Facts already checked (2026-10-06).** The table is the global
         `GEAR` (`crates/itsanas-store/src/chunker.rs` ~30), derived from
         the public `GEAR_DOMAIN`, pinned by `the_gear_table_is_pinned_forever`.
         `ChunkerConfig` is `Copy` and carries no table; it is built with
         `ChunkerConfig::default()` in `Store::open` (`store.rs` ~228) and in
         tests/bench (`itsanas-cli/src/bench.rs`, `itsanas-net`,
         `itsanas-node`, `itsanas-folder` tests). Chunk addresses are already
         per account (`UserKeys::chunk_id`, `identity.rs` ~335, keyed on
         `blinding`), so a per-account table costs **no** deduplication:
         there is none across accounts today.
      3. **Build.** A new KDF context in `itsanas-crypto` (beside
         `CTX_USER_*`) gives `UserKeys` a chunking secret; the table is 256
         words from `blake3` keyed on it, so every device of one account cuts
         identically and no other account can. `ChunkerConfig` takes the
         table (an `Arc<[u64; 256]>`, since it stops being `Copy`), and the
         public table stays for tests and the bench only. Files already
         stored keep their chunks: a file is re-cut with the new table only
         when it is next written, so its first rewrite re-uploads it once and
         **old files stay recognisable until then** -- say so in ROADMAP; a
         forced re-cut of everything is not part of this step.
      4. **Red-team tests expected.** Two accounts cutting the same 4 MiB file
         produce different size sequences, and two devices of one account
         the same; a host cutting its candidate copy with the public table
         reproduces none of the account's sizes. Sabotage: the public table
         for every account (red on the first and third), a table keyed on the
         device instead of the account (red on the second). Plus the pinned
         public table must still pass: tests depend on it.
      5. **Docs.** ROADMAP's entry and its table row move to ✅ with the
         ceilings named (old files until rewritten, total file length still
         visible, whatever step 1 found); DESIGN says what blinded
         addressing and keyed chunking each hide.
   d. ✅ **The LAN beacon stops grouping an account's machines.** Built
      2026-09-30 (see §0). **Corrected:** this item said a v1 beacon "heard
      as a stranger is safe". It was not, and no test ran it: `parse`
      refused any version other than `BEACON_VERSION`, so a plain bump would
      have made v1 and v2 machines blind to each other during an upgrade.
      Built instead: `parse` reads 1 and 2, v1 comes back `OwnerTag::Legacy`
      (dialled, never "mine"), `seal` writes 2 only: a per-beacon nonce and
      a hash keyed on the account over nonce and device, no clock. The original text follows. Checked
      2026-09-30: `owner_tag` (`crates/itsanas-discover/src/beacon.rs` ~87)
      is `blake3::derive_key(OWNER_TAG_DOMAIN, user_id)`, the same 32 bytes
      from every machine of an account for ever, so a listener groups them and
      anyone holding a user id recognises it (ROADMAP, "The LAN beacon groups
      an account's machines"). Not by clock rotation: the comment there and
      §6 ("the sender's clock decides nothing in discovery") rule it out, a
      Pi 4 boots in 1970. Instead split the 32-byte field into a fresh random
      16-byte nonce and a 16-byte `keyed_hash(account key, nonce)`, where the
      key is derived from the account's `UserKeys` (so only its own machines
      can compute it), and bump `BEACON_VERSION` to 2. Readers of the tag:
      `crates/itsanas-cli/src/discovery.rs` ~212 (`mine`) and the dial order
      in `crates/itsanas-discover/src/neighbours.rs` (`dial_order(owner)`,
      own machines first) -- both must take the key, not a user id. Decide
      and write down what a v1 beacon from a not-yet-upgraded machine does
      (see the correction above). Red-team
      test expected: two beacons from one device carry different tag fields,
      and a household member still recognises both; sabotage by a fixed
      nonce (tags equal) and by an unkeyed hash (a stranger holding the user
      id recognises it). §6's `red_team_the_user_id_never_appears_on_the_wire`
      must stay green.
   e. ✅ **The macOS installer puts `itsanas` on the PATH and waits for Apple's
      tools.** Built 2026-10-05 (see §0); the original text follows. Found on Mandarine's Mac, 2026-10-05 (§10 item 5): after a
      clean install, `itsanas` is "command not found" -- `install/macos.sh`
      ~369 only warns that `$BIN_DIR` (`~/.local/bin`) is not on the PATH,
      and the same for `~/.cargo/bin` ~314. On a Mac whose login shell is
      zsh, add the export line to `~/.zprofile` (once: check for it first,
      safe to run twice), asked under `confirm` and done under `--yes`;
      `--clean` removes exactly that line. Second: when the Command Line
      Tools are missing, ~236-248 triggers their installer and dies asking
      for a second run; wait for them instead
      (`/Library/Developer/CommandLineTools/usr/bin/clang`, bounded, an hour)
      so a fresh Mac is one command. Check in `scripts/check-installers.sh`
      (macOS job) that a second run leaves one line, and that `--clean`
      removes it. Linux (`install/linux.sh`) likely has the same PATH
      warning: look, and fix it the same way if so.
   f. **The account device roster** (§10 item 7, option 2, decided
      2026-10-05). Not yet planned in detail: a signed list of at most five
      device ids per account, checked when a segment is opened, so a host
      that re-signs a genuine body under its own key is refused. Plan it
      cold first: where the roster lives (coordinator, beside the claims),
      how it reaches a reader, what a keystore-replaced device does.
4. **Verification at a terabyte.** The criterion: under 100 MB a day to verify
   under a terabyte (DESIGN.md §6.5). Not met yet; three steps.

   a. ✅ **Within a differing bucket, ask only about chunks with no fresh
   record for that peer.** Built 2026-10-04 (see §0). A change of D chunks now
   lists about D, against a peer that takes what it is offered.

   b. ✅ **Stop re-offering a peer the chunks it refused.** Built
   2026-10-04 (see §0).

   c. ✅ **The full walk itself.** Built 2026-10-04 (see §0). The plan as it
   stood: It lists the whole account per peer every
   `REFRESH_AFTER`: 537 MB every 3.5 days at 1 TB, about 150 MB a day, over
   budget on its own. Decided (§10 8): an agreeing bucket re-stamps its
   records, **and the host checks its index against its disk**, built
   together -- one without the other is the downside §10 8 names.
   (i) Host side, `crates/itsanas-store/src/vault.rs`: the summary
   (`Vault::chunk_summary`, ~472) reads the `CHUNKS` table while `HaveChunks`
   reads the blob file (`service.rs` `chunk`, ~421), and the index is
   realigned with the disk only after an unclean close
   (`reconcile_chunks_if_needed`, ~506). Add a rolling check: each service
   round, a slice of `CHUNKS` rows (cursor kept in `TOTALS` or a new table) is
   checked with `blobs.contains`; a row with no file is removed (and its
   bytes from the totals), so the summary changes and the owner's next round
   lists that bucket. Size the slice so a full pass takes at most
   `REFRESH_AFTER` at 1 TB (~55 rows/s; a few thousand per five-minute
   round). (ii) Owner side, `push_scoped` in `session.rs`: on a due walk,
   list only the differing buckets (with no freshness filter inside them) and
   re-stamp, without the wire, the records of chunks in agreeing buckets
   (a new `Index::restamp_bucket(device, bucket, now)` over `live_chunks_page`
   ranges, writing only rows older than `REFRESH_AFTER`). Red-team tests
   expected: a host that loses a blob file with its index intact is found
   within one pass of the check (sabotage: skip the check, the summary still
   agrees); an idle due walk lists zero chunks and leaves the records fresh
   (sabotage: list everything again, the count comes back); against a
   peer with a smaller budget nothing changes (every bucket differs).

5. **A real phone**, and a release signing key for the APK that Nicolas holds
   (v0.1.0 ships with the development key).

6. **CI that costs what the change costs.** Asked for by Nicolas on
   2026-10-06: "test again only what changed, except for major milestones",
   then, the same day, "a lot of red-team tests, one file per test type or
   attack surface, then we stop having 40+ minute CIs".

   a. ✅ **Selective CI, by crate** (2026-10-06, #228, see §0), and
      **measured** the same day on two throwaway drafts (#229 crypto, #230
      placement, closed) and on its own docs-only follow-up: ROADMAP
      "Selective CI" has the table. Each run's plan was read from its
      summary and matched `check-ci-scope.py`.

   b. **Red-team tests by attack surface.** Fix the list in
      `docs/TESTING.md` -- suggested: wire/framing, identity & claims,
      coordinator, host storage & quotas, placement & audit, discovery/LAN,
      TLS, local store & crash, sync convergence, installers. Move each
      `red_team_*` test (203 in about 30 files on 2026-10-06; most in
      `node/src/owners.rs` 19, `net/tests/two_nodes.rs` 19,
      `node/src/contact.rs` 16, `coord/src/directory.rs` 14,
      `cli/src/main.rs` 13) to `crates/<crate>/tests/redteam_<surface>.rs`
      where the public API allows, else into an inline
      `mod redteam_<surface>`. One nextest filterset per surface in
      `.config/nextest.toml`. Do not rename tests unless forced; if forced,
      the catalogue changes in the same commit; `check-counts.py` green.
      Re-sabotage at least one moved test per surface. Then extend
      `ci_scope.py` so a PR runs only the surfaces it touches, with a gate
      that goes red on a source file mapped to no surface.

   c. **Not decided -- needs Nicolas.** The push to `main` re-runs the full
      suite on a tree a PR run usually already tested, which roughly doubles
      the cost of a merge. Skipping it when the merged tree equals the tested
      PR merge commit's tree would halve that, but Nicolas asked on
      2026-10-06 for `main` to run in full unconditionally, and the full run
      on `main` is half of what catches environment breaks (ROADMAP,
      "Selective CI"). Asked in §10 item 11; do not build before the answer.

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
- **The self-update's scratch directory is beside the program**
  (`update.rs`, `SCRATCH`), so the final rename stays on one disk. With a
  `--prefix` the daemon's account cannot write (`/usr/local`), even
  `updates = notify` fails its daily check, logged each day. Manifests
  could go to the node's home instead; not to a shared temp directory,
  where a fixed name is a symlink target.
- **One process per node.** The index is under an exclusive lock, so commands
  refuse to run while the daemon holds it. Since 2026-10-06 the commands a tray
  needs while it runs -- `pause`, `resume`, `sync-now`, `interval`, `status` --
  go through files in the home instead (`control.rs`, the snapshot); anything
  that reads or changes the store itself still needs the daemon stopped, and a
  live file list still needs a control socket.
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
  document saves in 29 ms on the laptop and 10 ms on the aarch64 VM; the
  small machine wins because a save is dominated by one file per chunk.
- **No file-level sharing between users.** Not needed for mutual storage;
  `UserKeys::agree` exists, is tested, and is deliberately unused until it is.

## 10. Open, waiting on Nicolas

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
4. ✅ **Merging.** Nicolas said on 2026-09-17 that he does not want to be
   handed merges an agent can do: once CI is entirely green, and unless the PR
   touches a decision in §6, the agent merges (`gh pr merge --squash
   --delete-branch`). #53 and #54 were merged that way. A local policy guard
   refused the first attempt ("Merge Without Review") before he said so in the
   session; if it refuses again, say which permission rule would allow it
   rather than handing him the command, and check `gh pr list` before assuming
   `main` carries the work.
5. **Whether `install/macos.sh` works.** It has run on a CI runner and never on
   a Mac. The second person's machine is a Mac, so this is on the critical path
   of the pilot rather than a nicety.
   🟨 **Ran on Mandarine's Apple-silicon Mac, 2026-10-05**: build, install and
   smoke test pass (`--yes --no-service`, from the `main` tarball). Not seen:
   the fresh-Mac path (tools and Rust were most likely present, 29 s in all),
   the LaunchAgent, an Intel Mac. Found: the PATH (§8 3e).
6. **`main` is not protected, and the working rules say it is.** Checked on
   2026-09-16: `gh api repos/SigSegGit/itsanas/branches/main/protection` returns
   **404 Branch not protected**. There is no required check and no required
   review, so anything at all can be pushed straight to `main`, skipping CI
   entirely — which is precisely what every gate in this repository exists to
   prevent.

   Found by pushing a one-line change there by mistake and expecting to be
   refused. That is the only reason it is written down, and it is worth saying
   plainly: for months the rule "never commit directly to main, it is protected
   by CODEOWNERS and required CI" has been a belief, not a setting. Every PR in
   this repository has gone through CI because somebody chose to, not because
   anything made them.

   ✅ **Enabled 2026-09-16**, on Nicolas's instruction. `main` now requires
   *Format and lint*, *Test (ubuntu-latest)*, *Test (windows-latest)*,
   *Test (macos-latest)* and *Acceptance phases between three local nodes*;
   `enforce_admins` is on, so the rule binds Nicolas and any agent equally;
   force pushes and deletion are off. No review is required, because a solo
   owner cannot approve their own pull request and requiring one would
   deadlock the repository.

   Verified rather than assumed, by pushing an empty commit at it:

       ! [remote rejected] main -> main (protected branch hook declined)
       remote: - 5 of 5 required status checks are expected.

   Deliberately **not** required: *No warnings anywhere in this run*, which
   reports `skipping` on some runs — a required check that can skip blocks
   every merge. Add more contexts with
   `gh api -X PUT repos/SigSegGit/itsanas/branches/main/protection`; remove the
   lot with `gh api -X DELETE` on the same path if CI ever wedges.

7. **A host can re-sign a genuine segment body under its own key** (named by
   #207, ROADMAP "The integrity surface, by hand"). The seal binds the body
   to owner and segment id, not to the device signing the envelope, so a host
   holding any segment names a keypair of its own as `device` and signs; the
   owner's machine applies that "stranger's chain", replaying its own
   operations, and every release it made is undone. **Bounded today:** it
   re-downloads content the owner released, up to the pull ceiling (8.1b), on
   that owner's own machines; nothing is deleted, no content is revealed, and
   other devices' operations replay idempotently. Two options, neither built:
   (1) put the device id in `SealContext` for new segments: a seal format
   change; old segments still open without it, so readers carry both paths
   and a host can keep serving old-format bodies until they age out -- cheap
   in code, the hole stays open for history; (2) an account device roster,
   signed by the account, checked on open: closes it for all segments and
   would also scope reads to the account (ROADMAP "The confidentiality
   surface, by hand"), but it is a new signed object to distribute, keep and
   revoke, and a keystore-replaced device must be added to it. Which one?
   **Decided 2026-10-05 (Nicolas): option 2, the account device roster.**
   Since 2026-10-01 (§8 2d) an account has at most 5 live devices, enforced
   by the coordinator: option 2's roster would be at most five entries, a
   bounded object to sign and ship. That prepares it; nothing of it is built.

8. ✅ **May an agreeing bucket hash re-stamp holder records?** (§8 4c.) **Decided 2026-10-04, yes, with the host-side check**, under Nicolas's delegation for no-brainers once the downside was closed: the summary is read from the vault index, `HaveChunks` from the file, so an honest host whose disk lost files kept agreeing; the rolling check makes its summary as good as the walk. A dishonest host gains nothing: `HaveChunks` was self-reported too. The question as it stood: Today
   records are re-stamped only by a listing, and the full walk that lists
   everything every `REFRESH_AFTER` costs about 150 MB a day per peer at a
   terabyte, over the 100 MB criterion. If yes: the walk lists only the
   buckets that differ and re-stamps the rest locally, so an idle terabyte
   really costs a hash a round. The price: a host that replays an old summary
   is believed past `REFRESH_AFTER` -- today for at most that long -- and only
   the storage audit (sixteen chunks a round) can contradict it, so a host that
   threw data away and replays its old hash keeps counting as a copy until the
   audit lands on a missing chunk. If no: 4c needs a tree summary or a longer
   walk interval, both slower to build and the second a weaker freshness
   rule.

9. **Chunk sizes as a fingerprint (§8 3c).** A host holding a candidate file
   can recognise it by the sequence of chunk sizes (ROADMAP, "A chunk-size
   sequence is a fingerprint"). Closed question: **pad** chunks to size
   classes (closes it; costs disk on every host, how much to be measured
   before choosing the classes), or **accept** it and document that contents
   are protected and which-file is not, against a host that already has a
   candidate.
   **Decided 2026-10-05 (Nicolas):** close it if the cost is kept in hand --
   so **pad**, after measuring. The disk cost on a real folder is measured
   first and the classes chosen from it; if no class set keeps the waste
   reasonable, that measurement comes back to him before any code.
   **Measured 2026-10-06** (`padding_cost` example, 30.2 GiB of Nicolas's
   files, 1 090 files of two chunks or more). "Recognisable" is the share of
   those whose padded size sequence no other file in the corpus shares, a
   floor on what a host with a candidate learns:

   | classes | extra disk | recognisable |
   |---|---|---|
   | none (today) | 0 % | 98.2 % |
   | Padmé | 1.5 % | 77.8 % |
   | 4 per doubling | 11.6 % | 37.2 % |
   | power of two | 59.8 % | 19.2 % |
   | one class (256 KiB) | 265.6 % | 4.7 % |

   No class set keeps the waste reasonable *and* closes it: even one class
   leaks the chunk count. Taken back to him as a closed question (keyed
   chunking / Padmé only / accept). **Decided 2026-10-06 (Nicolas): keyed
   chunking** -- each account cuts with its own secret table, so a host
   cannot compute the sizes to look for; no disk, no deduplication lost;
   old files stay recognisable until rewritten; the published attacks are
   read first and anything that breaks it comes back to him (§8 3c).

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

12. ✅ **Who signs a self-updating release?** (§8 0t.) **Decided 2026-10-06
   (Nicolas): his key, on his PC**, against a key in CI or per-machine
   builds from signed tags; reasons in 0t.

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
14. **Generate the release key** (2026-10-06, §8 0w (3)). Run
   `scripts/sign-release.cmd` once (double-click): it creates the
   passphrase-protected key in `%USERPROFILE%\itsanas-release-key\` and
   prints the public key in hex and where the offline copy goes. Paste that
   hex into `RELEASE_KEY` in `crates/itsanas-release/src/lib.rs` (a PR; the
   pinning test changes with it on purpose). Until then every signature
   check is refused and `get.ps1` / `get.sh` have no release to install.

15. **Clean removal: drain or refuse?** (2026-10-07, §8 0w (7), from 0f (f)
   item 4, which left it "to decide with Nicolas first".) When a machine
   that hosts other members' chunks is removed: (a) **refuse** while any
   chunk it hosts has no other confirmed holder -- cheap, but a host cannot
   see other holders, so the answer has to come from the owners, and a
   machine whose owners are offline may be stuck until they return; or
   (b) **drain**: stop accepting, hand every hosted chunk off until each
   owner's ledger shows another holder or a stated timeout passes, then
   forget the device -- more code (a new protocol message), but removal
   always finishes. Either way this machine's own data follows the
   existing `status` rule.
   ✅ **Decided 2026-10-07: (b), drain.** Nicolas: nobody is kept from
   switching their machine off because the network is not dense enough.
   Idea for a later version, **not decided**: as a last resort the
   coordinator takes the chunks no host accepted (sealed, so unreadable
   to it) until a new host is found -- a temporary recentralisation as
   backup. It would give the coordinator a data-plane role, against
   ROADMAP's "control-plane only" and its storage and bandwidth cost;
   to be weighed (and Rodin'd) before anything is built.

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
