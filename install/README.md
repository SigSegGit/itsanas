# Installing ITSaNAS

One script per system. Each one checks the machine before it changes anything,
says what it is doing, and can be run twice without harm.

| System | Script | Tested on |
| --- | --- | --- |
| Linux, including Raspberry Pi and the Freebox VM | [`linux.sh`](linux.sh) | Ubuntu **x86-64** from a checkout; Ubuntu 26.04 **aarch64** on a Freebox Delta VM through the `curl \| sh` one-liner, on a machine with no compiler and no Rust on it; and **a Raspberry Pi 4B on an SSD**, the same way. All three ended in a real store-and-read-back, and on both ARM machines the whole test suite then passed natively — on the Pi including the three `#[ignore]`d tests. This row used to end "never on a Raspberry Pi, which has a tenth of that VM's memory": both halves were wrong by then, the Pi having 3.8 GB against the VM's 11 GB, and the sentence survived because nothing checks a claim written in prose |
| Windows 10 and 11 | [`windows.ps1`](windows.ps1) | Windows 11, PowerShell 5.1, **full run, twice**: built, installed, stored and read back a file, and on 2026-09-06 created the account `sigseg42`, joined the Raspberry Pi's coordinator, and exchanged data with a second account on another machine in both directions. The second run is what found the bug below |
| macOS, Apple silicon and Intel | [`macos.sh`](macos.sh) | macOS 26.5.2 **Apple silicon**, in CI on every push: built, installed, and stored and returned a file natively on arm64. Never on Intel |
| Android, through Termux | [`android-termux.sh`](android-termux.sh) | **not yet run on a phone**; refuses correctly outside Termux and under `--check` |
| Android, as an app | [`android.md`](android.md), built by `scripts/build-apk.sh` | **Android 15 emulator, 2026-09-07, end to end through the interface**: restored an account from 24 words, added a desktop peer belonging to a *different* account, pulled 5 files and 910 KiB from it, opened one, and left the foreground service running. Never on physical hardware, never on a manufacturer skin, and never installed from anywhere but `adb install` |
| Any Linux, from nothing to a running member | [`provision.sh`](provision.sh) | **Run end to end on a freshly imaged Raspberry Pi 4B (Debian 13, SSD) on 2026-09-01**, and again on the Freebox VM to create a second account by invitation. From a machine with no compiler: toolchain, build, install, account, pledge, synced folder, coordinator, registration, systemd unit, and a smoke test that stores and returns a file. Run twice on the same machine to check it changes nothing. Three faults it had are in the git log — the service branch was unreachable, the idempotence guard could not tell "no node" from "node busy", and `systemctl --user` failed in the detached context a reinstall script actually runs in. All three needed a machine it had already succeeded on |
| Windows, from nothing to a running member | [`provision.ps1`](provision.ps1) | **Run on Windows 11 on 2026-09-06**: refuses without a passphrase and without a username, and its idempotent path was exercised against an already-provisioned node. It is the Windows half of `provision.sh` and carries the same three corrections — the passphrase from the environment only, the secret file locked down before the secret goes in, and idempotence decided by looking for the keystore rather than by asking a program that cannot tell "no node" from "node busy" |
| A coordinator on a machine with a public address | [`coordinator.sh`](coordinator.sh) | **Run for real twice**: on the Freebox VM (2026-09-01, service enabled at boot, admitted the first member) and on the Raspberry Pi (2026-09-01, `--check` first, then `--admit-first`, which founded the account `nicolas` and then admitted `voisin` on an invitation). `--check` also exercised on Linux with a busy port and a missing binary |

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

## From nothing to a running node, in one command

`linux.sh` compiles and installs. It touches no keys, creates no account and
writes no secret — deliberately. Getting from *installed* to *a member of
something* was five more commands in an order nobody had written down, half of
them needing values from another machine.

[`provision.sh`](provision.sh) is that order, written down:

```sh
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/provision.sh |
  ITSANAS_PASSPHRASE='...' sh -s -- \
    --username nicolas --pledge 100G --keep 10G --folder ~/Sync \
    --coordinator 192.168.1.11:9898 --coordinator-device <its id>
```

