# Quickstart

Getting two machines syncing. Every command below has been run; the output is
copied from a real session rather than written from memory.

> **Before you start.** Nothing here should hold data you care about yet. The
> transport is unencrypted (it protects your *data*, not your *metadata* — see
> [SECURITY.md](../SECURITY.md)), there is no daemon so nothing syncs on its
> own, and there is no placement layer so nothing decides which hosts should
> hold what. [ROADMAP.md](ROADMAP.md) is honest about all of it.

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

## 2. Put something in

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
serving on 127.0.0.1:9797
```

`serve` refuses a non-loopback address unless you pass `--allow-public`. That is
deliberate: this transport is unencrypted, and while your data stays sealed, an
observer on the path sees chunk identifiers and sizes. Use a VPN or an SSH
tunnel between machines until QUIC lands.

> **One process at a time.** While `serve` is running, other commands against
> the same `--home` will refuse to start:
>
> ```text
> itsanas: store: …/index.redb is already open in another process.
> Only one process at a time may hold a node's state — most likely
> `itsanas serve` is running. Stop it and try again.
> ```
>
> This is what the daemon in M7 exists to fix. Until then, stop the server when
> you want to run something else.

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
  serving   127.0.0.1:9797
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

**Not yet the folder-that-just-syncs experience.** The daemon does not watch the
filesystem; files enter through `itsanas put`. And while it runs, other commands
against the same home still refuse to start — stop it first.

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
| `refusing to bind …: this transport is unencrypted` | Use loopback, or pass `--allow-public` having read [SECURITY.md](../SECURITY.md). |
| `sync` reports `deferred` | A peer had the log but not yet the chunks. Sync again once the device holding them is up. |
| `doctor` reports missing chunks | Those files cannot be read until the chunks are refetched from a peer. |
