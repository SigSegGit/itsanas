# Installing ITSaNAS

One script per system. Each one checks the machine before it changes anything,
says what it is doing, and can be run twice without harm.

| System | Script | Tested on |
| --- | --- | --- |
| Linux, including Raspberry Pi and the Freebox VM | [`linux.sh`](linux.sh) | Ubuntu 22.04 **x86-64**, bare image, no toolchain, twice. Never on ARM |
| Windows 10 and 11 | [`windows.ps1`](windows.ps1) | Windows 11, PowerShell 5.1, **full run**: built, installed, binary ran |
| macOS, Apple silicon and Intel | [`macos.sh`](macos.sh) | **not yet run on a Mac** |
| Android | [`android.md`](android.md) | there is no app to install |
| A coordinator on a machine with a public address | [`coordinator.sh`](coordinator.sh) | **not yet run on the Freebox VM** |

That last column is the point of this table. Say plainly which of these has been
executed on the system it claims to install, because an installer nobody has run
is a hypothesis with a shebang.

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

See [`android.md`](android.md). The core compiles for `aarch64-linux-android`
and CI checks it on every push; the app does not exist. That file says what it
would take rather than pretending otherwise.

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
