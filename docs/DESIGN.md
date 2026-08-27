# Design Notes

Why each mechanism is the way it is, and what was rejected. Structural overview
lives in [ARCHITECTURE.md](ARCHITECTURE.md); this document is the reasoning. The
one set of decisions kept out of here is the economic contract — what a member
gives, gets, and loses — which has its own document, [ECONOMICS.md](ECONOMICS.md),
because it is the part a member has to understand before joining.

---

## 1. Identity

### One master secret, everything derived

A user is 32 random bytes. Every other key descends from them through BLAKE3
`derive_key` under a hardcoded, versioned context string:

```
master secret (32 bytes)
  ├── "itsanas v1 user master signing key"     → Ed25519 → user id
  ├── "itsanas v1 user master agreement key"   → X25519
  ├── "itsanas v1 user chunk data key"         → chunk sealing root
  ├── "itsanas v1 user chunk id blinding key"  → address blinding
  └── "itsanas v1 user oplog object key"       → log segment sealing root
```

**Why derive rather than generate independently:** recovery. A user who loses
every device must reconstruct their complete key material from one portable
artefact. Independent random keys would need a key *backup*; derived keys need
only the seed.

**Why versioned contexts:** a future v2 schedule must derive entirely different
keys rather than silently reinterpreting v1 material. Enforced by
`kdf::tests::context_strings_carry_a_version`.

### Device keys sit outside the tree

Device keys are generated locally and certified by the master key, deliberately
*not* derived from it.

**Why:** revocation granularity. If a device key were derived from the master
secret, a stolen laptop would compromise a key the user cannot change without
rotating their whole identity — new user id, re-encrypt everything, re-register.
With independent device keys, revoking a laptop is dropping one certificate.

### Recovery phrase: 24 words, not 12

24 words carries 256 bits of entropy, matching the master secret exactly. No
stretching, no truncation. A 12-word phrase is explicitly rejected — see
`identity::tests::short_phrases_are_rejected` — because silently accepting one
would halve key strength without the user noticing.

### Recovery must be verified, not assumed

A property test surfaced a limitation worth writing down.

BIP-39 gives a 24-word phrase only an **8-bit checksum**. Roughly one word
transposition in 256 therefore passes the checksum and decodes cleanly to a
*completely different* valid master secret. This is inherent to BIP-39 and
cannot be fixed in the crypto layer.

The failure mode is nasty precisely because it is quiet: a user restores with a
typo, lands on a valid empty account, and concludes their data is gone.

**Required mitigation at the CLI layer**, tracked as an M7 exit criterion:

1. After accepting a recovery phrase, display the derived user id.
2. Look up the username in the account directory and compare.
3. On mismatch, refuse to proceed and say the phrase is wrong — never silently
   create or open a different account.

Documented by
`a_mistyped_recovery_phrase_never_reconstructs_the_original_identity`.

---

## 2. Sealing

### Two modes, and why both are needed

**Deterministic** (chunks): key and nonce both derived from the chunk id, which
is itself a hash of the plaintext. Re-sealing identical content yields identical
bytes.

Safe despite the fixed nonce because the key is unique per plaintext: one key is
never used for two different messages, which is the actual requirement. Two
things fall out of it:

- **Deduplication.** Identical content stored twice occupies one chunk.
- **Remote audit without a second copy.** An owner can re-derive exactly the
  bytes a host should be holding, and challenge it, without keeping the
  ciphertext locally. Non-deterministic sealing would force the owner to store
  every chunk twice — once as plaintext, once as ciphertext — to audit anything.

**Randomised** (log segments, manifests): fresh 24-byte nonce each time, since
these are new objects rather than content-addressed ones.

### Everything is bound into the associated data

```
aad = version ‖ len(purpose) ‖ purpose ‖ owner_id ‖ len(address) ‖ address
```

Each field closes a substitution attack:

| Field | Attack it prevents |
| --- | --- |
| `version` | Rolling a v2 object back to v1 parsing |
| `purpose` | Serving a chunk where a log segment is expected |
| `owner_id` | Attributing an object to the wrong user |
| `address` | Serving chunk A's bytes when asked for chunk B — stale or swapped content |

Every variable-length field is length-prefixed. Without it, `("ab", "c")` and
`("a", "bc")` encode identically and the binding can be bypassed by shifting a
field boundary. Checked by `seal::tests::associated_data_encoding_is_unambiguous`.

