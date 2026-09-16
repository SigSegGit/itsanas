# The MVP, and how it gets judged

This document exists to answer one question with evidence instead of an
impression: **is this project worth a day of Nicolas's time?**

Until now that question had no checkable answer. The roadmap says what is built,
the test catalogue says what is proven in a laboratory, and neither says what has
to be true on four real machines before the project has earned anything. This is
that definition, written before the work rather than after, so it cannot be moved
to fit the result.

The rule: **the MVP is done when every acceptance test in §3 passes, unassisted,
on the real fleet.** Not when the roadmap looks complete.

---

## 1. What ITSaNAS is being compared against

Not Syncthing. Nicolas has never used a peer-to-peer file tool. His reference is
**Google Drive and Dropbox**, and that sets the bar in a specific and unobvious
way: the interesting part of ITSaNAS is not that it is decentralised — a user
does not want decentralisation, they want their files — it is that the machine
holding the files belongs to somebody whose interests are not theirs, and here
that stops mattering because the host cannot read anything.

So the comparison splits three ways, and being honest about the third column is
what stops this document becoming marketing.

| | Drive / Dropbox | ITSaNAS must |
| --- | --- | --- |
| **Must match** | Install, log in, files are there. Works while you ignore it. Never loses data. | Match it. Anything worse here is disqualifying, however good the cryptography is. |
| **Must beat** | The company can read every byte, can be compelled to hand it over, can close the account, and charges rent forever. | Make blindness *verifiable in one command*, not promised in a document. No account to close, no rent. |
| **Will lose, accepted** | Web access from any browser. Share links. File history. Mobile apps. Someone to call. | Not build any of it for the MVP. Say so plainly rather than pretending it is coming. |

The middle row is the whole argument. If a member cannot check for themselves,
in under a minute, that the machine hosting their data cannot read it, then
ITSaNAS is a worse Dropbox with extra steps.

---

## 2. The fleet the MVP is judged on

| # | Machine | Role | Realistic uptime | Dialable from outside |
| --- | --- | --- | --- | --- |
| 1 | Windows laptop (Dell) | member, own data | ~25 % of the day, changing networks | **no**, and its network changes |
| 2 | Raspberry Pi 4B+, 1 TB RAID1 | member, large host | high, home connection | only if a port is forwarded to it -- **record which** |
| 3 | VMware VM on external SSD | member, and the throwaway used for recovery tests | on demand | no |
| 4 | **Freebox Delta VM, public IP** | **coordinator, and an availability anchor** | always on | **yes** |

**The last column is not bookkeeping; it decides whether a result means
anything.** NAT traversal is not built: a node behind NAT can push but cannot be
dialled, and work flows in both directions as long as *one* side of a pair can
([ARCHITECTURE.md](ARCHITECTURE.md) §6). So machines 1, 2 and 3 reach each other
directly **only while they are on the same LAN**, where local discovery finds
them. The moment machine 1 is elsewhere -- and its row says it usually is --
machine 4 is the only component either side can dial.

Two consequences, both of which change how the tests below are read. A test run
with everything sitting at home exercises the LAN path and says nothing about
the other one. And for any node away from home, the coordinator is a single
point of failure *in practice*, whatever test I concludes on a kitchen table.

Machine 4 is new and it changes the design: it is the only publicly reachable
component of the system, so hostile traffic is its normal condition. It is also,
by [ECONOMICS.md](ECONOMICS.md) §2, the thing that makes read-on-demand possible
at all — three replicas across machines that are mostly off buys durability and
not availability.

~~**To confirm before building against it:** the Freebox Delta VM is aarch64.~~
**Confirmed on the machine itself, 2026-09-01.** Ubuntu 26.04 aarch64, 2 vCPU,
11 GB: installed by the `curl | sh` one-liner on a box with no compiler and no
Rust, built in 5m37s, **the whole suite passing and none failing** (640 tests on
2026-09-01), and a benchmark
that saves a 512 KiB document in 10 ms against the laptop's 29 ms. CI also runs
the suite for that target under emulation on every push.

What is left for the *Pi* rather than the VM: 1 GB of RAM against 11, and an SD
card against a virtual disk.

---

## 3. The acceptance tests

Each one is a procedure Nicolas runs by hand, with a pass criterion that is not a
matter of opinion. **A test that needs a workaround, a hint, or a second attempt
has failed** — the comparison is a product that requires none of those.

### A. Install and enrol, without editing a file by hand

```
machine 1:  itsanas init --username nicolas
            itsanas folder ~/ITSaNAS
            itsanas pledge 100G
            itsanas daemon
```

**Pass:** the daemon finds the network on its own. No IP address typed, no config
file opened, no peer added manually. The same three commands work on machines 2
and 3 with `login` instead of `init`.

**Why this one is first:** it is the only test that Drive would also pass, and
failing it makes every other result irrelevant.

### B. A file appears

Drop a file in `~/ITSaNAS` on machine 1.

