# ITSaNAS — Global Architecture

> Companion documents: [ROADMAP.md](ROADMAP.md) (what is built vs planned),
> [DESIGN.md](DESIGN.md) (why each mechanism works the way it does),
> [ECONOMICS.md](ECONOMICS.md) (what a member gives and gets),
> [TESTING.md](TESTING.md) (every automated test and what it proves),
> [TEST-USERS.md](TEST-USERS.md) (the published fixture identities).

## 1. What the system is

ITSaNAS is a mutual-storage network. Each member pledges disk space and
bandwidth; in return their files are replicated onto other members' machines.
The defining constraint is that **hosting is blind**: the machine storing your
data has no ability to read it, and no ability to learn anything useful about
it.

Three properties have to hold at once, and each one constrains the design:

| Requirement | What it forces |
| --- | --- |
| A host cannot read what it stores | All encryption is client-side; addresses are blinded; a host never receives a key |
| Devices are frequently offline | Writes cannot require a quorum of the owner's devices; state must be replayable from third parties |
| A user can recover on a fresh machine | Identity must derive from something memorable/portable, not from device state |

## 2. Layer map

```
┌───────────────────────────────────────────────────────────────────────┐
│  itsanas (CLI, and `itsanas daemon` as the background service)      │
│  init / login / pledge  file watcher, sync loop, repair loop, alerts  │
├───────────────────────────────────────────────────────────────────────┤
│  itsanas-sync                      itsanas-placement                  │
│  version vectors, op-log merge,    rendezvous hashing, replica        │
│  conflict materialisation          targets, repair, quota accounting  │
├───────────────────────────────────────────────────────────────────────┤
│  itsanas-store   itsanas-folder    itsanas-net                        │
│  FastCDC chunking, blob store,     peer protocol, sessions,           │
│  op-log, index, vault, a synced    proof-of-storage challenges        │
│  directory with a watcher          ├─ itsanas-tls  (device auth)      │
│                                    └─ itsanas-wire (framing)          │
├───────────────────────────────────────────────────────────────────────┤
│  itsanas-discover                  itsanas-crypto                     │
│  signed UDP announcements,         identity, key schedule, sealing,   │
│  no server involved                blinded addressing, keystore       │
└───────────────────────────────────────────────────────────────────────┘
                                    │
                    ┌───────────────┴────────────────┐
                    │  itsanas-coord  (optional)     │
                    │  control plane only, untrusted │
                    └────────────────────────────────┘
```

Every arrow points downward: no lower layer knows about a higher one.
`itsanas-crypto` depends on nothing in this project, which is what makes it
auditable in isolation.

## 3. Data model

### 3.1 Chunks

A file is split by content-defined chunking (FastCDC; 16 KiB minimum, 64 KiB
average, 256 KiB maximum). Splitting on content rather than offset means
inserting a byte at the start of a large file re-chunks only the neighbourhood
of the edit, not the whole file.

Each chunk is sealed independently:

```
plaintext chunk
   │
   ├─ chunk_id  = BLAKE3_keyed(user.blinding_key, BLAKE3(plaintext))
   │              deterministic for the owner → deduplication works
   │              opaque to everyone else     → host learns nothing
   │
   └─ ciphertext = XChaCha20-Poly1305(
                     key   = BLAKE3_XOF(user.chunk_root, context),
                     nonce = BLAKE3_XOF(user.chunk_root, context ‖ "/nonce"),
                     aad   = version ‖ purpose ‖ owner_id ‖ chunk_id)
```

Sealing is **deterministic**, which buys two things beyond deduplication: an
owner can re-derive exactly the bytes a host should be holding (making audits
possible without keeping a second copy of the ciphertext), and re-uploading
unchanged data is a no-op.

### 3.2 The operation log

This is the mechanism that makes live sync work across offline devices.

Each device keeps an append-only log. Entries are:

```
Put    { path, size, mtime, mode, chunk_ids[], version_vector }
Delete { path, version_vector }
Rename { from, to, version_vector }
```

Entries batch into **segments**: small, sealed, signed objects that replicate to
blind hosts exactly like chunks. A host can order segments by their signed
`(device_id, sequence)` without decrypting a single field.

Each device publishes a signed `Head { user, device, sequence, segment_id,
lamport }` record. Peers discover new work by comparing heads.

### 3.3 Why the log makes offline devices work

