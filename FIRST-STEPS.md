# First steps

Fifteen minutes from nothing to two machines keeping a folder in step. Pick your
machine, run one command, then follow **After the install** at the bottom.

> **Read this first.** This is v0.1.0 — a first tagged version so it can be
> tested, not a product. The on-disk format may change without a migration.
> **Keep another copy of anything you put in it.**

Everything here builds from source on the machine it runs on. There are no
prebuilt binaries and that is deliberate: the fleet this is written for is a
Windows laptop, a Raspberry Pi and an ARM virtual machine, and a build on each
is both the artefact and the proof it works there. Expect the first run to take
ten to forty minutes, mostly compiling.

---

## Linux, Raspberry Pi, a VM

```sh
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/linux.sh | sh
```

Installs a toolchain if there is none, builds, installs to `~/.local/bin`,
writes a systemd user unit, and finishes by storing a file and reading it back
so you know it works on that machine rather than in principle.

Refuses a 32-bit userland — including the 64-bit Pi running a 32-bit image,
which is the common case and fails an hour into the build without the check.

## Windows 10 and 11

```powershell
powershell -ExecutionPolicy Bypass -File install\windows.ps1
```

From a clone. `-ExecutionPolicy Bypass` on that one command rather than a
permanent change to your machine.

It checks for the **linker** before anything else. Rust on Windows links with
Microsoft's `link.exe`, which does not ship with Windows and is not part of
Rust; without it the build runs twenty minutes and then fails with a message
that sends people to reinstall Rust rather than the Visual Studio Build Tools
they actually need.

## macOS

```sh
sh install/macos.sh
```

Handles the Xcode command line tools a fresh Mac lacks. Built and smoke-tested
on Apple silicon in CI on every push; **never run on an Intel Mac**.

## Android

Two different things, and only the first is an app.

```sh
sh scripts/build-apk.sh          # needs the Android SDK and NDK 27.3
```

Kotlin and Compose over the same Rust core. Restores an account from its
twenty-four words, adds a machine, syncs in a foreground service, opens a file.
There is no Play Store listing, so installing it means enabling unknown sources.
Tested on an emulator, **never on a physical handset**.
See [`install/android.md`](install/android.md) for what it does not do.

The other is the command line inside [Termux](https://termux.dev), which is how
you run the real test suite on real ARM silicon:

```sh
pkg install git && git clone https://github.com/SigSegGit/itsanas && cd itsanas
sh install/android-termux.sh
```

## A coordinator

Only if you have a machine with a public address and want members to find each
other without typing addresses. It is a directory of addresses and sealed blobs;
it cannot read anything.

```sh
sudo sh install/coordinator.sh --check     # look first
```

---

## After the install

### 1. Make an account, on your first machine

```sh
itsanas init --username you
```

It prints **twenty-four words, once**. They are the only way to recover this
account on another machine, they are stored nowhere, and they cannot be
reissued. Write them down on paper now.

### 2. Decide how much space, and for whom

Two numbers, bound to each other. What you **pledge** is room for other people's
data; what you **keep** is room this machine may use for your own. Keeping a
byte of your own costs three pledged — a network where everyone stores and
nobody hosts has no storage in it — with a 10 GiB allowance for the first thirty
days so a new member is useful before it has earned anything.

Ask before committing to either:

```sh
itsanas space --pledge 100G --keep 30G
```

It reports the free space on the disk this node actually sits on, what the
pledge earns, and which limit binds. It changes nothing without `--apply`, and
it refuses to apply numbers it has just said do not fit.

**This bargain is enforced by your own client, not yet by the network.** A
modified client can ignore it and nothing will notice — fine among your own
machines, not yet fine among strangers. See `docs/ECONOMICS.md` §1.

### 3. Point it at a folder, and run it

```sh
itsanas folder ~/Sync
itsanas daemon
```

The daemon watches the folder, syncs with peers, answers storage challenges and
serves what it hosts. On Linux the installer already wrote a systemd unit:
`systemctl --user enable --now itsanas`.

### 4. Add a second machine

Install there, then restore the **same** account from the twenty-four words:

```sh
itsanas login --username you --phrase-file words.txt
itsanas peer add 192.168.1.42:9797
itsanas sync
```

Both machines now hold the same account. Drop a file in the folder on one; it
appears on the other.

For a machine that cannot hold everything — a phone, a laptop you do not want to
give the whole account to:

```sh
itsanas keep 2G --order newest
```

Everything is still listed; what is not held says so, and asking for one fetches
it.

### 5. Check on it

```sh
itsanas status
itsanas doctor
```

---

## Removing it

Every installer takes `--clean` (`-Clean` on Windows) and hands the work to one
uninstaller. It is a dry run by default:

```sh
sh install/linux.sh --clean          # list what would go
sh install/linux.sh --clean --yes    # do it
```

It leaves your account alone unless asked twice: `~/.itsanas` holds this
machine's sealed copy of the master secret and the only copy of anything not yet
replicated elsewhere. `--purge-account` removes it, and that is losing data
rather than uninstalling a program.

---

## When it does not work

`itsanas doctor` first — it checks the things that are usually wrong and says
what to do. Then:

| Longer | Where |
| --- | --- |
| Every install path, and which have been run on real hardware | [`install/README.md`](install/README.md) |
| Using it: budgets, conflicts, three machines, unattended | [`docs/QUICKSTART.md`](docs/QUICKSTART.md) |
| Why it is built this way | [`docs/DESIGN.md`](docs/DESIGN.md) |
| What is not built, with the arithmetic | [`docs/ROADMAP.md`](docs/ROADMAP.md) |

## What this version is not

Stated plainly, because a version number invites the opposite assumption:

- **Not tested at scale.** Three machines and two accounts, all belonging to one
  person. Nothing here has met a stranger.
- **Not proven at a terabyte.** An idle account of that size costs about 600 KB
  a day to verify; one that changes costs roughly 2 MB per differing bucket, and
  the arithmetic is in `docs/DESIGN.md` §6.5 rather than in a reassurance.
- **Not yet safe among strangers.** The 3-for-1 storage bargain is enforced by
  the honest client only; a rebuilt client can take more than it gives.
  `docs/ROADMAP.md` lists what an adversarial sweep found and did not fix.
- **Not a backup.** Two copies on machines you know is not an archive, and
  nothing here is off-site by default.