On Windows the same thing, with the same flags under PowerShell names:

```powershell
$env:ITSANAS_PASSPHRASE = 'a long one you have written down'
powershell -ExecutionPolicy Bypass -File install\provision.ps1 `
  -Username nicolas -Pledge 100G -Keep 10G -Folder "$env:USERPROFILE\ITSaNAS-Cloud" `
  -Coordinator 192.168.1.10:9898 -CoordinatorDevice <its-id> -Invite <code>
```

It ends by registering a scheduled task that starts the daemon at logon, which
needs the passphrase in a file only your account can read. `-NoTask` skips that
and leaves you to run `itsanas daemon` yourself.

It installs, creates or restores the account, sets the pledge and the folder,
pins and registers with the coordinator, writes the passphrase where systemd can
read it, enables the service, and finishes by storing a file and reading it back.
Run it twice and the second run changes nothing — `itsanas init` refuses to
overwrite an existing node, which is what makes that safe.

**It handles the passphrase, and that is why it is a separate script.** A daemon
cannot be prompted, so the passphrase goes into
`~/.config/itsanas/environment` with mode 600, and anything running as you can
read that file. That is the trade a background service makes. It belongs in a
script you read before running rather than in an installer's last step.

For a second machine on the same account, add `--phrase-file` with the
twenty-four words in it, and `--invite` if the coordinator admits by invitation.

**Not a container**, and the reasoning is in
[docs/PORTING.md](../docs/PORTING.md) §3b. The short version: for "one service
must not take the machine down with it" a container and a systemd unit are the
same mechanism, and the reproducibility a container would add needs a published
image, which does not exist — so on ARM you would build it on the Pi, for
exactly what building the binary costs. This script is the reproducible artefact
instead.

## Linux, Raspberry Pi, the Freebox VM

One line, on a machine with nothing on it:

```sh
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/linux.sh | sh
```

It clones, builds, installs, writes a systemd user unit, and then stores a file
and reads it back to prove the result works on that machine. Or from a checkout
you already have:

```sh
sh install/linux.sh
```

Or look at the machine without changing anything:

```sh
sh install/linux.sh --no-build
```

That one line had been printed at the top of `linux.sh` since the day it was
written and had never been run. It was broken twice over: the URL was a
placeholder, and under a pipe **stdin is the script**, so the first prompt would
have eaten the lines the shell had not executed yet. Both are fixed and the
line above is what was actually run.

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

There are two things here and they are not the same thing.

**The app.** Kotlin and Compose over the same Rust core, through a JNI boundary
in `crates/itsanas-android`. It restores an account from its twenty-four words,
adds a machine, runs sync rounds in a foreground service, lists the account and
opens a file. Build it in one command:

```sh
sh scripts/build-apk.sh
```

The APK lands in `android/app/build/outputs/apk/`. It carries three ABIs and
weighs about 19.5 MB. There is no Play Store listing and no signed release
channel, so installing it means enabling unknown sources on the phone — which
is a real cost and the honest state of things, not a step being skipped here.

What it does not do: no automatic photo backup, no storage-access-framework
folder watching, no doze-proof scheduling. Android will still stop a background
service on a manufacturer skin that decides to, and the foreground notification
only makes that less likely. [`android.md`](android.md) has the details and the
measurements.

**The command line, through Termux**, which is a different and older answer:

```sh
pkg install git && git clone https://github.com/SigSegGit/itsanas && cd itsanas
sh install/android-termux.sh
```

This builds `itsanas` for the phone's own processor and stores a file and reads
it back on it. It is worth having even now the app exists: half the constants in
this project are chosen for ARM devices, and this is the check that runs the
real test suite on the real silicon rather than under emulation.

Termux's package mirror is down or stale often enough that "E: Unable to locate
package rust" is the most common way this fails, and it reads as if the package
does not exist. The script handles that case by name and tells you to run
`termux-change-repo`.