```
t0   Pi writes report.pdf  ─────► seals chunks + log segment
t1   Pi pushes both to Alice's laptop and Carol's VM (blind hosts)
t2   Pi powers off
t3   Alice's laptop wakes, sees Pi's head advanced, pulls the segment
     from Carol's VM, replays it, fetches chunks, materialises the file
```

At no point did the Pi need to be online at the same time as the laptop. The
blind hosts acted as a store-and-forward relay for data they could not read.

### 3.4 Conflicts

Every entry carries a version vector (`device_id → counter`). Two entries on the
same path are concurrent when neither vector dominates the other. Concurrent
edits are **materialised side by side**:

```
report.pdf
report.conflict-a3f21c8d0e91-7.pdf
```

Nothing is silently overwritten and nothing is discarded. Deletes are
tombstones, garbage-collected only after a retention window, so a delete that
raced with an edit cannot destroy the edit.

## 4. Placement and durability

### 4.1 Where a chunk goes

Placement uses **rendezvous (highest-random-weight) hashing** over the signed
node set, weighted by pledged capacity:

```
slots(node)        = clamp(pledge / smallest pledge in the swarm, 1, 64)
score(node, chunk) = max over its slots of BLAKE3(node ‖ slot ‖ owner ‖ chunk)
replicas           = top R nodes by score
```

