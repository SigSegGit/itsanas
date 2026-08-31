# Installing ITSaNAS

One script per system. Each one checks the machine before it changes anything,
says what it is doing, and can be run twice without harm.

| System | Script | Tested on |
| --- | --- | --- |
| Linux, including Raspberry Pi and the Freebox VM | [`linux.sh`](linux.sh) | Ubuntu 22.04 **x86-64**, bare image, no toolchain, twice, ending in a real store-and-read-back. The binary it builds is exercised on emulated aarch64 by CI; **the installer itself has never run on ARM** |
| Windows 10 and 11 | [`windows.ps1`](windows.ps1) | Windows 11, PowerShell 5.1, **full run**: built, installed, binary ran |
| macOS, Apple silicon and Intel | [`macos.sh`](macos.sh) | **not yet run on a Mac** |
| Android, through Termux | [`android-termux.sh`](android-termux.sh) | **not yet run on a phone**; refuses correctly outside Termux and under `--check` |
| Android, as an app | [`android.md`](android.md) | there is no app to install |
| A coordinator on a machine with a public address | [`coordinator.sh`](coordinator.sh) | **not yet run on the Freebox VM** |

That last column is the point of this table. Say plainly which of these has been
executed on the system it claims to install, because an installer nobody has run
is a hypothesis with a shebang.

Every installer for a member node now ends by proving its own work: it creates
an account, checks the recovery phrase is still 24 words, stores a file across
several chunks and reads it back byte for byte. On Linux, macOS and Termux that
is `scripts/smoke.sh`, which also runs `doctor`; Windows has no `sh`, so the
same steps are written out in `windows.ps1`. The coordinator is the exception
and stays a `--version` check, because it has no store to exercise -- it holds
addresses and sealed blobs it cannot open.

What that replaced was a final `itsanas --version`, which proves the kernel can
execute the file and nothing else. On a Pi or a phone the difference is the whole question, so the answer
arrives on the machine rather than being inferred from a laptop. Skip it with
`--no-smoke` if you need the install regardless.

## Linux, Raspberry Pi, the Freebox VM

```sh
sh install/linux.sh
```

Or check the machine without changing anything:

```sh
sh install/linux.sh --no-build
```

It refuses rather than guesses when it matters:

- **32-bit ARM.** A Pi 3 or 4 running a 32-bit Raspberry Pi OS is 64-bit
  hardware with the wrong image. It says so and how to fix it, rather than
  building for an hour and failing at the link.
- **Not enough memory.** `cargo build --release` peaks near 1.5 GB. On a 1 GB Pi
  with no swap, rustc is killed by the OOM reaper and cargo reports `signal: 9`
  or a linker error that has nothing to do with the cause. The script measures
  RAM plus swap first and prints the `dphys-swapfile` commands.
- **A missing C compiler.** blake3 assembles NEON code on aarch64 through `cc`,
  and its build script fails with "failed to find tool", which nobody connects
  to `build-essential`.

It installs a **systemd user unit**, not a system one: the daemon needs the
passphrase that unlocks the keystore and writes into the user's home. Running it
as root would put the keys where the user cannot read them and give a storage
daemon privileges it has no use for. On a headless Pi you also want
`sudo loginctl enable-linger <user>`, or the daemon stops when you log out — the
script checks and tells you.

## Windows

```powershell
powershell -ExecutionPolicy Bypass -File install\windows.ps1
```

`-ExecutionPolicy Bypass` on that one command, rather than the script changing
your policy permanently. An installer that loosens a security setting to run
itself is teaching a habit worth not having.

The check that matters here is the **linker**. Rust on Windows links with
Microsoft's `link.exe`, which does not ship with Windows and is not part of
Rust. Without it the build runs for twenty minutes and then fails with
``linker `link.exe` not found`` — which reads like a Rust problem and sends
people to reinstall Rust. The script looks for the Build Tools with `vswhere`
before starting, and prints the `winget` line if they are absent.