### XChaCha20-Poly1305, not AES-GCM

- The Raspberry Pi 4B+ has no AES hardware acceleration; ChaCha20 is fast in
  software on ARM, AES is not.
- The 192-bit extended nonce removes any nonce-collision anxiety in the
  randomised mode.
- AES-GCM fails catastrophically on nonce reuse and has awkward length limits.

---

## 3. Blinded addressing

```
chunk_id = BLAKE3_keyed(user.blinding_key, BLAKE3(plaintext))
```

The naive choice — addressing chunks by their plaintext hash — enables global
deduplication and is used by several backup tools. It is wrong here.

**What it would leak:** a host holding a candidate file could hash it and check
whether that address exists in its store, confirming a specific user holds a
specific file. It would also reveal that two users hold identical content.

Blinding with a per-user secret keeps deduplication *within* a user, which is
where most of the benefit is anyway, while making the address meaningless to
anyone else.

**The cost, stated honestly:** no cross-user deduplication. Three users storing
the same 1 GiB film consume 3 GiB. That is the price of the confidentiality
property, and it is the right trade for this system.

Verified on real corpus data by
`the_shared_document_gets_a_different_address_for_every_user`, using a file all
three fixture users hold byte-identically.

---

## 4. Live sync across offline devices

### The problem

Syncthing-style live sync assumes peers can read the data they relay. Here they
cannot. And the devices that *can* read it — the user's own machines — are
exactly the ones that are frequently offline.

### Rejected: direct device-to-device sync only

Simplest design, and it fails the core requirement. If Alice's laptop and her Pi
are never online simultaneously, they never converge. For a laptop that sleeps
and a Pi that reboots, "never simultaneously" is the normal case.

### Rejected: a trusted always-on relay

Would work, but reintroduces a machine that must be trusted with ordering and
availability, and becomes a single point of failure. The whole point is to not
need one.

### Chosen: encrypted operation log replicated to blind hosts

Each device appends to its own log; entries batch into sealed, signed segments;
segments replicate to blind hosts exactly like chunks. Hosts order them by
signed `(device_id, sequence)` without reading a field.

```
t0   Pi writes report.pdf  ─────► seals chunks + log segment
t1   Pi pushes both to Alice's laptop and Carol's VM
t2   Pi powers off
t3   Laptop wakes, sees Pi's head advanced, pulls the segment from Carol's VM,
     replays it, fetches chunks, materialises the file
```

The blind hosts acted as a store-and-forward relay for data they cannot read.
The Pi never had to be online at the same time as the laptop.

### Conflicts: materialise, never resolve

Version vectors (`device_id → counter`) detect concurrency. Concurrent edits
produce siblings:

```
report.pdf
report.conflict-a3f21c8d0e91-7.pdf
```

The sibling is named after the **device id and sequence number** of the losing
version, not after a timestamp. This changed during implementation, for the same
reason the ordering does not use clocks: every device has to derive the sibling
path identically and independently, and the devices disagree about the time.
Device id plus sequence is already unique, and every device sees the same value.

Which version keeps the original path is decided by a total order on
`(device_id, sequence)` — highest wins. The rule is arbitrary but it must be
*deterministic*: if two devices disagreed about who won, each would write its own
winner to `report.pdf` and they would overwrite each other forever instead of
converging.

**Why not last-writer-wins:** it requires trusting clocks across machines, and
it silently destroys work. A user who loses an afternoon's edits to a clock skew
will not trust the system again. Siblings are ugly and obvious, which is the
correct trade for a storage system.

Deletes are tombstones with version vectors, garbage-collected only after a
retention window, so a delete racing an edit cannot destroy the edit.

---

## 5. Placement

### Rendezvous hashing, not consistent hashing

```
slots(node)        = clamp(pledge / smallest pledge in the swarm, 1, 64)
score(node, chunk) = max over its slots of BLAKE3(node ‖ slot ‖ owner ‖ chunk)
replicas           = top R nodes by score
```

Rendezvous hashing wins here on three counts:

- **No ring state.** Consistent hashing needs an agreed ring with virtual nodes;
  rendezvous needs only the node set, which the coordinator already publishes
  signed. Fewer things to agree on, fewer things to attack.
- **Correct weighting.** Capacity weights are exact, not approximated by virtual
  node counts. A member pledging 4 TB should receive proportionally more than
  one pledging 500 GB.
- **Minimal disruption, provably.** Removing a node moves only that node's
  chunks. Nothing else reshuffles.

