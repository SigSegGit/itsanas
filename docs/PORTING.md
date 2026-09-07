# Porting: one core, several shells

> Measured, not estimated. Every claim in the "verified" column was produced by
> running the command next to it on 2026-08-28.

## 1. The shape

There is **one core and a shell per operating system**. Not one binary that
adapts — that would be the wrong shape, because the systems differ in kind and
not in taste. Android kills background processes and has no terminal; macOS has
`launchd`; Windows has services.

What must be identical everywhere is everything that decides *behaviour*:
chunk boundaries, sealing, version vectors, the wire protocol. Two devices that
disagree about any of those stop understanding each other, silently. So the core
is one Rust workspace compiled for each target, byte-for-byte the same logic.

```
       ┌──────────────── the core, identical everywhere ────────────────┐
       │ crypto  store  sync  placement  discover  wire  policy  net    │
       │                                          tls  coord           │
       └────────────────────────────────────────────────────────────────┘
              │                    │                    │
        itsanas-cli          (Android shell)      (future shells)
     Windows, macOS, Linux    not written yet
```

`itsanas-cli` is already the shell for three systems, because they all have a
terminal, a filesystem and no restrictions on background processes. Android
needs its own because it has none of those three.

`itsanas-policy` exists for the same reason and is worth naming here: *when* and
*how much* to sync is a question every shell has to answer and Android is only
the platform that forces it. A laptop tethered to a phone should not upload
forty gigabytes either.

## 2. macOS on Apple Silicon — ready

**This is where ITSaNAS meets real ARM silicon.** CI runs the full test suite on
`macos-latest`, which has been Apple silicon since macOS 14, so
`aarch64-apple-darwin` — a genuinely weakly-ordered machine with genuine NEON —
is tested on every push and has been since the first run. That is easy to miss
because the job is called "Test (macos-latest)" rather than anything about ARM.

`install/macos.sh` is run there too, in full, and finishes with the
store-and-read-back: `PASS: ITSaNAS stored and returned a file -- native arm64`,
macOS 26.5.2, Apple silicon. Before that job existed the script had never been
executed anywhere by anybody.

| | Status |
| --- | --- |
| Compiles | ✅ CI, every push |
| Full test suite | ✅ CI, every push, **on real Apple silicon** |
| `install/macos.sh` | ✅ CI, every push, ending in a store-and-read-back |
| Run by a person on their own Mac | ❌ nobody has |
| Platform-specific code | one `cfg(unix)` for key file permissions |

That fourth row is the one that is still empty, and the row above it used to say
"Run on real hardware — nobody has" while the three paragraphs above it said the
opposite. A `macos-latest` runner *is* real hardware; what is missing is somebody
using it, which is a different claim.

### Installing

Compiling locally is the easiest path and avoids Gatekeeper entirely — a binary
you built yourself is not quarantined.

```bash
brew install rust          # or rustup, needs 1.88 or newer
git clone <repository> && cd itsanas
cargo build --release -p itsanas-cli
./target/release/itsanas init --username <name>
```

Then follow [QUICKSTART.md](QUICKSTART.md).

**If you copy a binary from another machine instead**, macOS quarantines it:

```bash
xattr -d com.apple.quarantine ./itsanas
```

Signing it properly needs an Apple Developer account at 99 USD a year, which is
not worth it for a handful of testers.

### Running it as a service

`launchd` rather than systemd. Save as
`~/Library/LaunchAgents/net.itsanas.daemon.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>net.itsanas.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/itsanas</string>
    <string>daemon</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <!-- Anything able to read this process's environment can read the
         passphrase. That is a trade you are making, not a default. -->
    <key>ITSANAS_PASSPHRASE</key>
    <string>…</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/tmp/itsanas.log</string>
  <key>StandardErrorPath</key><string>/tmp/itsanas.err</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/net.itsanas.daemon.plist
```

The application firewall will ask once whether to accept incoming connections.
Say yes, or nothing will be able to dial this machine.

## 3. Raspberry Pi and the Freebox VM — both have run it

**The Freebox Delta VM has run it.** aarch64 Ubuntu 26.04, 2 vCPU, 11 GB,
installed on 2026-09-01 by `curl … | sh` on a machine that had no compiler and
no Rust on it. The build took 5m37s. Then, on that machine:

- **the whole suite passes, none fail** — including the three `#[ignore]`d ones,
  natively, no emulator. 640 of them on 2026-09-01; the figure moves as tests are
  added, and `docs/TESTING.md` holds the current one
- `scripts/smoke.sh`: an account, a 24-word phrase, a 350 KB file across five
  chunks read back byte for byte, `doctor` clean —
  `PASS: ITSaNAS stored and returned a file -- native aarch64`
- `itsanas bench --quick`: it **saves a 512 KiB document in 10 ms against the
  laptop's 29 ms**, on a machine that chunks 4.6× slower. See ROADMAP.md M9 for
  why, and for what that says about pack files.

**And the Raspberry Pi has run it — twice, on two different disks, and the
second time is the one to read.**

The first Pi was a 4 Model B Rev 1.1, Debian 13, aarch64, 4 GB, on a 58 GB SD
card — not the 1 GB machine this file assumed, which is the first correction the
hardware made. It installed and benchmarked. Within the hour its root filesystem
failed: `EUCLEAN`, and `sshd` stopped completing a handshake. Its `ext4` had
logged six `EFSCORRUPTED` block-bitmap errors at boot, *before* anything here
touched it, and the compile load almost certainly accelerated the damage. The
test suite could not be run on it at all: `rustc` died with `SIGBUS` on assorted
small dependencies, varying between runs.

**The second Pi is the same board reimaged onto a 119 GB SSD**, Debian 13
trixie, 3.8 GB, four cores, `/dev/sda2`. On 2026-09-01 it was taken from a
freshly written image to a running node by `install/provision.sh`, and it now
carries the coordinator as a system service as well. On that machine:

- **the whole test suite passes natively — every binary, no failures.** This is
  the first time it has run on a Pi at all. All three `#[ignore]`d tests ran too
  and passed: the crash test that kills a store mid-write a dozen times
  (15.9 s), the real 64 MiB Argon2id derivation, and the streaming test for a
  file larger than any buffer. The `SIGBUS` crashes were the card, as suspected,
  and nothing about the architecture.
- `scripts/smoke.sh` passes as part of every install
- both directions of cross-account hosting, with the coordinator on this machine
- **it survives a reboot**, which nothing had checked: `systemctl reboot`, and the
  coordinator and the member node both came back without a hand on them --
  the first as a system service, the second as a user unit through lingering.
  The fourteen blobs it hosts for another account were still there, and
  `scripts/disk-health.sh` bracketing the reboot reported nothing moved

`itsanas bench` on the SSD, against the two machines that had numbers before:

| | laptop x86-64 | VM aarch64 | Pi 4B, SD card | **Pi 4B, SSD** |
| --- | --- | --- | --- | --- |
| store write | 27.2 MiB/s | 54.1 MiB/s | 44.1 MiB/s | **44.1 MiB/s** |
| store read | — | — | — | **75.7 MiB/s** |
| a note, 4 KiB | 9.2 ms | 1.1 ms | 0.8 ms | **0.7 ms** |
| a spreadsheet, 64 KiB | — | — | — | **2.1 ms** |
| a Word document, 512 KiB | 29 ms | 10 ms | 12 ms | **12 ms** |
| a big PDF, 4 MiB | 159 ms | 75 ms | — | **94 ms** |
| a photo burst, 32 MiB | — | — | — | **729 ms (p95 737 ms)** |
| peak memory | — | 9.2 MiB | 7.6 MiB | **7.5 MiB** |

**The SD card's numbers survived.** Store write is identical to three
significant figures, the Word document is identical, the note is within a tenth
of a millisecond, and peak memory is within 0.1 MiB. So the caveat that used to
stand here — that those figures were taken on a failing machine and needed
repeating — is now discharged: they were right.