**Pass:** it is on machines 2 and 3 within 60 seconds of both being awake, byte
for byte. `sha256sum` agrees on all three.

### C. Blind hosting, verified rather than promised

Machine 2 is hosting machine 1's data. On machine 2, with root, scan everything
ITSaNAS wrote for a distinctive string that is in a file on machine 1.

```
grep -r "<a phrase only in the laptop's file>" /var/lib/itsanas/
itsanas doctor          # on machine 2: it can account for the bytes, not read them
```

**Pass:** nothing found. Not the phrase, not the filename, not the directory
structure. Machine 2 can say how much space it is lending and to whom, and
nothing else.

On machine 1, `itsanas status` should meanwhile say the data *is* elsewhere:

```text
is it anywhere else?
  yes            every chunk is on at least 3 machines
```

The two together are the whole claim: replicated, and unreadable where it went.

**This is the test that justifies the project.** If it fails, stop.

### D. Recovery from nothing

Destroy the VM on machine 3 completely — delete the disk image. Create a fresh
one, install ITSaNAS, and:

```
itsanas login --username nicolas
```

Passphrase only. **The 24 words must not be required.**

**Pass:** the account is restored and the files come back from whichever peers
happen to be online. If machines 1 and 2 are both switched off, the anchor
(machine 4) still serves them.

**Why the 24 words are not allowed here:** Drive does not ask for a seed phrase.
A recovery story that requires a piece of paper is a recovery story most people
will not have. The phrase remains the ultimate backup and the escrow is the
everyday path — with the honest caveat, recorded in [ECONOMICS.md](ECONOMICS.md)
§7, that the passphrase then becomes the weak link for anyone who steals the
coordinator's database.

### E. Machines that are never awake together

1. Machine 3 is off. Machine 1 writes a file and shuts down.
2. Wait. Machine 1 stays off.
3. Bring machine 3 up.

**Pass:** machine 3 has the file, obtained from machine 2 or 4, neither of which
can read it. Machines 1 and 3 were never online at the same time.

**Why:** this is the architectural claim of the whole system — blind hosts as a
store-and-forward relay. It is tested in the laboratory
(`a_host_relays_one_device_to_another_that_it_never_met`) and has never been done
over a real network with real power cycles.

### F. Deleting means deleting, including for a machine that was away

1. Machine 3 is off. Delete a file on machine 1.
2. Machine 1 goes off. Bring machine 3 up.

**Pass:** the file disappears from machine 3 without machine 1 being present, and
does not come back on the next sync. Nothing else is deleted.

**Why:** a delete that resurrects is the failure that destroys trust in a sync
tool permanently, and it is the failure mode that a naive design produces.

### G. Two edits, no loss

With machines 1 and 3 both disconnected from the network, edit the same file
differently on each. Reconnect.

**Pass:** both versions exist, one at the original path and one named
`<name>.conflict-<device>-<sequence>.<ext>`. Nothing was silently overwritten,
and both machines agree on which is which.

### H. It is not expensive to leave running

Leave the daemon on the laptop for 24 hours of ordinary use.

**Pass, all four:** no perceptible effect on battery life; CPU at idle indistinguishable
from the daemon being stopped; memory under 200 MiB with a large folder; the
machine sleeps normally and does not wake up for the daemon.

Two of those four are read more precisely than the sentence above, and the
readings were fixed before any result:

* **"indistinguishable from the daemon being stopped"** is tested as *under 5 %
  of one core, averaged over the hours the machine was awake*. That is a
  substitution and worth naming as one: the criterion is a **comparison** with
  the machine at rest, and neither kit takes a baseline with the daemon stopped.
  Until one does, a daemon at 4.9 % passes a test whose words it does not meet.
* **"200 MiB"** is what both kits have always compared against. The sentence
  said "200 MB" until 2026-09-16, a 4.9 % difference nobody was applying.

**"with a large folder" is now recorded, and still never judged.** Until
2026-09-16 neither kit recorded the size of the account it measured, so a PASS
on an empty account was indistinguishable from a PASS on a full one — and the
figures in §6 were taken on an account of about a megabyte. Both kits now ask
`itsanas status` for the file count on every sample and put it in the verdict
line:

    peak 11.7 MiB on an account of 4210 file(s)

or, when it could not be read:

    peak 11.7 MiB account size NOT recorded, so this says nothing about "with a large folder"

No threshold is applied, because the criterion says "large" without saying how
large and a number invented here would be exactly the kind of substitution the
CPU reading above already is. The receipt simply carries what was measured.

**It needs a binary from 2026-09-16 or later**, because reading the count
without a passphrase is what made this cheap. An older daemon — and every
machine on the fleet ran a 2026-09-14 build when this was written — records
`unknown`, which is the honest answer rather than a blank column that would
read as zero. Upgrade the machine before starting the 24 hours, and run H
against the folder the narrative in [BRIEFING-MVP.md](BRIEFING-MVP.md) builds,
not on a fresh node.

