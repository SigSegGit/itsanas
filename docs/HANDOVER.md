# Handover

Everything needed to pick this project up cold. Read this, then
[ROADMAP.md](ROADMAP.md) for status and [ECONOMICS.md](ECONOMICS.md) for the
contract.

---

## 0. Resume here after `/clear`

<!-- ITSANAS-STATE
NEXT: 8.0o
TITLE: Phase 2 of 0o -- peers exchange the presences they saw, so the coordinator is a backup
WRITTEN-AT: 2026-09-18
BASE: 5712623
-->

Read this section, then §8. Nothing else is needed to continue. The block
above names the next step and `scripts/check-handover.py` keeps it honest;
whether CI is green and whether a PR is open are facts for `git` and `gh`,
never for this file.

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

**Where the truth is:** `docs/ROADMAP.md` "Known ceilings" and "What an
adversarial sweep found" (open findings, with arithmetic); `docs/ECONOMICS.md`
§1 (the 30/70 bargain is enforced locally only); `docs/DESIGN.md` §6.5–6.7
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

   l. **A storage location that vanished never reads as a deletion.** Asked
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

   m. **Count the live copies, and let a machine leave politely.** Asked for
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

   n. **Refuse a file that will not fit, before copying it.** This is 8.1b,
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

   o. 🟨 **Reach the network from outside the LAN, with machines that move.**
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

      **Phase 2 -- this is `NEXT`, and it is the decentralised half.** Specified
      on 2026-09-18 after Nicolas asked for it in his own words: *"j'aimerais
      que la vm centrale ne soit contactée que si c'est nécessaire, par exemple
      si aucune machine d'un compte n'est connectée"*. Today every node dials
      the coordinator **twice per round, unconditionally** -- `announce` then
      `peers`, `daemon.rs` ~634 -- which at the 300-second default is 576
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
      actual deliverable: a round dials the coordinator only when (a) it reached
      **no** peer by LAN discovery or by its address book, or (b) the book holds
      no entry for a device it needs, or every entry for it has failed, or (c)
      this machine's own address changed *and* it could not hand its new
      presence to any peer, or (d) an account event -- register, enrol,
      withdraw, escrow -- or (e) a **backstop interval**, long, so a fleet that
      is quietly healthy still checks in. At home, healthy, the answer is
      **never** between backstops.

      Constraints, none of them negotiable:

      - **A peer's clock never decides ordering** (§6). For a *relayed* presence
        this is sharper than it looks: the receiver did not observe the
        announcement, only the relay. So do not order by any claimed time at
        all. Keep every candidate address for a device as a **set**, and order
        it by *this machine's own record of which one last worked*. Success is
        the evidence; a timestamp is an opinion.
      - **A relay cannot invent a presence**: `SignedPresence` carries the
        device's own signature, and `Response::Peers` currently throws it away
        (`service.rs` ~336 maps to bare `Presence`). Carrying the signature
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

      **The decision this needs from Nicolas, and it must not be made by
      accident:** the per-round announce is also the heartbeat the coordinator
      measures availability from (`Directory::last_seen`, `AvailabilityRecord`),
      and availability is what ECONOMICS §3 turns into entitlement. Dialling the
      coordinator only when necessary makes that measurement coarser by design.
      Two honest options: keep a **backstop announce** (say hourly) so
      availability keeps its meaning at a twelfth of today's cost, or move
      availability onto **bilateral evidence** -- peers already exchange signed
      rounds, and who answered whom is a better measure of being *useful* than
      who pinged a server. The second is the better system and the larger
      change. Do not start phase 2 without choosing.

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

   p. **Named instances only, each showing its account and storage.** Asked
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

   f. **A tray icon for the Windows daemon.** Asked for by Nicolas on
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
   - `itsanas pledge` and the Android JNI `setKeep`/`setPledge`
     (`crates/itsanas-android/src/lib.rs`, around 551–622) still skip the check
     altogether — they set bytes without consulting any split. That is finding 3
     in item 3 below and was already true; (a) did not touch it. The coordinator
     claim (`crates/itsanas-cli/src/coordinator.rs:180`) and the daemon's
     `Pledge` (`crates/itsanas-cli/src/daemon.rs`) carry `pledge_bytes` and never
     read a split, which is correct.
   - **Writing never consults it.** `Store::write_stream` and `write_file`
     (`crates/itsanas-store/src/store.rs`, ~246 and ~310) and the folder import
     (`crates/itsanas-folder/src/lib.rs`, ~259) accept any amount: an account's
     size is bounded by nothing.
   - **Hosts bound themselves, not owners.** `would_exceed_pledge`
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
   b. **Bound writes on the honest client.** `Store::write_stream` and
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
   c. **Bound owners on the host — the part a rebuilt client cannot delete.**
      In `service.rs` `StoreChunk` and `StoreSegment`: a host stores for owner O
      at most an allowance plus `k ×` the bytes of this host's own data that O's
      devices have **proved** they hold (a passed storage challenge, as
      `Store::release` already requires). Attribute a device to its owner through
      the owner-signed `NodeClaim`, never the unauthenticated `Hello` field. Tests:
      a peer hosting nothing is refused past the allowance; a peer that hosts and
      passes audits keeps being served.
   d. `ECONOMICS.md` §1 back to ✅ built when (c) lands — it is 🟨 today and
      correctly so. Its §8 constants row, the catalogue rows and the counts in
      README, ROADMAP and TESTING were all done in (a).

2. **Finish the red team, one surface per session, by hand.** Three surfaces have
   never been examined — every multi-agent attempt died on usage limits:
   *integrity* (a hostile peer: forged or replayed segments, version vectors that
   win or resurrect deletions, chunks whose id does not match their bytes,
   downgrade past a later defence), *confidentiality* (convergent ciphertext, what
   two hosts learn by comparing notes), *identity* (many devices, claiming someone
   else's device, LAN discovery eclipse). Git history was checked for secrets on
   2026-09-14 and is clean.
3. **The open findings** listed in ROADMAP.md: a refused request still walks the
   whole vault (and, since the listener became concurrent, delays honest stores
   behind the storing lock); `pledge` and the
   JNI setters skip the ratio check; chunk-size sequences fingerprint files; the
   LAN beacon groups an account's machines.
4. **Verification at a terabyte.** Within a differing bucket, ask only about
   chunks with no fresh record for that peer (DESIGN.md §6.5). Today the budget
   buys about 3 MB of change a day at 1 TB.
5. **A real phone**, and a release signing key for the APK that Nicolas holds
   (v0.1.0 ships with the development key).

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