That is worth stating carefully, because it is the kind of result that invites
the wrong lesson. The measurements being reproducible does **not** make it
correct to have run a long compile on a filesystem that had already logged
corruption. The check made beforehand was a single reading of the error counter,
which cannot show a direction; the decision was wrong when it was made, and it
would have been wrong if the card had survived. `scripts/disk-health.sh` exists
so that check cannot be made that way again: it takes two readings and reports
what moved, and it carries a control so that a count of zero from a search that
finds nothing is not mistaken for a clean machine. It was used around this
suite run — `errors_count` 0 before and after, kernel errors 0 with a control
that matched, 41.8 °C rising to 50.1 °C, `throttled=0x0`.

The photo burst is the one figure that fails its own bar: 737 ms at the 95th
percentile against the 100 ms that reads as instant. That is a 32 MiB write, and
it is what `bench` says to fix.

What that leaves genuinely untested, on any machine: **redb on an SD card under
sustained write**. The medium's real question is now unanswered in a different
way than before — this fleet no longer has an SD card in it.


### What emulation established before that, and what it did not

Both are `aarch64-unknown-linux-gnu`. CI also cross-builds the whole workspace on
every push and then **runs it on that architecture** under `qemu-user-static`:
the whole suite except the three `#[ignore]`d, plus the doctests, and **none
fail**.

No total is written here on purpose. This paragraph used to carry one, and a
sentence explaining why it differed by three from the total in `TESTING.md` —
and then the suite grew and both numbers here went stale while the explanation
of their difference stayed, which is worse than either number being wrong on
its own. `docs/TESTING.md` holds the count, `scripts/check-counts.py` checks it
against the source, and this file describes what runs rather than how many.

Then `scripts/smoke.sh` creates an
account, checks the recovery phrase is still 24 words, stores a 350 KB file
across five chunks, reads it back byte for byte and runs `doctor` — the same
script an installer runs at the end of a real install, with no emulator in the
way.

The three old unknowns are answered *as far as instruction semantics go*, which
is a narrower statement than it first looks and is worth keeping narrow:

- `blake3` compiles NEON assembly for aarch64 — **exercised**, it is what hashed
  every chunk in every store test, and a wrong NEON path would have produced
  wrong hashes
- `redb` uses memory mapping, which is where architecture surprises usually
  live — **exercised**, 138 store unit tests and 29 integration tests
- `ring` has its own aarch64 assembly paths — **exercised**, `itsanas-tls`'s
  eleven tests include a real handshake over a real socket

### What emulation does not establish

`qemu-user` translates aarch64 instructions and runs them against **this
machine's kernel**. Three consequences, none of which the green tick above
covers:

- **Which code path a library picks.** `ring` chooses among its assembly
  implementations from runtime CPU feature detection. Under emulation it is
  interrogating an emulated CPU, not a Cortex-A72, so the path that passed may
  not be the path a Pi takes.
- **The machine.** Answered since: a real Pi 4B ran the installer, the smoke
  check and the benchmark, and peaked at 7.6 MiB. What no machine has met yet is
  an SD card under *sustained* write, where redb's pattern meets erase blocks and
  a controller that lies about flushes.
- **Linux on ARM specifically.** glibc rather than Darwin's libc, ext4 rather
  than APFS, and a kernel that pages differently.

**Memory ordering is not on that list, and an earlier version of this section
put it there.** aarch64 is weakly ordered and x86 is not, and `qemu-user` does
not manufacture the weakness — but `Test (macos-latest)` has been running the
entire suite on Apple silicon since CI first ran. That is real aarch64, with a
real weak memory model and real NEON, on every push. The emulated run adds the
*Linux* half of `aarch64-unknown-linux-gnu`; the ARM half was already covered by
a job nobody had thought of as an ARM job.

Timings say nothing either, in the flattering direction or the other: the CLI's
40 tests took 147 seconds under qemu against about 4 on the host. That measures
the emulator. Real numbers need the Pi, and `itsanas bench` is the command.

```bash
sudo apt install gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --workspace --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/itsanas pi@raspberrypi:
ssh pi@raspberrypi ./itsanas bench --quick
```

That is the command that closes what emulation cannot: it runs on the CPU whose
features `ring` is asking about, under the memory model the emulator does not
reproduce, against an SD card, in 1 GB of RAM. The latency figures then come
from the machine the constants were chosen for rather than from a laptop.

`sh install/linux.sh` does the same thing the long way and ends by running
`scripts/smoke.sh` on the Pi itself, which is the shorter route to the same
answer if the Pi has a toolchain.