**Why this is an acceptance test and not a nicety:** the first version of this
that makes the laptop hot gets uninstalled, and the project ends there.

### I. The coordinator is not a single point of failure

Switch machine 4 off for 48 hours.

**Run it twice**, because the fleet table's last column makes them two different
tests: **once with machines 1, 2 and 3 at home**, where they can dial each other
over the LAN, and **once with machine 1 off the LAN** -- a phone hotspot is
enough. The second run is the one that matches how machine 1 actually lives.

**Pass:** machines 1, 2 and 3 keep syncing with each other using the node set they
already pinned. What stops is joining, address changes, and new-machine recovery —
nothing is lost, and `itsanas status` says clearly what is degraded rather than
pretending everything is fine.

**The two runs are expected to differ, and that difference is the result.** On
the LAN run this should pass outright. On the off-LAN run, machine 1 can be
reached by nobody and can dial only machine 4, which is switched off.

**What machine 1 should print, verified on 2026-09-16** against a node whose
coordinator and only peer were both unreachable — this wording was checked
because a pass condition nobody has seen the software meet is a day of waiting
for a verdict that may not exist:

    coordinator: unreachable (<address>: ...)
      Peers already known keep syncing. New machines cannot be found.
    1 machine(s) known, none reachable this round.
    folder: 1 in, 0 out, 0 deleted locally, 0 deleted remotely, 0 conflicts

That is the honest pass for machine 1: it **says** it is cut off, names what is
degraded rather than only reporting an error, and the file written during the
outage is still ingested and still there afterwards. If instead it goes quiet
and looks healthy, that is a failure of the same kind as L.

**Do not run `I check` on machine 1 for the off-LAN run.** The phase looks for
sync rounds *completing* after the outage begins, and a machine that can reach
nobody completes none — it would report FAIL for a node behaving exactly as it
should. Run `I check` on machines 2 and 3, which are on the LAN together, and
read machine 1's log by eye against the four lines above.

**Why:** if the answer is "everything stops", ITSaNAS is Dropbox with a worse
Dropbox in the middle, and the entire premise is gone.

### J. Everything reboots, nothing needs a human

Reboot all four machines, in any order, including power-cutting one mid-sync.

**Pass:** they come back and reconverge with no intervention. No corrupt index, no
manual `doctor --repair`, no lost file.

### K. A host that throws away what it holds

On the machine hosting another account's data, delete the vault by hand, as the
person who owns that machine. There is no kit phase: remove the node's `vault`
directory -- **not** `store/blobs`, which holds that node's *own* chunks and is
empty on a pure host.

**The owner's daemon must be running.** `itsanas sync` pushes and pulls and
**never audits**: `session::audit` is called from the daemon loop and nowhere
else. A one-shot round against a host whose vault has been emptied therefore
reports nothing at all, which reads as "the sanction does not work" and is
really "the sanction was never asked to run". This was found by automating the
test, not by reading the code.

