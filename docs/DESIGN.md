# Design Notes

Why each mechanism is the way it is, and what was rejected. Structural overview
lives in [ARCHITECTURE.md](ARCHITECTURE.md); this document is the reasoning.

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
score(node, chunk) = weight(node) / -ln(uniform_hash(node_id ‖ chunk_id))
replicas = top R nodes by score
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

## 7. The coordinator

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
| QUIC relay | Relays ciphertext it cannot open |

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

## 8. Operational design

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

## 9. Language and dependencies

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