It does **not** register a scheduled task automatically. The daemon needs a
passphrase, and storing one in your registry hive where anything running as you
can read it is a decision to make on purpose. The script prints the two commands
and says what the trade is.

## macOS

```sh
sh install/macos.sh
```

Not yet run on a Mac. It handles the things that differ — the Xcode command line
tools that a fresh Mac lacks and that pop a graphical installer, Rosetta making
an Apple-silicon machine claim to be x86_64, and a LaunchAgent instead of a
systemd unit — but until somebody runs it on a Mac, treat the table above as the
honest statement.

The LaunchAgent is written and deliberately **not loaded**: same reason as
Windows. Everything in `~/Library/LaunchAgents` is readable by anything running
as you.

## A coordinator, on the machine with the public address

A different role, so a different script:

```sh
sudo sh install/coordinator.sh
```

A coordinator is a notice board. It holds usernames, device addresses and
sealed escrow blobs it cannot open — no file data, no user keys, nothing it can
read. If it disappears, members keep syncing with the peers they already know;
they simply cannot find new ones.

That is why its setup differs from a member node's in three ways, and the script
does all three:

- **A system service under its own user.** It is the only machine in a fleet a
  stranger can reach unprompted, so it owns nothing but its own state directory
  and gets a systemd sandbox to match. A member node's daemon holds your keys
  and runs as you; this one holds nothing and must not.
- **`--invite-only` from the start.** Otherwise "who is a member" means "anyone
  who can open a socket". The script prints what to do about the first member,
  which needs `--admit-first` once because an invitation to admit them would
  have no author.
- **It prints its device id.** Members pin it: a coordinator supplies addresses
  and is never trusted to say who lives at one.

It needs no passphrase, which is what lets it be a system service at all. It
holds only its own device key.

Before running it, check the machine can actually be reached:

```sh
sh install/coordinator.sh --check
```

That looks at whether the port is free and whether any of this machine's
addresses is routable, and says what to forward if not — on a Freebox that is
Paramètres > Gestion des ports.

## Android

```sh
pkg install git && git clone <repo> && cd itsanas
sh install/android-termux.sh
```

**This installs the command-line tool on your phone. It is not a sync app, and
there is no sync app.** No APK, no file picker, no background service; Android
will kill a daemon left running overnight whatever you do about wake-locks.
[`android.md`](android.md) says what a real client would take and why none of it
is written.

What it is for is the one thing a phone is genuinely good for here: half the
constants in this project are chosen for ARM devices, and a phone is the ARM
device most people own. The script builds `itsanas` for the phone's own
processor and then stores a file and reads it back on it. CI runs the same check
under emulation on every push; a phone is the real thing.

Termux's package mirror is down or stale often enough that "E: Unable to locate
package rust" is the most common way this fails, and it reads as if the package
does not exist. The script handles that case by name and tells you to run
`termux-change-repo`.

Install Termux from **F-Droid**. The Google Play build is unmaintained and
ships a 32-bit userland on some devices, which the script detects and refuses.

## After installing, on any of them

```sh
itsanas init --username <your-name>   # writes down 24 words; keep them
itsanas pledge 100G                   # space you offer other members
itsanas folder ~/Sync                 # the directory kept in step
```

Then either a coordinator, so machines on different networks find each other:

```sh
itsanas-coordinator --identity            # on the coordinator, prints its id
itsanas coordinator <host:port> --device <that-id>
itsanas register
```

or a peer on the same network, directly:

```sh
itsanas peer add <host:port>
```

And run it: `itsanas daemon`, or the service the installer set up.

`docs/QUICKSTART.md` goes further, including what `itsanas status` is telling
you and how invitations work once the coordinator is somewhere strangers can
reach it.

## If something goes wrong

Every failure in these scripts prints what it was doing, what it expected, and
what to try. If one of them prints something that leaves you stuck, that is a
bug in the script and worth reporting — an installer whose error message needs
a person to interpret it has not finished its job.
