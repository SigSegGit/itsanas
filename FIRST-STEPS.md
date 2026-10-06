# First steps

Fifteen minutes from nothing to two machines keeping a folder in step. Pick your
machine, run one command, then follow **After the install** at the bottom.

> **Read this first.** This is v0.1.0 — a first tagged version so it can be
> tested, not a product. The on-disk format may change without a migration.
> **Keep another copy of anything you put in it.**

From a clone, build the tag rather than `main`:
`git clone --branch v0.1.0 https://github.com/SigSegGit/itsanas`.

**No compiler? Install a release.** Windows:
`irm https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.ps1 | iex`. Linux/macOS:
`curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.sh | sh`. It downloads the latest release, checks it, installs it and starts
`itsanas setup`. This works only once Nicolas has published a first signed
release; until then, compile as below.

Everything here builds from source on the machine it runs on. There are no
prebuilt binaries and that is deliberate: the fleet this is written for is a
Windows laptop, a Raspberry Pi and an ARM virtual machine, and a build on each
is both the artefact and the proof it works there. Expect the first run to take
ten to forty minutes, mostly compiling.

---

## Linux, Raspberry Pi, a VM

```sh
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/v0.1.0/install/linux.sh | ITSANAS_REF=v0.1.0 sh
```

Installs a toolchain if there is none, builds, installs to `~/.local/bin`
(and offers to put it on your PATH in your login profile), writes a systemd user unit, and finishes by storing a file and reading it back
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

Installs the Xcode command line tools a fresh Mac lacks and waits for them
(at most an hour), then offers to add one marked line to `~/.zprofile` so
`itsanas` is found in a new terminal (`--clean` removes it). Built and
smoke-tested on Apple silicon in CI on every push, and on a real Apple-silicon
Mac on 2026-10-05; **never run on an Intel Mac**, and the wait for the tools
has not yet been seen on a fresh Mac.

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

A test account holds its username until the operator frees it: stop the
coordinator, run `itsanas-coordinator --state <its state dir> --forget-account
NAME`, start it again. Withdrawn devices stay withdrawn.

---

## After the install

### The easy way: `itsanas setup`

Run `itsanas setup` (add `--instance NAME` for a second account on the same
computer). On a computer with a screen it opens a page in your browser and
walks you through each step: this machine, the account (new, or one you
already have), joining the network, the space you offer, the folder, a
connectivity check, the background service and a final check. If no page
opens, paste the address it prints into your browser; it only works on that
computer and only while the command runs.

**Your 24 recovery words and your passphrase are never typed into the page:** a
separate ITSaNAS window asks for them (look in your taskbar / Dock if you
cannot see it). **Never type your recovery words into a web page.** A new
account shows the 24 words once and asks three of them back before anything is
written. If a step fails, it says the one thing to do; run `itsanas setup` again
afterwards: it skips what is done and never makes a second account.

`itsanas setup --text` asks the same questions in the terminal, and an SSH
session uses the terminal by itself. Later, `itsanas settings` opens the same
page to pause or resume, sync now, change how often it syncs, the space you
offer, your folder or your coordinator, or to sign out (changing space, folder
or coordinator restarts ITSaNAS, a few seconds).

The numbered steps below are the same thing done by hand, the alternative.

### 1. Make an account, on your first machine

```sh
itsanas init --username you
```

It prints **twenty-four words, once**. They are the only way to recover this
account on another machine, they are stored nowhere, and they cannot be
reissued. Write them down on paper now.

### 2. Decide how much space, and for whom

Two numbers, bound to each other. What you **pledge** is room for other people's
data; what you **keep** is room this machine may use for your own. Keeping
three bytes of your own costs seven pledged — a **30/70** split of everything
this machine commits, because a network where everyone stores and nobody hosts
has no storage in it — with a 10 GiB allowance for the first thirty days so a
new member is useful before it has earned anything.

Ask before committing to either:

```sh
itsanas space --pledge 100G --keep 30G
```

It reports the free space on the disk this node actually sits on, what the
pledge earns, and which limit binds. It changes nothing without `--apply`, and
it refuses to apply numbers it has just said do not fit. Then run it again with
`--apply`: **a new node pledges nothing, and a node that pledges nothing stores
nothing for anybody -- your own other machines included.** Until then a `sync`
towards it says `refused ... its pledge is full or zero`.

```sh
itsanas space --pledge 100G --keep 30G --apply
```

**This bargain is enforced by your own client and, since 2026-10-04, by each
host:** a host stores for your account at most three sevenths of what your
devices' claims pledge. The pledge is still your account's own word, so a
modified client can claim space it does not have; each host lends on such
promises only a shared 30 % of its own pledge. Fine among your own machines,
not yet among strangers. See `docs/ECONOMICS.md` §1. **Every machine must run
the same version:** a host now refuses to store for a device that does not
present its claim.

### 3. Point it at a folder, and run it

```sh
itsanas folder ~/Sync
itsanas daemon
```

The daemon watches the folder, syncs with peers, answers storage challenges and
serves what it hosts. On Linux the installer already wrote a systemd unit:
`systemctl --user enable --now itsanas`.

While it runs it holds the node: `status` still answers, but commands that
change the node (`space --apply`, `pledge`, `folder`, `peer add`, `ls`, `put`)
say the store is open in another process. Stop it first -- `systemctl --user
stop itsanas`, or on Windows `Stop-ScheduledTask ITSaNAS` -- and start it again
after.