### Rejected: the textbook weighted formula

This document originally specified `weight / -ln(uniform_hash(…))`. That is
correct mathematics and a latent data-loss bug. `f64::ln` is the platform's
libm: two machines can differ in the last unit in the last place, and when two
candidates land that close the Pi and the laptop disagree about where a chunk
lives — silently, permanently, with no error raised and no way to notice except
by losing data.

Integer slots give the same proportionality with no floating point anywhere: a
node holds slots in proportion to its pledge, its score is the highest hash
across them, and the chance of holding the swarm's highest hash is exactly its
share of the slots. Slots are capped at 64 so one enormous member cannot become
a single point of concentration.

Determinism across architectures is not a nicety here. It is the property that
lets placement work with no agreement protocol at all.

### Where a replica actually is: recorded, not derived

Rendezvous hashing answers "where *should* this chunk go". It does not answer
"where *is* it", and the original design closed that gap with a
coordinator-published node set that every peer agreed on — which is a consensus
protocol wearing a different hat. See §8.

The answer is that the owner writes it down. A `HOLDERS` table in the store maps
each chunk to the devices known to hold it, keyed `chunk_id || device_id` so that
every holder of one chunk is one contiguous range.

Two decisions inside it are worth stating because both are easy to get wrong:

**It is filled from what a peer says it does *not* need.** A push round already
asks the peer which chunks are missing, in order to decide what to send.
Everything the peer did not ask for, it already has — so that same answer is
recorded. The ledger therefore converges on every round rather than only growing
when this node uploads something, and a device restored from its recovery phrase
learns where its data lives by asking instead of re-uploading its whole store to
find out.

**A replication target counts this device.** A target of three asks for two
remote holders, because the copy on this disk is the third. The alternative — an
implicit `+1` at each call site — is the unwritten rule that produces a repair
loop quietly keeping two copies while reporting three, wrong in a direction
nothing reports until two machines die instead of three. It is pinned by
`a_target_counts_this_device_so_three_asks_for_two_elsewhere`.

**What a record means:** this device sent the chunk, and that device said it
accepted it. Evidence, not proof. A host that accepted and then deleted still
has a record until a storage challenge fails. That is the honest position, and
it is why challenges exist — without a ledger there is nothing to challenge,
because nothing knows who to ask.

### Anchors: decided, not built

Replication buys durability. It does not buy availability, and conflating the
two produces absurd numbers: with a replica online a quarter of the time,
reaching 99 % availability needs `ln(0.01) / ln(0.75)` ≈ **16 replicas**.

The decision is therefore to buy them separately: three replicas for durability —
a switched-off laptop still holds the bytes — plus at least one replica on a
node that is actually up, an *anchor*, which any always-on machine becomes
automatically.

**None of that is implemented.** `NodeSet::replicas_for` takes an owner, a chunk
and a count; it has no availability input and no anchor concept. `is_anchor`
exists only in `coord::accounting`, where it labels a member's standing. Wiring
the rule into placement needs the signed node set a coordinator would publish,
so it is blocked behind M6 — see [ROADMAP.md](ROADMAP.md).

One constraint on how it gets wired, worth fixing now because it is easy to get
backwards: **measured availability may affect entitlement, never placement.**
Placement must stay computable from the signed node set alone, so the decision
that risks data never depends on the untrusted coordinator's opinion of who is
reliable. An anchor rule that reads a coordinator-published availability number
would violate this; one that reads a locally observed challenge history would
not. [ECONOMICS.md](ECONOMICS.md) §3 carries the argument.

### Owner affinity

A user's own devices always rank as preferred replicas. This is what makes
reading your own data independent of anyone else being online, and it means the
system degrades to "a slightly odd local folder" rather than to "unavailable"
when the swarm is quiet.

### Replication now, erasure coding later

Replication factor 3 is 3× overhead; RS(4,2) gives the same two-failure
tolerance at 1.5×. But RS needs at least 6 independent nodes to mean anything,
and the initial swarm is three devices belonging to one person.

So: replication first, with the shard interface shaped for erasure coding from
day one. Trading 1.5× overhead away in exchange for a design that works at n=3
is the right call at n=3.

---

## 6. Proof of storage

```
verifier → host:  chunk_id, random nonce
host     → verifier: BLAKE3_keyed(nonce, ciphertext)
```

The verifier re-derives the expected ciphertext locally — possible only because
sealing is deterministic — and compares. A host that discarded or corrupted the
data cannot answer.