Install Termux from **F-Droid**. The Google Play build is unmaintained and
ships a 32-bit userland on some devices, which the script detects and refuses.

## How much space, and for whom

Two numbers, and they are not the same number. **The pledge** is room you offer
other members; **keep** is room this machine may use for your own data. A disk
has to hold both.

They are bound to each other and to the disk:

- Keeping a byte of your own costs **three pledged**. That ratio is the whole
  bargain — a network where everyone stores and nobody hosts has no storage in
  it — and it is stated once, in `itsanas-coord::accounting`.
- For the first thirty days a new member may keep **10 GiB** whatever they
  pledge, so a machine can be useful before it has earned anything.
- Neither number may exceed what the disk the node sits on can actually give,
  counting what is already there.

So offering 90 GiB earns 30 GiB, and asking for 31 GiB is refused:

```sh
itsanas space --pledge 90G --keep 31G
```

```
that does not fit:
  keeping 31.0 GiB needs 93.0 GiB pledged; you are offering 90.0 GiB
```

`itsanas space` on its own reports the current bargain and changes nothing.
Add `--apply` to set the numbers, and it refuses to set any it has just said do
not fit.

The provisioners take both and hand them to the same command, so an installer
cannot accept numbers a coordinator will reject later:

```sh
sh install/provision.sh --username nicolas --pledge 100G --keep 10G
```

```powershell
powershell -ExecutionPolicy Bypass -File install\provision.ps1 `
  -Username nicolas -Pledge 100G -Keep 10G
```

Leaving `--keep` out means "keep everything here", which is the right answer for
a laptop that is the only copy and the wrong one for a phone. It is bounded by
the ratio all the same.

## Removing it

Every installer takes `--clean` (`-Clean` on Windows), and every one of them
hands the work to a single uninstaller — [`clean.sh`](clean.sh) and
[`clean.ps1`](clean.ps1). Three lists of paths that drift apart is how a machine
ends up with a service pointing at a binary something else removed.

It is a dry run by default, because the first thing anybody does with an
unfamiliar clean-up script is run it to see what it says:

```sh
sh install/linux.sh --clean          # list what would go
sh install/linux.sh --clean --yes    # do it
```

```powershell
powershell -ExecutionPolicy Bypass -File install\windows.ps1 -Clean
powershell -ExecutionPolicy Bypass -File install\windows.ps1 -Clean -Yes
```

It takes the service or scheduled task first — removing a binary out from under
a running daemon leaves a process with a deleted executable — then the binaries,
the passphrase file, the wrapper script and the logs.

**It leaves the account alone unless asked twice.** `~/.itsanas` holds this
machine's sealed copy of the master secret and the only copy of anything not yet
replicated elsewhere; removing it is losing data, not uninstalling a program, so
it takes its own flag:

```sh
sh install/clean.sh --yes --purge-account
```

One thing it cannot do: other members still count this machine as holding their
data until their next audit withdraws it. It says so at the end. If this machine
was a host for somebody, tell them.

## After installing, on any of them

```sh
itsanas init --username <your-name>   # writes down 24 words; keep them
itsanas pledge 100G                   # space you offer other members
itsanas folder ~/Sync                 # the directory kept in step
itsanas listen 0.0.0.0:9797           # only if 9797 is taken here
```

To host with somebody on another network, you need their name and nothing else:

```sh
itsanas peer find mandarine
```

The coordinator turns the name into their machines' addresses. On one network
the discovery beacons do this without being asked.

`itsanas device list` shows every machine the coordinator has for your account,
and `itsanas device forget <id>` withdraws one that is gone for good. Without
it a laptop that was reinstalled or sold stays in the directory and every other
machine you own keeps dialling it. The short id from the log is enough:

```sh
itsanas device forget 393f7d4acf72
```

`listen` matters when something else already holds 9797 on the machine — a
second node, or another program. Set it *before* `register`, because
registering is what publishes the address: change it afterwards and the
coordinator keeps handing other members a port this node does not answer on.
`itsanas listen` with no argument prints the current one.

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
