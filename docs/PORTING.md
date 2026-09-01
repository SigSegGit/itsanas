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
| Full test suite | ✅ CI, every push |
| Run on real hardware | ❌ nobody has |
| Platform-specific code | one `cfg(unix)` for key file permissions |

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

## 3. Raspberry Pi and the Freebox VM — the VM is done, the Pi is not

**The Freebox Delta VM has run it.** aarch64 Ubuntu 26.04, 2 vCPU, 11 GB,
installed on 2026-09-01 by `curl … | sh` on a machine that had no compiler and
no Rust on it. The build took 5m37s. Then, on that machine:

- **640 tests pass, none fail** — the whole suite plus the three `#[ignore]`d
  ones, natively, no emulator
- `scripts/smoke.sh`: an account, a 24-word phrase, a 350 KB file across five
  chunks read back byte for byte, `doctor` clean —
  `PASS: ITSaNAS stored and returned a file -- native aarch64`
- `itsanas bench --quick`: it **saves a 512 KiB document in 10 ms against the
  laptop's 29 ms**, on a machine that chunks 4.6× slower. See ROADMAP.md M9 for
  why, and for what that says about pack files.

So the coordinator's future home is no longer a hypothesis. What is left is the
Pi: 1 GB of RAM against this VM's 11, and an SD card against its virtual disk.

### What emulation established before that, and what it did not

Both are `aarch64-unknown-linux-gnu`. CI also cross-builds the whole workspace on
every push and then **runs it on that architecture** under `qemu-user-static`:
**637 pass, 3 `#[ignore]`d, none fail.** That is 635 of the project's 638 test
functions plus its 2 doctests — the arithmetic is worth spelling out because
`TESTING.md` says 638 and this says 637, and a reader who cannot reconcile two
numbers on the same subject is right not to trust either. Then `scripts/smoke.sh` creates an
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
- **The machine.** A Pi 4B has 1 GB of RAM and a runner has 16, and nothing here
  says the index fits. Nor has anything met an SD card, where redb's write
  pattern meets erase blocks and a controller that lies about flushes.
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

## 4. Android — the core compiles, the shell does not exist

### What was actually measured

```bash
rustup target add aarch64-linux-android
cargo check -p itsanas-crypto -p itsanas-store -p itsanas-sync \
            -p itsanas-policy -p itsanas-discover -p itsanas-placement \
            -p itsanas-wire --target aarch64-linux-android
```

**Passes.** That is identity, key schedule, sealing, blinded addressing,
chunking, the blob store, the index, the operation log, version vectors,
conflict resolution, the vault, framing, local discovery and the sync policy —
the entire data path — type-checking for Android with no changes.

```bash
cargo check --workspace --target aarch64-linux-android
```

**Fails**, at `ring`, for a missing C compiler. Not a code problem: `ring`
assembles its own primitives on every target and needs a cross compiler, exactly
as the Pi build needs `gcc-aarch64-linux-gnu`. On Android that compiler is the
NDK, and the standard tool that wires it up is `cargo-ndk`:

```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release
```

*Untried here — this machine has no NDK. It is the documented path, not a
verified one.*

### What still has to be written

| Piece | Why the existing code cannot serve | Size |
| --- | --- | --- |
| An FFI boundary | Kotlin cannot call Rust functions directly | small, and the only `unsafe` in the project |
| A replacement for `itsanas-folder` | `notify` plus scoped storage: an app may not watch an arbitrary directory since Android 10 | medium |
| A replacement for `itsanas-cli` | no terminal, no `argv`, no signals | medium |
| A foreground service | the system kills background processes; periodic work goes through `WorkManager` | medium |

### The browse-then-download behaviour — built

`itsanas_store::catalogue` reports every file the account has, marking each
`Local` or `Absent`, by combining the index with a walk of the vault's log
segments. So a client on a metered connection shows the whole account and
fetches on demand, which is the Drive model.

Derived from the vault rather than recorded in a table, so it cannot go stale.
It does not write an index entry for an absent file: that would break the
invariant that a listed file is readable, which the conflict and delete logic
both assume.