**Pass, both halves:** the owner's daemon prints `FAILED n of m storage
challenges — it is not holding what it said`, `itsanas status` on the owner
grows a section naming the machine —

    peers that have failed a storage challenge
      419c86f7b294 answered 0 and failed 1, and is answering now

— **and the data is re-placed on another machine.** Nothing of the owner's is
lost, and after three consecutive failures the host stops being offered new
content.

**Both halves were run on 2026-09-16** in `scripts/acceptance-local.sh`, and
the second half is the one that matters: a system that detects a cheating host
and re-places nothing has lost its redundancy quietly, which is worse than not
detecting. With a spare host present the owner's `placements` count went
**83 → 125** — it *rose*, because the withdrawn copies were rewritten
elsewhere.

**One trap, because it nearly cost this criterion.** An earlier version of the
bench had only one host, so there was nowhere to re-place to; `placements`
stayed flat at 81 → 81 and that was briefly written up as "the sanction may not
work", with this criterion softened to match. It was the bench that was wrong.
**Run K with at least two hosts pledging**, or the half that matters cannot
happen and a flat count will look like a defect.

**Why:** every economic claim in this project rests on a sanction nobody has
ever watched fire. The mechanism is built -- storage challenges, `Reliability`,
`FAILURES_BEFORE_PAUSE = 3`, a pass decrementing the counter rather than zeroing
it -- and has only ever been exercised by its own unit tests, against a peer
that was polite enough to answer.

**Known before running it, so it is not recorded as a finding:** the vault is
ordinary files owned by the local user, so deleting it takes no privilege and
nothing tells that person they have just broken a promise. Whether it should be
harder is a ROADMAP question, not a result of this test.

### L. The person is told when their data is not safe yet

On a fleet that has not reached the replication target -- which is every fleet
this project has ever had -- look at what the software says without being asked.

**Pass:** the person learns that copies are missing **without running a
command**, and learns it before they trust the folder with anything.

**Failing today by construction, and written down so it stops being invisible.**
`itsanas status` already reports this well: `spreading off: N machines hold
anything of yours`, `the promise — 2 complete copies is what this is for; you
have 1`, `headroom — N chunks are on fewer than 3 machines`, and `unconfirmed —
the ledger remembers N copies; M holder records have gone quiet`. But it is a
report somebody has to ask for, and asking for it needs the passphrase. There is
no alert at all: [ARCHITECTURE.md](ARCHITECTURE.md) §7 is a table of conditions
that must eventually warn, with **nothing implemented**, and its first row is
this one.

**Why this is an acceptance test and not a nicety:** a system that is quietly
one disk away from losing your files, and looks identical to one that is safe,
has the failure mode of a backup that was never running.

### M. Two accounts on one machine cannot see each other

Run two instances on one machine, one per account, as
[BRIEFING-MVP.md](BRIEFING-MVP.md) §4 sets out.

**Pass:** each instance's folder and `itsanas status` show only its own
account's files; the account hosting the other's chunks cannot read them; and
stopping or withdrawing one instance leaves the other syncing.

**Why:** this shipped in #18 and entered no acceptance test. It is also the
cheapest way to have two accounts without a second person, which is what makes
C and K runnable alone.

### N. The filenames real people actually have

Put these in the synced folder on the **Linux** machine, let them reach the
Windows laptop and (when there is one) the Mac, and look at all three:

1. `Café décembre.txt` -- an accented name, which is most French filenames.
2. `Photo.JPG` **and** `photo.jpg`, both with different contents.
3. A folder tree about 300 characters deep.
4. `rapport final .txt` and `notes.` -- a trailing space and a trailing dot.
5. `CON.txt`, and a `~$rapport.docx` beside an open Word document.

**Pass:** every machine ends with the same set of files and the same contents,
and anything the system refuses it refuses *out loud*, naming the file.

**What is already right, so it is not re-discovered:** 4 and 5 are handled.
`crates/itsanas-store/src/path.rs` rejects trailing spaces and dots (Windows
strips them, so `evil.txt ` and `evil.txt` would be one file on one machine and
two on another) and Windows reserved device names; `crates/itsanas-folder/src/scan.rs`
ignores `.DS_Store`, `Thumbs.db`, `desktop.ini`, `ehthumbs.db`, the `~$` and
`.~lock.` prefixes Office and LibreOffice leave behind, and the `.tmp`,
`.crdownload` and `.part` suffixes of a download in flight. That is the same
list Syncthing and Drive arrived at, and it is already here.

**Run on the laptop against NTFS on 2026-09-16, which refuted two of three
guesses this row first carried.** They had been written from the absence of
code rather than from a run, and the absence of code was the wrong evidence:

* **Case-only pairs (2) are handled, and handled well.** `Camera/IMG.JPG` and
  `Camera/img.jpg` are two distinct logical paths in the store. Written out to
  NTFS, which folds case, the second collides with the first — and the conflict
  machinery catches it, keeps **both**, and says so by name:

      !!   Camera/img.jpg conflicted — your version kept as Camera/img.local-0e7c2b5917b1.jpg

  `img.jpg` held `lower-content` and the conflict copy held `UPPER-content`:
  **nothing was lost.** Three further scans reported `0 in, 0 out, 0 conflicts`
  and the file count stayed at two, so it settles rather than oscillating, and
  the conflict copy is not re-ingested as a new file.
* **Long paths (3) are fine.** A 305-character logical path — over 400
  characters absolute once the folder prefix is added, well past Windows'
  traditional 260 — was stored and written to NTFS without complaint.
* **Accented names (1) remain the open one, and it is still untested.**
  Nothing in the workspace normalises a filename; the only normalisation in the
  tree is of a BIP39 phrase, in `itsanas-crypto`. macOS returns decomposed
  Unicode (NFD) where Linux and Windows use composed (NFC), so the same name
  is two different byte strings on the two platforms. `Café décembre.txt`
  round-tripped correctly **on Windows**, which says nothing about a Mac, and
  there is no Mac here to try. Given how (2) behaved, the likely outcome is a
  **duplicate rather than a loss** — which is what Syncthing saw — but that is
  a prediction, not a result, and it stays marked as one until somebody runs it
  on the Mac.

**Why this is an acceptance test and not a nicety:** a sync tool that quietly
makes a second copy of your accented filenames is one that people stop
trusting, and none of the ten original tests would have caught it -- every one
of them uses names a program chose.

---

### Running them with the kit

`scripts/acceptance.sh` turns each test into phases that end in `PASS` or `FAIL`
with the numbers, and appends every verdict to
`~/.itsanas-receipts/acceptance.txt` — paste that file, not an impression. It
checks; moving power and cables stays with the person running it. The order to
run them in across the laptop, the Pi and the VM, with the preparation and the
account checks, is [BRIEFING-MVP.md](BRIEFING-MVP.md) (in French).

**Before anything: every machine pledges.** A host refuses to store past its
pledge, its own account's log included (DESIGN.md, the table of what counts
against what), and the default pledge is zero. A machine that has pledged
nothing relays nothing. The kit's own bench failed E, F and G for that reason
until it pledged, while every round said `sent 0 B in 0 chunks, 0 segments` —
the line for "nothing to send". Since 2026-09-15 the round adds
`refused N offer(s): its pledge is full or zero` instead of staying silent.

| Test | Where | Command |
| --- | --- | --- |
| A | every machine | no phase: the criterion is that nothing was typed but the three commands |
| B | machine 1, then 2 and 3 | `B write ~/ITSaNAS` → name and sha256; then `B check ~/ITSaNAS <name> <sha256>` |
| C | machine 1, then the host of **another** account | `C plant ~/ITSaNAS` → canary, which is the file's name as well as its content; then `C scan <canary> ~/.itsanas` (the control is built in). Only on another account's host: your own machine's index holds file names in the clear, correctly, and the scan will find the canary there |
| D | the rebuilt machine | `itsanas login --username <name> --from <coordinator>`, `itsanas register`, one `itsanas sync` with no address (it asks the coordinator for the account's other machines, so one of them must be up; the daemon also reaches hosts on the local network), then `D check <path> <sha256>` |
| E | machine 1, then 3 | `E write ~/ITSaNAS`, machine 1 off; later `E check ~/ITSaNAS <name> <sha256>` on machine 3. The kit cannot tell whether 1 and 3 ever met — that part is the person's discipline, so switch machine 3 off *before* machine 1 writes |
| F | machine 1, then 3 | `F delete ~/ITSaNAS <name>`; `F check ~/ITSaNAS <name>` on machine 3, and again after another round |
| G | machines 1 and 3, both offline | `G edit ~/ITSaNAS <name> <tag>` on each; after reconnecting `G check ~/ITSaNAS <name>` on both — the digests must match |
| H | the Windows laptop, and the Linux machines | **Windows:** `scripts\acceptance.ps1 H schedule` (samples every five minutes as a scheduled task), after 24 hours `H report` (CPU, peak memory, bytes written, and a `powercfg` battery report beside the verdict), then `H sleep` from an **administrator** PowerShell (whether the daemon holds the machine awake, and whether it woke it in the last day); `H unschedule` to stop. **Linux:** `H sample` every five minutes for 24 hours, then `H report`. Battery is reported, not judged: one day of one laptop measures the day as much as the daemon. **H passes only when `H report` and `H sleep` both pass and the battery report has been read**; `H report` alone is CPU and memory. The kit reads "CPU at idle indistinguishable from the daemon being stopped" as under 5% of a core averaged over the hours awake (at least 4 of them), decided before any result |
| I | any member, coordinator off | daemon output to a file, and **write a file on another machine during the outage** — idle rounds print nothing; then `I check <that file>` |
| J | every machine | `J count ~/ITSaNAS` before; reboot or cut power; daemon stopped, `J check ~/ITSaNAS <count>` |

`D check` and `J check` open the node, so the daemon must not be running on that
machine at that moment. `scripts/acceptance-local.sh` runs B, D, E, F and G and
every negative control between three nodes on one machine on each push: it
proves the kit and the mechanisms agree, and it is not a fleet result.

## 4. The verdict rule

Set in advance so it cannot be softened afterwards.

- **C fails** → stop the project. The premise is false.
- **A, B, D, F or J fails** → not an MVP. It is a demo. Fix before asking for the day.
- **E, G or I fails** → the distributed design is wrong somewhere; that is a
  redesign, not a bug fix, and worth knowing before more code is written.
- **H fails** → fixable, but nothing else gets built until it is.
- **K fails** → the sanction the whole bargain rests on does not fire. That is
  the economic model rather than a bug: treat it as C.
- **L fails** → the product is not fit to put in front of a person, however
  correct it is underneath. Nothing goes to anybody else until it passes.
- **M fails** → the multi-instance work of #18 is not finished.
- **N fails on the accented names** → a design decision rather than a bug fix
  (which normalisation to canonicalise to), and the one part of N still
  untested, because it needs a Mac. It does not block the verdict on A-M; it
  blocks putting the folder in front of somebody who has a Mac and accented
  filenames. The case and long-path halves were run on 2026-09-16 and pass.
- **All pass** → the project has earned the day of reading, and the question
  becomes whether to open it to people beyond Nicolas.

---

### The one measurement nobody can make from a laptop

`blob.rs` flushes every chunk to disk before publishing its name. `itsanas
bench` measured the cost: **a factor of two on write throughput** — fifteen
hours per terabyte instead of eight. Nothing has measured what it buys.

A process kill cannot: the crash test was run again with the flush removed and
passed identically, because killing a process leaves the kernel's page cache
intact. Only losing power discards it.

**The experiment, on the Pi, ten seconds:**

1. Start a large write — `itsanas put big.bin <a few hundred megabytes>`
2. Pull the power out, partway through
3. Boot, then `itsanas doctor --deep`

If it reports a file that fails verification or chunks referenced but missing,
the flush is earning its keep and the throughput cost stays. If it reports only
orphaned chunks — which is the expected, harmless result — then the flush is
buying nothing that content-addressing and authenticated encryption do not
already provide, and the write path can be twice as fast.

Worth doing twice, since one trial of a race proves little. It is the only open
question in this project that hardware answers and reasoning does not.

## 5. What "MVP" explicitly does not license

A minimum viable product is a reduced scope, not a reduced standard. These stay,
at MVP, because retrofitting any of them means rewriting rather than adding:

- **Every gate stays green.** `fmt`, `clippy -D warnings`, `cargo doc -D warnings`,
  the full test suite, MSRV 1.88, `cargo-deny`. A red CI hides the next real bug.
- **No `unsafe`, anywhere.** Workspace-wide `forbid`.
- **No shortcut through the threat model.** No "we will add authentication later",
  no plaintext on the wire in a debug mode, no coordinator that briefly holds a key.
- **Bounded memory on every path.** The Pi and the ARM VM are the target, not the
  laptop, and "it works on my machine with 32 GB" is not a result.
- **Every new test earns an entry in [TESTING.md](TESTING.md)** saying what breaks
  in the real world if it fails.
- **Documents keep the tense discipline** of [HANDOVER.md](HANDOVER.md) §2. Present
  indicative means it runs today and a test proves it.

The corners that *may* be cut: no web interface, no mobile, no sharing between
users, no erasure coding, no NAT traversal, no packaging or installer, no
migration guarantees between versions, and a coordinator that serves a handful of
members rather than thousands.

---

## 6. Where the MVP stands today

Measured against §3, not against the roadmap.

| Test | Status | What is missing |
| --- | --- | --- |
| **Runs on Linux at all** | ✅ *verified, with limits stated* | `install/linux.sh` on a bare Ubuntu 22.04 image with no compiler and no Rust: it installed the toolchain, built, installed, and the binary ran. Two homes of one account then moved a 293 KiB file across a real loopback socket, byte for byte, `doctor` clean afterwards. **What it proves:** the code compiles and runs against glibc and a Linux kernel, the CLI works, the protocol works over a real socket, and the installer works from nothing. **What it does not:** it was WSL2 — a real Linux kernel, but x86-64, on ext4 inside a virtual disk. Nothing here says anything about **ARM** (blake3’s NEON path, and every constant in this repository that says "on a Raspberry Pi"), about an SD card, or about `fsync` on real hardware |
| A — install and enrol | 🟨 | **Invitation tested between two real machines**, 2026-09-01: `itsanas invite` on the laptop produced a code, `itsanas register --invite` on the VM was admitted by it, through a coordinator running as a system service. Coordinator-mediated peer lookup works too — a daemon with no peer configured found the other machine and synced with it. Still untested: local discovery between two physical machines, and the third and fourth |
| B — a file appears | 🟨 | **Across two physical machines, two architectures and a real coordinator**, on 2026-09-01. A 683 KiB file written on an aarch64 Ubuntu VM on the Freebox Delta, pulled byte for byte by a Windows x86-64 laptop that had joined the account **from the 24 words alone** — no key material copied between them. Then the reverse: a file written on the laptop reached the VM, and a file deleted on the VM disappeared from the laptop on the next round, content and all. What is left for a full pass: the third and fourth machines, and a week of it running unattended |
| C — blind hosting, and the audit that backs it | ✅ **on two real machines** | 2026-09-01. Two *separate accounts*: `alice` on the Windows laptop, `bob` on the aarch64 VM, Bob admitted by an invitation code Alice issued through the coordinator. Alice pushed a document containing a canary string; Bob's `status` reads `peers hosted 1, chunks held 1`, his `ls` reads `(no files)`, and grepping his entire store finds neither the canary nor the path `prive/secret.txt` — with a control proving the grep finds the canary when it is there. What Bob *does* know is whose data it is: the blob sits under `vault/owners/<alice's user id>/`, which bilateral accounting requires. Then the defence itself, on the same two machines: Bob's blob was deleted from his disk behind his back, and Alice's next daemon round printed `FAILED 1 of 1 storage challenges — it is not holding what it said` and **re-uploaded the chunk in the same round**, after which rounds returned to 208 bytes of nothing-new. Detection, sanction and repair, on hardware. Not yet done: more than one chunk, and the probation ladder over many rounds | **Repeated on 2026-09-01 between two Linux machines, in both directions.** A Raspberry Pi 4B on an SSD (account `nicolas`, founding member) and the Freebox VM (account `voisin`, admitted by an invitation the Pi issued), on a coordinator running as a system service on the Pi. Each stored a 528 KiB file containing its own canary; each ended up holding the other's chunks and neither canary appears anywhere under the other's `~/.itsanas` — with the planted-copy control run on both machines, so the search is known to work. Then the host attack again, across accounts: one of the Pi's blobs was moved out of the VM's vault, and the Pi's next round re-sent exactly that chunk (`sent 79.0 KiB in 1 chunks`) and the file reappeared under its own name. What this adds over the laptop/VM run is that the coordinator, both members and both directions were on hardware that stays on.
| D — recovery from nothing | 🟨 **from the 24 words only — the criterion forbids them** | *2026-09-07, and this is the first time it was not four processes on one box.* The Freebox VM, which had never held the account `sigseg42`, was given its twenty-four words and nothing else: `itsanas login --username sigseg42 --phrase-file …` restored the identity with a **new device id for that machine**, `register` enrolled it with the coordinator on the Raspberry Pi, and one sync round pulled **four files** from the laptop over a real socket. `photo.bin` came back with the same SHA-256 as the original, byte for byte, and the canary in `note.txt` read correctly. The temporary device was then withdrawn with `itsanas device forget`, which is the other half of the same story: a machine used once must be able to stop counting. **What is still not covered, and it is the pass criterion:** recovery by passphrase alone through the coordinator's escrow — the path that matters for somebody who has lost the paper, and the one Drive does not ask for. It is built (`itsanas register --recovery` lodges the sealed container, `itsanas login --username <name> --from <coordinator>` restores from it) and has never been run on the fleet. This row said ✅ until 2026-09-14 while §3 D says "the 24 words must not be required" |
| **A device smaller than the account** | ✅ **on real machines** | *2026-09-07.* A node on the Freebox VM restored `sigseg42` from its twenty-four words, was told `itsanas keep 200K` against an account of about a megabyte, and synced. It ended holding **206.7 KiB** with **four of five files listed as `not here`** — the behaviour a phone needs, and the one nothing could express a day earlier. Then `itsanas get gros.bin` fetched that one file from the laptop over a real socket: `fetched gros.bin from 192.168.1.142:9797`, 700 KiB, SHA-256 identical to the original. **The first run of this test failed and is why it is worth recording**: `itsanas sync` ignored the budget entirely — only the daemon honoured it — so the device downloaded everything and the `get` that followed proved nothing, because the file was already local |
| **A device that chooses what to keep** | ✅ **on real machines** | *2026-09-07.* A node on the Raspberry Pi, `keep 300K --order smallest`, built itself **entirely from two hosts** — it could not reach another of its own devices at all — and kept `a.bin` (120 KiB) and `b.bin` (130 KiB) while listing `c.bin` (400 KiB) as `not here`. A 60 KiB file arrived: it fetched that and let go of `b.bin`, ending at 180 KiB. Then the safety rule, both ways: a 200 KiB file created *on that device* was refused release against one host — `fewer than 2 other live machines hold them` — and released against the second. `itsanas get` brought it back byte-identical (SHA-256 `6a6367dd…`) and the next round let it go again. **Three defects were found by running it and none by the test suite**: a released file vanished from its own device's listing; it could then not be opened, while two hosts held it; and releasing erased the holder ledger, so a device could let go of a file exactly once and then sat over its limit for ever, saying so every round |
| **A limit that can still be checked afterwards** | ✅ **on real machines** | *2026-09-07, after a review found the hole.* Three lines together meant that once a device released a chunk, nothing could ever tell it the holders had lost that chunk: the have/missing sweep starts from the local blob store, the audit re-derives from a local copy, and liveness was asked of the machine rather than the record. A round now asks each peer about the chunks the ledger says it holds and this device does not — free on a machine that holds its whole account. On the Pi, `status` on the keep-limited node now reads **`could you get back what is ON THIS MACHINE`** and names the **12 chunks it cannot speak for**, where it used to print `2 copies` as though that covered the account. Quiet rounds also went silent across all three machines, the push having stopped re-offering the whole log every five minutes |
| **The phone** | ✅ **an APK, and it runs** | *2026-09-07.* An Android 15 emulator, driven through the interface rather than a harness: an account **restored from its twenty-four words** typed into the phone, one machine added, and a sync that pulled **five files, `5 here · 0 not here · 910 KiB`** — exactly the sum of the file sizes, so the bytes are on the device and not merely listed. The machine it pulled from belongs to a *different account* and holds this one's data sealed in its vault. Opening a file writes it out and hands it to the system chooser; the foreground service runs with its notification. The APK is 19.5 MiB with three ABIs, built by `scripts/build-apk.sh`. **Not yet tested on a real handset, and there is no folder that syncs by itself** — the application holds files, it does not watch a directory |
| E — never awake together | ✅ *in the laboratory* | `a_host_relays_one_device_to_another_that_it_never_met`; never done with real power cycles. Since 2026-09-14 also run through the kit between three local nodes on every push (the `acceptance-local` job) — still one machine and no power cycle |
| F — delete survives absence | ✅ *in the laboratory* | The local ledger and the 27-case decision matrix; never done across a real reboot. Since 2026-09-14 the kit deletes through a relay between three local nodes on every push, and checks the file stays gone a round later |
| G — two edits, no loss | ✅ *in the laboratory* | Conflict siblings, tested through a real socket. Since 2026-09-14 the kit edits apart on two local nodes, meets them through a third, and checks both keep two distinct versions with the same agreement digest |
| H — cheap to run | 🟨 | Measured on both machines. **Saving a document is instant** — a 512 KiB Word document takes 29 ms on the laptop and **10 ms on the aarch64 VM**, a 4 MiB PDF 159 ms and 75 ms. The small machine wins because a save is dominated by writing one file per chunk, which NTFS charges for and ext4 does not; the laptop chunks 4.6× faster and still loses. Archive throughput is the weak number (27.2 MiB/s on the laptop, 54.1 on the VM, 14.7 million files per terabyte) and pack files are the decided fix — a first-fill problem, not a daily one, and a bigger win on Windows than anywhere else. **Seven hours of the twenty-four are in**, sampled every five minutes on all three machines with an account of about a megabyte and nothing happening:

| | CPU, of one core | peak resident | written per day |
| --- | --- | --- | --- |
| Raspberry Pi 4B | 1.43% | 7.2 MiB | 313 MB |
| Freebox VM | 1.06% | 16.3 MiB | 296 MB |
| Windows laptop | 2.53% | 12.6 MiB | not measured |

Memory is an order of magnitude inside the 200 MiB the criterion asks for. **The number that was not being watched is the third column: three hundred megabytes written per day by a node with nothing to do** — a hundred gigabytes a year to store nothing new. A controlled experiment since: **61 KB per round with no peer, 962 KB with two**, so 94% is the sync round. The cause turned out to be the *number* of transactions rather than their content — a copy-on-write engine charges by the commit. Writing back fewer ledger rows bought 19%; collapsing the audit's sixteen commits into two bought 43% more. **447 KB per round now, about 129 MB/day against the 313 it was found at.** One transaction per round is the end state and is not built. On an SSD it is unremarkable; on the SD card this project has already destroyed one of, it is not. Recorded rather than guessed at, and the next thing to measure |
| I — coordinator outage | ✅ *verified locally* | A node with a coordinator configured and unreachable syncs normally with a known peer and the file arrives. The daemon keeps its loop, reports the outage **once** rather than every round, and says what is degraded. Not yet done across the real fleet for 48 hours |
| J — reboots cleanly | 🟨 | **Two of three parts done.** *The machine part, on real hardware, 2026-09-06*: both Linux machines were rebooted -- the Freebox VM and the Raspberry Pi that carries the coordinator. Everything came back without a hand on it: the coordinator as a system service, both member nodes as user units through lingering, ports rebound, and the vaults intact (7 blobs on the VM before and after, 14 on the Pi). Then the network reformed on its own -- a file dropped into the laptop's synced folder afterwards reached the VM, taking it from 7 blobs to 8. `scripts/disk-health.sh` bracketed the Pi's reboot and reported nothing moved, with a control proving the search worked. *The process part*: a crash test kills the process mid-write a dozen times at measured points and checks that nothing is listed-but-unreadable and no repair is needed; it passes. *What is still missing*: a power cut. Killing a process does not discard the kernel's page cache, and neither does a clean reboot -- `systemctl reboot` flushes. That needs ten seconds and a plug. See below |

