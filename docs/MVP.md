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

| # | Machine | Role | Realistic uptime |
| --- | --- | --- | --- |
| 1 | Windows laptop (Dell) | member, own data | ~25 % of the day, changing networks |
| 2 | Raspberry Pi 4B+, 1 TB RAID1 | member, large host | high, home connection |
| 3 | VMware VM on external SSD | member, and the throwaway used for recovery tests | on demand |
| 4 | **Freebox Delta VM, public IP** | **coordinator, and an availability anchor** | always on |

Machine 4 is new and it changes the design: it is the only publicly reachable
component of the system, so hostile traffic is its normal condition. It is also,
by [ECONOMICS.md](ECONOMICS.md) §2, the thing that makes read-on-demand possible
at all — three replicas across machines that are mostly off buys durability and
not availability.

**To confirm before building against it:** the Freebox Delta VM is aarch64. CI
already cross-builds for `aarch64-unknown-linux-gnu`, but that is a compile, not
a run.

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
from the daemon being stopped; memory under 200 MB with a large folder; the
machine sleeps normally and does not wake up for the daemon.

**Why this is an acceptance test and not a nicety:** the first version of this
that makes the laptop hot gets uninstalled, and the project ends there.

### I. The coordinator is not a single point of failure

Switch machine 4 off for 48 hours.

**Pass:** machines 1, 2 and 3 keep syncing with each other using the node set they
already pinned. What stops is joining, address changes, and new-machine recovery —
nothing is lost, and `itsanas status` says clearly what is degraded rather than
pretending everything is fine.

**Why:** if the answer is "everything stops", ITSaNAS is Dropbox with a worse
Dropbox in the middle, and the entire premise is gone.

### J. Everything reboots, nothing needs a human

Reboot all four machines, in any order, including power-cutting one mid-sync.

**Pass:** they come back and reconverge with no intervention. No corrupt index, no
manual `doctor --repair`, no lost file.

---

## 4. The verdict rule

Set in advance so it cannot be softened afterwards.

- **C fails** → stop the project. The premise is false.
- **A, B, D, F or J fails** → not an MVP. It is a demo. Fix before asking for the day.
- **E, G or I fails** → the distributed design is wrong somewhere; that is a
  redesign, not a bug fix, and worth knowing before more code is written.
- **H fails** → fixable, but nothing else gets built until it is.
- **All pass** → the project has earned the day of reading, and the question
  becomes whether to open it to people beyond Nicolas.

---

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
| A — install and enrol | 🟨 | **Machines on one network now find each other with nothing configured.** A machine on a different network still needs `itsanas peer add`, which needs the coordinator |
| B — a file appears | 🟨 | Works between peers that have found each other; the discovery half is done, the sync round on top of it is not yet exercised across two real machines |
| C — blind hosting | ✅ *in the laboratory* | Proven by `a_host_stores_a_strangers_data_and_cannot_read_a_byte_of_it`; never done on real machines |
| D — recovery from nothing | 🟨 | Works with the 24 words. Escrow login by username and passphrase is not wired |
| E — never awake together | ✅ *in the laboratory* | `a_host_relays_one_device_to_another_that_it_never_met`; never done with real power cycles |
| F — delete survives absence | ✅ *in the laboratory* | The local ledger and the 27-case decision matrix; never done across a real reboot |
| G — two edits, no loss | ✅ *in the laboratory* | Conflict siblings, tested through a real socket |
| H — cheap to run | ❌ | `itsanas bench` now exists and found the answer is no at scale: 19 MiB/s of local write, so 15 hours for a terabyte, and 14.7 million files to hold it. Pack files are the decided fix |
| I — coordinator outage | 🟨 | Nothing to switch off yet. Stronger than it was: local discovery means a household keeps working with no server in the design at all, not merely with one that is down |
| J — reboots cleanly | ❓ | Never tested. The index is transactional, which is a reason for confidence, not evidence |

**The critical path is now H**, which turned out to be the item that could
invalidate the project — exactly as this document predicted, and for a reason
nobody had looked at. `itsanas bench` measured it: 19 MiB/s of local write and
14.7 million files per terabyte. The Raspberry Pi with a 1 TB array is the
machine this project exists for, and the current blob layout does not reach it.

After that, D and the remote half of A, both blocked on the coordinator server.

That ordering is the plan: pack files, then the coordinator, then escrow
recovery, then the fleet bring-up. It is reflected in
[ROADMAP.md](ROADMAP.md) and [HANDOVER.md](HANDOVER.md) §8.

### Test I is now a stronger claim than it was

It was written as "the coordinator can be down". After the decentralisation
audit in [DESIGN.md](DESIGN.md) §8 it is closer to "there is nothing vital for
the coordinator to be down *for*": local discovery needs no server, placement is
to be recorded by the owner rather than agreed globally, and accounting is
bilateral. What is left for it is finding a machine on another network, and
holding escrow blobs somewhere a rate limit can exist.