Reachable from the command line too — `itsanas sync --metadata-only`, then
`itsanas ls` — because a laptop tethered to a phone wants it as much as a phone
does. This was the last piece missing from the *core*; what remains for Android
is shell work.

### The sync policy, decided and running

Implemented and tested in `itsanas-policy`, and **`itsanas daemon` is its first
consumer**. The daemon no longer carries a hard-coded interval: it asks the
policy, prints the interval, the scope and the reason, and honours `--interval`
when an operator wants to decide instead.

```text
itsanas daemon                    itsanas daemon --metered
  interval  300s                    interval  86400s
  syncing   everything              syncing   the log only (no file contents)
  because   running as a           because   metered connection — checking
            service on an                     for changes only, once a day
            unmetered connection
```

That means the Android shell inherits behaviour that a desktop has already
exercised, rather than being the first thing ever to call this crate.

Wiring it added the state that had been missing: a **service**. A daemon is not
a backgrounded app — nobody is watching it *and* no platform is restricting it
— and conflating the two would have given the Pi in the cupboard a two-hour
interval. Two hours is not a considered choice about ethernet; it is the
smallest number that survives Android's Doze. See `Attention::Unattended`.

The one thing still asked for rather than detected is whether the connection is
metered. Windows and macOS both expose it, and reading it would mean a platform
crate for a single flag; `--metered` is honest in the meantime, and guessing
from the interface type is refused outright.

The rule is **metered or not**,
never Wi-Fi or not: a phone's own hotspot is Wi-Fi and charged by the gigabyte,
and plenty of mobile plans are unlimited. Android answers the right question
directly through `NET_CAPABILITY_NOT_METERED`.

| Situation | What it would select | Interval |
| --- | --- | --- |
| App open, unmetered | everything | 30 s |
| App open, metered | segments only; tap a file to download it | 30 s |
| Background, unmetered | everything | 2 h |
| Background, metered | segments only | 24 h |
| **Service, unmetered** | **everything** | **5 min** |
| **Service, metered** | **segments only** | **24 h** |
| Battery low, not watching | nothing | — |
| "Sync now" pressed | everything, whatever the conditions | once |

The two service rows are what `itsanas daemon` selects today. A service is
exempt from the "background syncing is off" switch — starting a daemon is the
deliberate act that switch exists to require — but not from the low-battery
rule, because nothing about being a service makes the battery bigger.

Two deliberate choices. The button always works: a button that does nothing
teaches people the application is broken, and somebody pressing it on mobile
data has decided. And a low battery never stops a person who is watching — they
can see their own battery indicator.

### Two Android constraints worth knowing before starting

**Android 15 caps `dataSync` foreground services** at roughly six hours per day
in total. That is the service type a continuous sync would use. It does not
prevent the design above — 2 h and 24 h intervals are `WorkManager` periodic
work, not a foreground service — but it does rule out "always running".

**Samsung One UI puts apps into deep sleep** far more aggressively than stock
Android. Without the user excluding the app from battery optimisation, periodic
work stops. That is a checkbox somebody has to find, and it is the sort of
friction that gets an application uninstalled.

*Both from memory; verify against current Android documentation before building
against them. Background execution rules change with almost every release.*

### And the reason a phone is a client, not an anchor

[ECONOMICS.md](ECONOMICS.md) §2 argues that the network needs always-on anchors,
and a phone looks like an excellent one — far higher uptime than a laptop.

It is not, and the reason has nothing to do with battery. **A phone on mobile
data is behind carrier-grade NAT and cannot be dialled at all.** It can push and
it can poll; nothing can reach it. On home Wi-Fi it becomes reachable, and there
local discovery already handles it with no coordinator and no app.

So the value of a phone here is as a client — which is what it was wanted for
anyway.

## 5. iOS and iPadOS — a different problem, not attempted

Worth stating because "Apple" hides the distinction. A MacBook Air runs macOS: a
full Unix, arbitrary binaries, real background daemons, no sandbox on anything
you compile yourself. iOS shares an instruction set with it and nothing else
that matters here — no sideloading without a developer account, no background
daemons, no arbitrary filesystem.

Everything in §4 applies to iOS and is harder. Nothing has been attempted.