**Why challenge–response rather than trusting reports:** the fair-share model
gives storage in proportion to storage provided. Self-reported capacity is an
invitation to claim 10 TB, store nothing, and collect. The challenge makes the
claim cost something to fake, which is exactly enough.

**Its limit, stated plainly:** this proves the host has the bytes *now*, not
that it kept them continuously, and a host that fetched the chunk from another
replica just in time would pass. That is acceptable — such a host is still
serving the data. Genuine proof-of-retrievability schemes exist and are much
heavier; they are not worth it at this scale.

---

## 7. Transport authentication

### The problem with putting the identity in the certificate

The obvious design is a self-signed certificate whose public key *is* the device
key, and pinning. It works, and it drags X.509 parsing into the trusted path for
no gain: certificates carry names, extensions, validity windows and encodings
that all have to be parsed before the peer is authenticated. Historically that
is where TLS implementations get broken.

It also makes every connection linkable. A static certificate is a stable
identifier visible to anyone on the path, so an observer can tell that the same
device connected twice, from two networks, without breaking anything.

### Chosen: anonymous certificates, authentication bound to the channel

Certificates are self-signed, throwaway, and regenerated on every start-up. They
authenticate nobody. Once the handshake completes, each side signs the session's
**TLS exporter value** (RFC 5705) with its device key and sends the signature
over the established channel:

```
proof = Ed25519_sign(device_key,
                     "itsanas v1 device channel authentication" ‖ exporter)
```

A man in the middle who terminates TLS has two sessions with two different
exporters. A proof from one session is worthless in the other, and it cannot
produce a valid one for its own session without the device key. So the identity
is bound to *this* channel rather than to a credential that can be replayed.

The regression test is `a_proof_from_one_session_is_worthless_in_another`. It is
the single test that would catch this being quietly weakened.

**Consequences:**

- No X.509 validation in the trusted path. The certificate is a key transport
  for the handshake and nothing else.
- An observer cannot correlate two connections by certificate.
- Device revocation is a coordinator concern (`NodeClaim.revoked`), not a
  certificate-expiry concern. Nothing has to be reissued when a device leaves.

### Dialling pins the expected device

`PeerClient::connect` takes an optional expected device id and refuses the
connection if a different device answers. The coordinator hands out addresses;
it is not trusted to say who lives at one. Where the caller does not yet know
which device to expect — a first contact — it passes `None` and gets an
authenticated but unpinned peer, which is exactly as much as is actually known.

### Rejected for now: QUIC

QUIC would bring 0-RTT resumption, no head-of-line blocking, and the machinery
NAT hole punching wants. It was not the right first move: TLS-over-TCP gets the
same confidentiality and the same authentication with a much smaller dependency
surface, and everything above the transport is transport-agnostic and tested, so
porting later is a contained change rather than a rewrite.

---

## 8. Decentralisation, and the coordinator

> **Not built.** This section is the design for a component that does not exist
> as a running service; `itsanas-coord` is the library half. See
> [ROADMAP.md](ROADMAP.md) M6.

### The question, asked properly

"Can this be done with no central component at all?" is the right question and it
has a wrong first answer. The wrong answer is to argue about whether a server
exists. The useful question is **what would break if the server vanished** — and
the honest audit of that found that the design as written had made the
coordinator load-bearing for reasons that do not survive scrutiny.

The coordinator was doing five jobs. Taken one at a time, four of them either do
not need it or should never have been given to it.

#### 1. Finding machines — decentralisable, and partly done without a server

Kademlia-style DHTs have solved address lookup at the scale of millions of nodes
since 2005. The technique is not in question.

What is in question is **the size at which it becomes safer than a server**. A
DHT gets its Sybil resistance from dilution: hostile nodes have to be
outnumbered. At ten nodes there is nothing to dilute them with, and an eclipse
attack — feeding one member a false view of the network — costs an attacker
almost nothing. Sybil resistance in an open DHT with no external cost function is
an open problem, not an implementation detail. A DHT also needs bootstrap nodes,
so it does not remove the server, it renames it.

Two consequences, and the second is counter-intuitive:

- **Local discovery needs no server** — a signed UDP beacon on the local network.
  Machines in one house find each other with nothing configured. See §9.
- **A DHT leaks more metadata than a private coordinator**, because lookups are
  visible to strangers. Self-hosting the notice board is better for privacy than
  publishing queries to the world.