**The critical path is now the fleet itself.** Everything the acceptance tests
need is built; none of it has been run on four real machines, and three of the
fourteen tests have never been attempted at all -- K, L, M and N were written
on 2026-09-16 and none of them has ever been run on the fleet, though C, K and
M now run automatically in `scripts/acceptance-local.sh`. Checked again on 2026-09-14, when
the question was "is anything functional": B, C and a words-only D run on real
hardware, and the verdict of §4 has still not been taken because E, F, G, I and
the power-cut half of J have never left the laboratory. The plan puts running
them ahead of enforcing the bargain, which only matters once somebody other
than Nicolas joins — see [HANDOVER.md](HANDOVER.md) §8 item 0.

H was briefly the critical path and then was not, which is worth recording
because the mistake is instructive. `itsanas bench` measured throughput first and
concluded the storage layer was the emergency: 19 MiB/s and 14.7 million files
per terabyte. But throughput answers "how long does the archive take", and nobody
waits for the archive. Measuring the thing a person actually waits for — a save —
gave 6.6 ms for a note and 28 ms for a Word document on the laptop, and 1.1 ms
and 10 ms on the ARM VM. Pack files are still the
right answer to the archive; they are not an obstacle to using the thing.

That ordering is the plan: the coordinator, then escrow recovery, then the fleet
bring-up, then pack files before the array is filled. It is reflected in
[ROADMAP.md](ROADMAP.md) and [HANDOVER.md](HANDOVER.md) §8.

### Test I is now a stronger claim than it was

It was written as "the coordinator can be down". After the decentralisation
audit in [DESIGN.md](DESIGN.md) §8 it is closer to "there is nothing vital for
the coordinator to be down *for*": local discovery needs no server, placement is
to be recorded by the owner rather than agreed globally, and accounting is
bilateral. What is left for it is finding a machine on another network, and
holding escrow blobs somewhere a rate limit can exist.
