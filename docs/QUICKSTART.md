# Quickstart

Getting two machines syncing. Every command below has been run; the output is
copied from a real session rather than written from memory.

> **Before you start.** Nothing here should hold data you care about yet.
> Nothing carries out a repair plan, no host is challenged on a schedule, and
> none of this has been run on four real machines.
> [ROADMAP.md](ROADMAP.md) is honest about all of it.

## Build

```bash
cargo build --release -p itsanas-cli
```

The binary lands at `target/release/itsanas`. Build in release: `init` and every
subsequent command run Argon2id at production cost, which is about half a second
optimised and unpleasantly slow otherwise.

## 1. Create an account on your first machine

```bash
itsanas --home ~/.itsanas init --username nicolas
```

You are asked for a passphrase twice, then shown 24 words:

```text
Account created.
  username : nicolas
  user id  : 486b33f0c6d44cd8cb9beca41b8d37405966c541c297adad17f79d2e1b2d1584
  device   : 92e6c4dba44784899518ddb33804ec0504379db5f53957cc34dfb54b3806d165

┌─ RECOVERY PHRASE ─────────────────────────────────────────────┐
│ Write these 24 words down, on paper, and keep them somewhere  │
│ your house burning down would not reach.                      │
...
 1. ██████       2. ██████       3. ██████       4. ██████
 5. ██████       6. ██████       7. ██████       8. ██████
...
```

(The words are redacted here on purpose. This output came from a real run, and
printing even part of a real phrase in documentation is exactly the habit this
project is trying not to have — see [TEST-USERS.md](TEST-USERS.md) for the only
keys that are ever published deliberately, and the ban list that keeps them out
of production.)

**Write them down now.** They are shown once and are not stored anywhere on the
machine — a phrase kept on the disk it protects is not a backup. There is a test
(`the_phrase_is_not_written_anywhere_under_the_node_directory`) that fails if
that ever stops being true.

The passphrase and the phrase do different jobs. The passphrase unlocks *this
machine's* copy of your keys and can be changed. The phrase **is** your identity:
anyone holding it can read everything you store, and losing both it and your
passphrase means every byte is gone.

## 2. Point it at a folder

```bash
itsanas folder ~/ITSaNAS
```

```text
synced folder set to /home/nicolas/ITSaNAS
the folder and the store already agree.
```

From here on, that directory is the product. Files you put in it are uploaded,
files you delete from it are deleted on every machine, and changes from your
other devices appear in it.

If you point this at a directory that already has files in it, the first pass
imports them and says how many — nothing is deleted, and nothing is overwritten.

## 2b. Or drive it by hand

```bash
itsanas put notes/hello.txt ./hello.txt
itsanas put archive/big.bin ./big.bin
itsanas ls
```

```text
stored 43 B as notes/hello.txt (1 chunks)
stored 2.0 MiB as archive/big.bin (27 chunks)
     2.0 MiB     27 chunks  archive/big.bin
        43 B      1 chunks  notes/hello.txt
```

## 3. Offer some space, and serve

```bash
itsanas pledge 10G
itsanas serve
```

```text
serving on 0.0.0.0:9797
```

Every connection is TLS 1.3, and both ends prove which device they are by
signing the session's exporter value with their device key. Listening on a real
network is a normal thing to do; there is no override to pass and no warning to
read.

> **One process at a time.** While `serve` is running, other commands against
> the same `--home` will refuse to start:
>
> ```text
> itsanas: store: …/index.redb is already open in another process.
> Only one process at a time may hold a node's state — most likely
> `itsanas serve` is running. Stop it and try again.
> ```
>
> Use `itsanas daemon`, which does both. A local control socket would remove
> the restriction entirely and is not built.

## 4. Bring up a second machine

On the Pi, with the same 24 words:

```bash
itsanas login --username nicolas
```

You are prompted for the phrase, then for a passphrase for *this* machine — it
does not have to match the first machine's.

```text
Account restored.
  user id : 486b33f0c6d44cd8cb9beca41b8d37405966c541c297adad17f79d2e1b2d1584
  device  : 3ca674652bf69bd48c61d759b394918eb4fa8f387769d777cd09c9c67f2d25a5 (new for this machine)
```

Same user id, different device id. That is the point: your identity survives
losing every machine you own, and each machine gets its own revocable key.

## 5. Sync

```bash
itsanas sync 192.168.1.20:9797
itsanas ls
```

```text
192.168.1.20:9797: sent 0 B in 0 chunks, 0 segments; received 2 files, 0 conflicts
     2.0 MiB     27 chunks  archive/big.bin
        43 B      1 chunks  notes/hello.txt
```

Read one back and check it:

```bash
itsanas get archive/big.bin ./recovered.bin
sha256sum ./recovered.bin ./big.bin   # identical
```

To avoid typing the address every time:

```bash
itsanas peer add 192.168.1.20:9797
itsanas sync              # uses every configured peer
```

## 5b. Reaching a machine somewhere else, and recovering from nothing

Everything above works on one network with no server. Two things need one: a
machine on a *different* network, and restoring an account from a passphrase
instead of 24 words.

On an always-on machine with a public address — a VPS, or a VM on a Freebox
Delta:

```bash
itsanas-coordinator --state /var/lib/itsanas-coordinator --listen 0.0.0.0:9898
```

It prints its device id at start-up. Pin it, or an address that resolves
somewhere else is trusted:

```bash
itsanas coordinator coord.example.net:9898 --device <the id it printed>
itsanas register
```

`register` claims the username, enrols this device, and publishes its address.
The daemon then republishes and asks for the account's other devices each round.

To make passphrase recovery possible, lodge a container:

```bash
itsanas register --recovery
```

Then a brand-new machine needs neither the 24 words nor an address:

```bash
itsanas login --username nicolas --from coord.example.net:9898
```

```text
Account restored.
  user id : 7d572490c91f20dd12798a1edf2625be048a467ff41ca51570867c91bdec436e
  device  : a863c43c0f6985781768f0dc2ec61b1cfda5c28928a6877deb47d009f43484c7 (new for this machine)
```

Same identity, new device key. **The trade, plainly:** anybody who steals the
coordinator's database can attack that passphrase offline. Five attempts per
account per quarter-hour is what stands in their way, plus the Argon2id cost.
It is off until you ask for it, and `itsanas register --withdraw-recovery`
takes it back — after which only the 24 words work.

The coordinator holds no keys, no chunks and no plaintext, and it can be
switched off without stopping anything already known. That is deliberate;
[DESIGN.md](DESIGN.md) §8 works through why it holds so little.

## 6. Check on it

```bash
itsanas status
itsanas doctor --deep     # reassembles and re-hashes every file
itsanas gc                # reclaims deleted and overwritten chunks
```

`status` separates two things a node holds that are easy to confuse:

```text
hosting for other people
  pledged         10.0 GiB
  used            0 B
  peers hosted    0

relaying for your own devices
  segments held   2 (so this machine can pass your other devices' work along)
```

## Running it unattended

```bash
itsanas peer add 192.168.1.20:9797
itsanas daemon --interval 300
```

`daemon` serves peers *and* syncs on a timer, in one process:

```text
itsanas daemon
  serving   0.0.0.0:9797
  pledged   10.0 GiB
  interval  300s
  peers     192.168.1.20:9797

Ctrl-C to stop.

192.168.1.20:9797: sent 0 B (0 chunks, 0 segments), received 1 files, 0 conflicts
```

Both halves in one process is not tidiness — it is the only arrangement that
works. The index is held under an exclusive lock, so `serve` and `sync` cannot
run at the same time against the same node, and two cron entries would fight
over it. It also means the passphrase is entered once instead of paying a full
Argon2id derivation on every scheduled sync.

A quiet round prints nothing, so a journal only shows real activity. An
unreachable peer is logged and the loop continues — machines being off is the
normal state of this network, not a fault.

For a service manager, the passphrase comes from `ITSANAS_PASSPHRASE` when there
is no terminal to prompt at. Anything able to read the process's environment can
then read it, so this is a trade you are making, not a default.

```ini
# /etc/systemd/system/itsanas.service
[Service]
Environment=ITSANAS_PASSPHRASE=…
ExecStart=/usr/local/bin/itsanas --home /var/lib/itsanas daemon
Restart=always
```

The daemon watches the folder, so a saved file starts syncing in under a second
rather than waiting for the interval. The watcher is never trusted alone — every
platform's drops events under load and none report changes made while the
process was stopped — so a full rescan runs on the interval regardless, and an
hourly deep rescan re-hashes everything.

**One process at a time still applies.** While the daemon runs, other commands
against the same home refuse to start. Stop it first.

## What a working setup looks like

Two machines, each with a folder, one pointed at the other:

```bash
# on the laptop
itsanas folder ~/ITSaNAS
itsanas pledge 10G
itsanas daemon                                        # listens on 0.0.0.0:9797

# on the Pi
itsanas folder /srv/itsanas
itsanas pledge 500G
itsanas peer add laptop.lan:9797
itsanas daemon
```

Then drop a file in `~/ITSaNAS` on the laptop and watch it appear in
`/srv/itsanas` on the Pi. Delete it, and it goes from both. Only one side needs
the other configured as a peer — a node that only accepts connections still
applies whatever was pushed to it.

## Three machines

The design does not require any two machines to be online together. If the Pi
pushes to a host and switches off, a VM that has never spoken to the Pi still
gets the Pi's work from that host — the host relays segments it cannot read.
That is tested end to end in
`a_host_relays_one_device_to_another_that_it_never_met`.

## When something goes wrong

| Symptom | What it means |
| --- | --- |
| `already open in another process` | `itsanas serve` is running against the same home. Stop it. |
| `wrong passphrase, or the keystore has been tampered with` | Exactly that. The two are indistinguishable on purpose. |
| `no node found at …` | Run `init` for a new account or `login` to restore one. |
| `expected to reach device X but Y answered` | The address resolved to the wrong machine. Refused on purpose — the coordinator is not trusted to say who lives at an address. |
| `sync` reports `deferred` | A peer had the log but not yet the chunks. Sync again once the device holding them is up. |
| `doctor` reports missing chunks | Those files cannot be read until the chunks are refetched from a peer. |