### 4. Add a second machine

Install there, then restore the **same** account from the twenty-four words:

```sh
itsanas login --username you --phrase-file words.txt
itsanas folder ~/Sync
itsanas peer add 192.168.1.42:9797
itsanas sync
```

`words.txt` holds the twenty-four words: on one line, one per line, or the
numbered grid `init` printed, pasted as it is. `login` then asks for a
passphrase for *this* machine's keystore; it need not be the first machine's
(`ITSANAS_PASSPHRASE` supplies it to scripts). For `peer add`, use the first
machine's LAN address and the port it listens on -- 9797, or the next free one
if another node took it: the `listen` line of `itsanas status` there shows it,
daemon running or not. `sync` writes what it received into the folder. For it to keep happening
without you, run `itsanas daemon` on **both** machines (or enable their
services): a machine with no daemon never dials anybody, and the other side
only dials the peers it knows -- give each the other's address with `peer add`
when they do not find each other on the network.

An account has **at most 5 live devices** at once. A sixth is refused when it
registers, with the full ids of the five; withdraw one that is gone with
`itsanas device forget <id>`, from any of your machines -- the refused one
included -- which frees its slot.

**A second account on the same computer** -- somebody else in the house, or a
separate account of yours -- is a named instance, with its own home, port and
passphrase: `itsanas --instance sam init --username sam`, then every command
with `--instance sam` (or `ITSANAS_INSTANCE=sam` once in the shell).
`itsanas instances` lists every node on the machine. The installers set one up
with `provision.sh --instance sam` / `provision.ps1 -Instance sam`; see
`install/README.md`.

Both machines now hold the same account. Drop a file in the folder on one; it
appears on the other.

For a machine that cannot hold everything — a phone, a laptop you do not want to
give the whole account to:

```sh
itsanas keep 2G --order newest
```

Everything is still listed; what is not held says so, and asking for one fetches
it.

### 4b. A machine somewhere else

Everything above assumes one network, where machines find each other by
themselves. Across two houses, one side of each pair has to be dialable, and
nothing can guess which:

```sh
itsanas announce ngas.example:9801    # what reaches THIS machine from outside
itsanas register                      # republish it to the coordinator
```

That is the outside name and the **outside** port -- a router forward maps one
port to another, and this is the one somebody else dials. A global IPv6 address
works the same way and needs no forward: `itsanas announce [2001:db8::1]:9797`.

Nothing here can check that the address reaches this machine. That is a forward
or a firewall rule on your router, and a wrong one makes this node quietly
unreachable rather than noisy -- `itsanas status` shows what is being published,
which is the first thing to look at.

A laptop or a phone wants none of this: it has no address another network can
dial, it takes part by dialling out, and one reachable side per pair is enough.

### 5. Check on it

```sh
itsanas status
itsanas doctor
```

`doctor` checks the stored data and then the network, in the order that makes
the next line worth reading: whether this machine can reach the coordinator,
whether anything can reach *it* -- which only somebody outside can answer, so it
asks the coordinator to try -- and whether the addresses it was given are ones it
could dial from where it is standing. If nothing is syncing, that is the command
that says whose problem it is.

While the daemon runs:

```sh
itsanas pause        # stop syncing here; this machine keeps hosting for the others
itsanas resume       # start again, and sync at once
itsanas sync-now     # don't wait for the next round
itsanas interval 10m # how often it syncs: 30s to 1d, or auto
```

Nothing is lost while paused: what you change waits until you resume.
`itsanas pause --for 2h` (1 min to 30 days) pauses with an end, and syncing
resumes by itself; `itsanas status` then says "PAUSED until <time> (in N
min)".

```sh
itsanas signout      # stop ITSaNAS here and forget the passphrase; keys and files stay
itsanas signin       # ask the passphrase again and start it
itsanas settings     # the same choices in a page in your browser
```

While signed out this machine does not answer the others' checks.

An ITSaNAS icon starts at login: in the notification area on Windows, in the
menu bar on a Mac, and as an indicator on a Linux desktop (it needs
python3-gi and AppIndicator; without them, use `itsanas settings`). Its menu
has the same choices: open the folder, pause for 1 hour, 8 hours or until you
resume, sync now, sync every, Settings..., Sign out..., open the log, restart.
The Mac and Linux icons have not yet been run on a real desktop.

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
- **Not proven at a terabyte.** A quiet round costs 8 KB, a change lists about
  its own chunks, and since 2026-10-04 the ledger walk every three and a half
  days lists only the buckets where the peer's summary differs, re-stamping the
  rest without the wire. By arithmetic an idle terabyte is now inside the
  100 MB a day budget; nothing has been measured at that size, and each machine
  now reads a slice of its disk every round to keep its summary honest. The
  arithmetic is in `docs/DESIGN.md` §6.5 rather than in a reassurance.
- **Not yet safe among strangers.** Hosts now hold each account to 30/70 of
  the pledge its devices claim, but the claim is the account's own word: a
  rebuilt client can still claim space it does not have, inside the 30 % of
  each host lent on promises.
  `docs/ROADMAP.md` lists what an adversarial sweep found and did not fix.
- **Not a backup.** Two copies on machines you know is not an archive, and
  nothing here is off-site by default.