The coordinator goes on the Freebox VM with the same binary set:

```bash
scp target/aarch64-unknown-linux-gnu/release/itsanas-coordinator vm:
```

Forward TCP 9898 to it in the Freebox interface. It is the only publicly
reachable component; the limits that make that safe are in
`itsanas-coord::server` rather than in the firewall.

## 3b. Should it be a container? No, and here is what the question was right about

Asked because the Pi in this fleet already carries a VPN in Docker, and piling
services onto one small machine invites the crash that takes the others with it.
That worry is correct. Containers are the wrong answer to it.

**The comparison that matters, stated first.** For the thing actually asked —
one service must not take the machine down with it — a container and a systemd
unit are *the same mechanism*. Both set cgroup limits; Docker's `--memory` and
`MemoryMax=` write to the same controller. So the honest comparison is not
"container versus unit", it is "container with limits versus unit with limits",
and on the question asked they tie. An earlier version of this section argued
against a container by listing what the unit had just gained, which is comparing
the improved version to the unimproved one.

Given a tie on the point at issue, the rest is cost, and the costs are one-sided.

**What a container would not fix** — worth saying because it was the trigger,
not because it was the question. `rustc` dying with `SIGBUS` on a Pi with a
damaged filesystem happens identically inside one: same kernel, same disk.
Container isolation is about namespaces, not about a machine whose storage is
lying.

**What it would cost.**

- **Host networking.** Local discovery is a signed UDP beacon on 21037,
  broadcast on the LAN, and Docker's bridge does not carry broadcast to the
  physical network — so the container needs `--network host`. That is the normal
  deployment for a peer-to-peer daemon rather than a defeat, and plenty of
  production systems run exactly that way. It is a cost because it removes the
  network isolation, which was never the isolation being asked for.
- **A bind mount, carefully.** `redb` memory-maps its index. That is fine on a
  bind mount and a bad idea on an overlay, so the volume is not optional and
  getting it wrong is silent.
- **The same passphrase problem.** A daemon cannot prompt. In a unit that is an
  `EnvironmentFile` with mode 600; in a container it is an environment variable
  or a secrets mount. Neither is better, and the container's is more visible in
  `docker inspect`.
- **An image to build and keep current** — and this is the argument that
  actually decides it, not the three above. A four-machine fleet running Windows,
  macOS and two Linuxes does not repay an image pipeline: three of the four could
  not use it, and the one that could already has a unit that does the same job.
  Images become the right unit of deployment when the number of machines exceeds
  the patience for setting each one up. Four is not that number.

**What the worry was actually asking for, and what was missing.** "One service
must not take the machine down with it" is a resource question, and systemd
answers it directly. The units now carry `MemoryMax`, `MemoryHigh`, `CPUWeight`
and `IOWeight`, and the member unit sets `OOMScoreAdjust=500` — if the kernel
must pick something to kill, pick the storage daemon, which comes back in thirty
seconds, rather than the VPN. That is a smaller change than containerising, it
targets the actual risk, and it was genuinely absent before the question was
asked.

The ceilings are sized from measurement rather than from caution: `itsanas
bench` peaks at **7.6 MiB** on the Pi for a 256 MiB run, because the store
streams. `MemoryMax=512M` is sixty times that, so reaching it means a leak — and
being killed for a leak is the right outcome.

**When this decision should be revisited.** If a member node ever needs to run
untrusted code, or if the fleet grows past the point where per-machine setup is
sensible and images become the unit of deployment. Neither is true of four
machines.

## 4. Android — built, and it runs

### What was measured, on 2026-09-07

The open question was `ring`, which assembles its own primitives for every
target and needs a cross compiler. With NDK 27.3 it builds:

```bash
cargo ndk -t arm64-v8a check -p itsanas-store -p itsanas-net -p itsanas-tls
    Finished `dev` profile in 12.19s
```

Then the whole thing, on an Android 15 emulator, driven through the interface
rather than through a test harness:

1. **Restored an account from its twenty-four words.** Typed into the phone;
   the keystore was created, the master secret derived, the store and the vault
   opened. No `UnsatisfiedLinkError`, no crash, and the account name came back
   on the next screen.