**No floating point, deliberately.** The textbook weighted formula is
`weight / -ln(uniform_hash(…))`, and `f64::ln` is the platform's libm: two
machines can differ in the last unit in the last place, and when two candidates
land that close the Pi and the laptop disagree about where a chunk lives —
silently, permanently, with no error. Taking the maximum over integer slots
gives the same proportionality (the chance of holding the swarm's highest hash
is exactly a node's share of the slots) computed identically everywhere.

Properties that matter here:

- **Deterministic.** Any node computes the same placement from the same node
  set, so no agreement protocol is needed for "where does this live".
- **Minimal churn.** Adding or removing a node moves only the chunks that node
  should own, not a reshuffle of the whole keyspace.
- **Owner affinity.** A user's own devices are always preferred replicas, so
  reading your own data never depends on anyone else being online.

**What placement does not do yet.** It weights by pledged capacity and nothing
else. It has no notion of an *anchor* — a node that is actually up — so nothing
guarantees a replica lands somewhere reachable while your own machines sleep.
Durability and availability are separate purchases and only the first is
implemented; [ECONOMICS.md](ECONOMICS.md) §2 has the arithmetic and the gap.

### 4.2 Redundancy

v1 uses plain replication, default factor **3**, minimum 2. The shard interface
is deliberately shaped for Reed–Solomon so that erasure coding can replace
replication once a swarm has enough independent nodes — RS(4,2) gives the same
two-failure tolerance at 1.5× overhead instead of 3×.

### 4.3 Repair

A background loop tracks live replica counts. A chunk below target replication —
because a node left, or has been unreachable past a threshold — is re-pushed to
the next-best nodes by rendezvous score.

> **Planning only.** `placement::repair::plan` computes what should be pushed
> where, deterministically, and is tested. Nothing calls it: building the census
> it needs means querying every peer about what it holds, which needs the node
> set the coordinator would publish. The daemon runs no repair loop.

### 4.4 Proof of storage

Hosts are challenged periodically:

```
verifier → host:  chunk_id, random nonce
host     → verifier: BLAKE3_keyed(nonce, ciphertext)
```

The verifier re-derives the expected ciphertext locally (deterministic sealing
makes this possible) and compares. A host that discarded the data, or corrupted
it, cannot answer.

> **The challenge works; nothing schedules it and nothing records the answer.**
> It can be issued over the wire today and there is a test for it. The intended
> consequence — a failed challenge marking a host unreliable, so pledged
> capacity that is not actually provided stops earning storage — needs a
> reputation store that does not exist. Until it does, a host that quietly
> discards data is caught only by accident.

## 5. The coordinator

Optional, and deliberately minimal. Intended to run on a small OVH VPS or an ARM
VM on a Freebox Delta.

> **Machines on one network need no coordinator at all.** `itsanas-discover`
> finds them with signed UDP announcements; see §6.1. What follows is only about
> reaching a machine on a *different* network.
>
> **No coordinator server exists.** `itsanas-coord` is a tested library — claims,
> revocation, presence, measured availability, accounting, account directory,
> escrow storage — with nothing serving it and no client speaking to it. The
> list below is what it is *for*; [ROADMAP.md](ROADMAP.md) M6 is the status.
> Until it is built, peers are configured by hand and there is no node set, so
> nothing in §4 that depends on one runs either.

**What it will do**

1. Account directory: `username → user public key`. Prevents name squatting and
   impersonation.
2. Presence: which device keys belong to which user, pledged capacity, last
   seen, reachable addresses.
3. Publishes the signed **node-set epoch** so every peer computes identical
   placement.
4. Stores each user's opaque, passphrase-encrypted **escrow blob** for
   new-device login.
5. May relay for peers that cannot be dialled directly. **Not built** — a node
   behind NAT can already push, and `session::drain_vault` means the node it
   pushed to still applies what arrived.

**What it explicitly cannot do**

- Read plaintext — it never receives a key or a decryptable object.
- Forge a user's data — every log segment and head is signed by the user's
  device key, which the coordinator does not hold.
- Read the escrow blob — it is Argon2id-sealed under a passphrase the
  coordinator never sees.

**What a compromised coordinator can do:** deny service, lie about who is
online, and attempt to partition the swarm. Mitigations: peers pin the node set
they last saw and treat unexplained shrinkage as an alert condition rather than
as truth.

The whole surface is control-plane, which is what keeps it replaceable by a DHT
later without touching a single byte of the data plane.

## 6. Transport

### 6.1 Finding a peer

On one network, nothing central is involved. A node broadcasts a fixed 147-byte
announcement on UDP 21037, signed by its device key. Since a `DeviceId` *is* the
Ed25519 verifying key, any receiver verifies it with no prior contact, so nobody
can advertise a device they do not hold.

The address is deliberately **not** in the packet — only the port is, and the
address comes from the UDP source, so a node cannot advertise a machine other
than itself. What is discovered is then dialled with the announcing device
pinned, so an address answering as somebody else is refused: discovery says who
*might* be worth talking to and never says who somebody *is*.

Across networks, a coordinator supplies addresses. That is the only discovery job
left for it, and [DESIGN.md](DESIGN.md) §8 explains why it is not worth replacing
with a DHT at this size.

### 6.2 The connection

TLS 1.3 over TCP, with mutual authentication by device key and no certificate
authority.

Authentication sits *above* TLS rather than inside the certificate. Certificates
are anonymous and regenerated every start-up; each side proves itself by signing
the TLS session's **exporter value** with its device key. A man in the middle who
terminates TLS has two sessions with two different exporters, so a proof from one
is worthless in the other and they cannot produce their own. Two consequences:
no X.509 parsing anywhere in the trusted path, and an observer cannot correlate
two connections by their certificates.

Dialling pins the expected device id wherever one is known, so an address that
resolves to the wrong machine is refused rather than trusted — the coordinator
hands out addresses and is not trusted to say who lives at one.

**NAT traversal is not built.** A node behind NAT can push but cannot be dialled.
That is less limiting than it sounds: `session::drain_vault` means a node that
only ever accepts connections still learns what was pushed to it, so work flows
in both directions as long as one side can dial. Hole punching and relay
fallback would want QUIC, which is now an optimisation rather than a
prerequisite for security.

## 7. Operational behaviour

The daemon is meant to be invisible: a folder that syncs. Invisible systems fail
silently, so the daemon should raise explicit alerts on the conditions below.

> **No alerting exists.** The daemon logs sync rounds and unreachable peers and
> nothing else; `itsanas status` and `itsanas doctor` are the only ways to learn
> the node's condition, and both have to be run by hand. Most of these
> conditions cannot even be evaluated yet, because they need the node set. This
> table is the specification.

Conditions that must eventually alert:

| Condition | Why it matters |
| --- | --- |
| Online node count below the replication floor | New writes cannot reach their durability target |
| Any chunk below minimum replicas past a grace period | Data is one failure from loss |
| No successful sync round in *N* minutes | Sync is broken, not merely quiet |
| A proof-of-storage challenge fails | A host is not holding what it claims |
| Local pledged space exhausted or unwritable | Node can no longer meet its side of the bargain |
| Local free space below reserve | Host is about to start failing writes |

## 8. What is deliberately not protected

Being explicit about the limits is part of the design:

- **Object sizes and counts** are visible to a host. Hiding them needs padding
  and cover traffic; that is a deliberate non-goal for now.
- **Access timing** is visible. A host can see when you read or write, though
  not what.
- **Availability** is not guaranteed against a fully offline swarm. Your own
  devices always hold your own data, so you are never locked out of your files,
  but replication targets need peers.
- **A published test identity is not protected at all** — by design. See
  [TEST-USERS.md](TEST-USERS.md).