#### 2. Placement — the coordinator should never have had this job

The original design had the coordinator publish a **signed node-set epoch** so
that every peer computed identical rendezvous placement "without an agreement
protocol". That phrasing hid the problem: requiring every peer to hold the same
membership list *is* an agreement protocol. It was consensus by decree.

It is also unnecessary, and noticing why is the useful part. A global content
store — IPFS, a DHT — must answer "who holds this block?" for an arbitrary asker,
and that needs global agreement about the keyspace. **ITSaNAS never asks that
question.** Every chunk belongs to exactly one user, and that user holds an
operation log listing their own chunks. The owner knows what they stored, so the
owner can record *where they put it*, in their own log, which already replicates
to blind hosts.

Placement therefore becomes: the owner picks replicas from peers it knows and can
reach, records the choice, and repairs when the count drops. Rendezvous hashing
survives — it still spreads load proportionally over the peers an owner knows —
but it no longer requires the world to agree.

This is not a concession. Third-party repair was never possible anyway: a host
holds opaque bytes and cannot read an owner's log to learn what is missing.
Repair was always going to be the owner's job. The node-set epoch was solving a
problem the system does not have.

#### 3. Accounting — bilateral beats a global ledger

A global entitlement figure needs a trusted accountant, which is exactly the role
the rest of the design refuses to grant anyone.

The alternative is **bilateral**: two members each track what they hold for the
other. "I store 100 GB for Bob; Bob stores 300 GB for me" is checkable by both
parties, forgeable by neither, and needs no bookkeeper. It is BitTorrent's
tit-for-tat, which has held up on an openly hostile network for twenty years.

Two things fall out of it that the global model had to be told:

- **The 3x contribution ratio appears on its own.** Wanting three replicas of
  100 GB means finding three counterparties and giving each 100 GB back.
- **Availability needs no third-party measurement.** If a peer is never
  reachable, a member gets no value from it and reduces or ends that contract.
  Each side measures the other directly, and nobody can lie to them about it.

What survives from the global model is the part that was never global: a member
computing **their own** standing from their own pledge, availability and usage.
That needs no coordinator, and it is what `accounting.rs` actually does.

#### 4. Human-readable names — a real wall, and it is theoretical

This is **Zooko's triangle** (Zooko Wilcox-O'Hearn, around 2001): a name can be
human-meaningful, decentralised, or secure — pick two.

| Way out | Cost |
| --- | --- |
| Blockchain naming (Namecoin, ENS) | Genuinely squares the triangle, by requiring a global consensus system. Either depend on someone else's — fees, a public ledger, a different centralisation — or run your own, which needs a validator network that does not exist here. Disproportionate. |
| No global names | Identity *is* the public key. Syncthing's choice: device IDs are base32 keys exchanged out of band. Fully decentralised and fully secure; the user handles sixty characters. |
| Petnames | Everyone keeps their own mapping. Decentralised and readable locally, not globally unique. SSH's `known_hosts`. |

**ITSaNAS takes the second, and the username is demoted to a convenience.** The
public key is the identity and the root of trust. This costs less than it sounds,
because recovering your own account never needed a name: the 24 words derive the
master secret directly, with nothing to look up and nobody to ask.

#### 5. Escrow — the one place centralisation is genuinely better

The escrow blob is sealed under a passphrase, so that a new machine can be
recovered with a username and a passphrase instead of 24 words. On a coordinator,
a thief who steals the database can attack it offline; that is already admitted
in [ECONOMICS.md](ECONOMICS.md) §7.

In a DHT it is worse. The blob would be **public by construction**: anyone can
fetch it and grind it at leisure, with no rate limit and no trace.

Centralisation offers exactly one thing here that decentralisation cannot, and it
is the thing that matters: **somewhere to enforce a rate limit.**

### The decision

Not "with or without a server". **A server that carries nothing vital.**

| Job | Where it lives | Why |
| --- | --- | --- |
| Local discovery | **No server.** Signed UDP beacon on the LAN | Free, and covers a household fleet entirely |
| Remote discovery | Coordinator, with addresses **cached and pinned** by peers | It can be down without stopping anything already known |
| Placement | **The owner records it.** No global node set | Removes the agreement problem entirely |
| Accounting | **Bilateral**, per counterparty | No trusted bookkeeper, and self-enforcing |
| Identity | **The public key.** The name is a label | No naming authority to trust or to attack |
| Escrow | Coordinator | Only for the rate limit, with the trade written down |