2. **Added a machine and synced.** One peer, `10.0.2.2:9797` — a desktop node
   belonging to a *different account*, holding this one's sealed chunks in its
   vault. Five files arrived: **`5 here · 0 not here · 910 KiB`**, which is
   exactly 120 + 130 + 400 + 60 + 200 KiB of file, so the bytes are on the
   phone rather than merely listed.
3. **Opened one.** The file was written out through a `FileProvider` and handed
   to the system chooser. A `.bin` has no viewer on a bare emulator, so the
   first attempt answered "No apps can perform this action" — a dead end for a
   file the person can plainly see. It falls back to sharing now, which always
   has somewhere to go.
4. **The foreground service ran**, `isForeground=true`, with its notification.

The APK is 19.5 MB with three ABIs inside it, and `scripts/build-apk.sh`
produces it in one command.

### How it is put together

| Piece | Where |
| --- | --- |
| The core | unchanged, cross-compiled |
| Keystore, configuration, one sync round | `crates/itsanas-node`, shared with the command line |
| The JNI boundary | `crates/itsanas-android`, the only crate that relaxes the unsafe lint |
| The application | `android/`, Kotlin and Compose, about 900 lines |

**`itsanas-node` is the part worth explaining.** All of it used to live inside
the command-line binary. Writing the passphrase handling a second time for
Android would have meant two implementations of the most security-sensitive
glue in the project — the key derivation, the refusal of published test
identities, the "a node already exists here" guard — drifting apart from the day
the second one was written. So the binary became a shell over a library, and the
application is a second shell over the same library.

**The JNI boundary is coarse on purpose.** Every crossing allocates and can
throw, so the calls are whole operations — list the account, run a round, fetch
this file — each answering with one JSON string. Errors are Java exceptions
carrying the same sentence the command line prints for the same fault, which is
what makes it possible to help somebody over a telephone.

**The unsafe exception is checked, not asserted.** A JVM calls
`extern "system"` symbols by name, so `#[unsafe(no_mangle)]` is unavoidable and
the workspace's `forbid` cannot cover that crate. `scripts/check-unsafe.py`
fails if any crate holds an `unsafe` block or an `unsafe fn`, and if any crate
but that one relaxes the lint. The sentence "the only unsafe in this project is
the export attribute" is therefore a thing the build verifies.

### What the shell decides, and what it does not

Nothing about *when* to sync. The application reports what it can see — Android
answers "is this connection metered" directly, through
`NET_CAPABILITY_NOT_METERED` — and `itsanas-policy` returns an interval, a scope
and a sentence to show. That is the same decision table the desktop daemon has
been running for weeks, so the phone inherits behaviour that has been exercised
instead of being the first caller of it.

| Situation | What it selects | Interval |
| --- | --- | --- |
| App open, unmetered | everything | 30 s |
| App open, metered | segments only; tap a file to download it | 30 s |
| Background, unmetered | everything | 2 h |
| Background, metered | segments only | 24 h |
| Battery low, not watching | nothing | — |
| "Sync now" pressed | everything, whatever the conditions | once |

### What is still missing

| Piece | Why it matters |
| --- | --- |
| A folder that syncs by itself | The application holds files; it does not watch a directory. Scoped storage means an app may not watch an arbitrary one since Android 10, so this is a design question rather than a port |
| Doze and the `dataSync` budget | Android 14 caps foreground data-sync at about six hours a day. **Written from memory and still unverified** — the twenty-four-hour measurement now running on three desktops is the first honest number this project will have about idle cost |
| A signing key of its own | The APK is signed with the debug key, which is fine for installing by hand and not for anything else |
| Folders in the interface | Every file is listed flat. The paths carry directories and nothing renders them |

## 5. iOS and iPadOS — a different problem, not attempted

Worth stating because "Apple" hides the distinction. A MacBook Air runs macOS: a
full Unix, arbitrary binaries, real background daemons, no sandbox on anything
you compile yourself. iOS shares an instruction set with it and nothing else
that matters here — no sideloading without a developer account, no background
daemons, no arbitrary filesystem.

Everything in §4 applies to iOS and is harder. Nothing has been attempted.