What this buys: the coordinator degrades to an address book and a meeting point
for first contact — precisely the role a DHT would take over later, which is what
makes the swap contained instead of a rewrite. The claim below that the
coordinator "stays replaceable" was aspirational when it was written; the table
above is what makes it true.

What it still costs, stated plainly: the coordinator sees who is online and who
asks after whom. Nothing at this scale fixes that, a DHT least of all. It can
refuse to list a member — mitigated by the address being configuration, and by
the member being free to run their own, or none.

### Why not decentralise the rest now

Because **decentralisation has a minimum viable size and this network is far
below it.** A ten-node DHT is not decentralised, it is fragile: several thousand
independent participants are needed before dilution works at all. Building one
now means thousands of lines of new attack surface bought in exchange for a
*weaker* security property than a machine the owner physically controls.

The precedent is worth stating because it is the closest comparable system:
**Syncthing, after more than a decade, still runs central discovery and relay
servers.** Open source, self-hostable, several instances, avoidable with static
addresses — but they exist, because nobody has found better.

### Why there is one at all

Fully decentralised discovery — DHT, gossip membership — is achievable and was
the original preference. It was deferred for one reason: it doubles the attack
surface and the debugging surface at exactly the moment when neither the storage
layer nor the sync layer is proven.

### Why it is safe to have one

Its entire remit is control plane:

| Responsibility | Why it cannot become a data-plane risk |
| --- | --- |
| `username → public key` directory | Public keys are public |
| Presence and addresses | Metadata the peers exchange anyway |
| Signed node-set epoch | Signed; peers pin the last set they saw |
| Escrow blob storage | Argon2id-sealed under a passphrase it never sees |
| Relay (not built) | Would relay ciphertext it cannot open |

A compromised coordinator gets denial of service, lies about who is online, and
partition attempts. It does not get plaintext, keys, or the ability to forge a
signed log entry.

**Because the surface is control plane only, it stays replaceable.** Swapping it
for a DHT later touches no byte of the data plane.

### Escrow: convenience with an honest caveat

The escrow blob lets a user log in on a new machine with a username and
passphrase, rather than typing 24 words. Convenient, and it makes the passphrase
the weakest link for anyone who steals the coordinator's database.

Hence: Argon2id at 64 MiB / 3 passes / 1 lane, KDF parameters bound into the
associated data so cost downgrade fails
(`keystore::tests::downgrading_the_kdf_cost_is_detected`), and documentation
that is clear the **recovery phrase, not the passphrase, is the authoritative
backup**. Escrow is a convenience; the phrase is the guarantee.

---

## 9. Operational design

### It should feel like a folder

The daemon runs in the background; the user drops files in a directory and they
appear elsewhere. No sync button, no upload dialog.

### Invisible systems fail silently, so alert loudly

A system that is invisible when working must be conspicuous when broken. The
daemon raises explicit alerts for:

| Condition | Why it matters |
| --- | --- |
| Online node count below replication floor | New writes cannot reach their durability target |
| Chunks below minimum replicas past a grace period | Data is one failure from loss |
| No successful sync round in *N* minutes | Sync is broken, not merely quiet |
| A storage challenge fails | A host is not holding what it claims |
| Pledged space exhausted or unwritable | The node can no longer meet its side of the bargain |
| Free space below reserve | The host is about to start failing writes |

The distinction that matters is the third one: **quiet is not the same as
working.** A sync engine with nothing to do and a sync engine that has crashed
look identical from the outside, which is why elapsed-time-since-last-successful-round
is a first-class alert rather than an afterthought.

---

## 10. Language and dependencies

**Rust**, because: one static binary per platform with no runtime to install on
a Pi or a VM; clean cross-compilation to `x86_64-pc-windows-msvc` and
`aarch64-unknown-linux-gnu`; memory safety in code that parses attacker-supplied
bytes; and `unsafe_code = "forbid"` across the workspace.

Dependency policy for `itsanas-crypto` specifically: keep the list short enough
to audit. It pulls the RustCrypto primitives, BLAKE3, BIP-39, Argon2, and
nothing else. `zeroize`'s derive macro was declined in favour of a six-line
manual implementation, to avoid a proc-macro dependency in the crate whose whole
job is to be reviewable.

`cargo-deny` enforces licence compatibility with AGPL-3.0 and fails CI on any
unpatched advisory.
