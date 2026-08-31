# The Bargain

What a member gives, what they get, and what happens when they stop giving.

This document exists because the project had a hole in exactly this shape: the
code could store a stranger's data and could refuse to store more than a
configured limit, and that was the whole of the "mutual" in mutual storage.
Nothing measured who owed whom, nothing noticed a member who took without
giving, and nothing decided what to do about it.

Everything here is a decision, not a description. Where a decision is arbitrary,
it says so. Where it is forced by arithmetic, the arithmetic is shown.

> **This is a specification, and most of it is not built yet.** Reading a
> mechanism here does not mean the code does it. Every section carries one of
> these markers, and the rule for the whole document is that an unmarked
> present-tense sentence is a bug in the document:
>
> | Marker | Meaning |
> | --- | --- |
> | ✅ **built** | Implemented and covered by tests named in [TESTING.md](TESTING.md) |
> | 🟨 **partly built** | The rule exists in code; the thing that would apply it does not |
> | ⬜ **specified only** | Decided here, no code. Nothing enforces it today |
>
> [ROADMAP.md](ROADMAP.md) is the authority on status; if the two disagree,
> ROADMAP is right.

---

## 1. The core exchange ✅ **built**

> **Pledge three times what you store.**

A member who wants `S` bytes of their own data protected must offer `3 × S` bytes
of their own disk to other members.

### Why three, and not a number picked because it sounded generous

It is not arbitrary. With a replication factor of `R`, the network as a whole
must physically hold `R × S` bytes for every `S` bytes a member stores. If every
member pledges `C × S`, the network holds `C × S` per member. So:

```
network capacity needed  =  R × S
network capacity offered =  C × S
balanced when            =>  C = R
```

**The contribution ratio and the replication factor are the same number.** Three
replicas means pledge three times. Choosing `C = 3` is choosing `R = 3`, and
`R = 3` is the smallest number where losing one machine is not an emergency and
losing two simultaneously is required to lose anything.

Anything above `C = R` is headroom for churn, repair traffic and members who
join before they contribute. Anything below is a network that has promised more
than it can hold, which will discover this by losing files.

### What this is not

It is not a market, a currency, or a reputation score. There is no price, no
trading, and no way to buy your way out of contributing. Those are interesting
and they are all a second system to secure and get wrong. The rule is one
number, checkable by anyone with the node set, and a member can verify their own
standing without asking permission.

---

## 2. The uptime problem, which is bigger than it looks 🟨 **partly built**

Nicolas's observation, which turned out to be structural rather than a detail:

> *a laptop can store data, but you cannot expect it to be online more than a
> quarter of the day*

Bytes on a machine that is usually off are not worth the same as bytes on a
machine that is always on. Pretending otherwise makes the whole accounting a
fiction. But the honest version is worse than "worth less" — it is worth
working out how much worse.

### The arithmetic nobody wants

For a chunk with `R` replicas on hosts each online a fraction `u` of the time,
the chance that **none** of them is reachable right now is `(1 - u)^R`. To read
a file with probability `A`:

```
R  =  ln(1 - A) / ln(1 - u)
```

| Host availability `u` | Replicas needed for 99% read availability |
| --- | --- |
| 0.99 (a server) | 1 |
| 0.90 (a NAS that reboots) | 2 |
| 0.50 (a desktop) | 7 |
| **0.25 (a laptop)** | **16** |
| 0.10 (a laptop that travels) | 44 |

Sixteen replicas is not a tuning problem. It is a different system. A network of
laptops alone cannot offer read-on-demand at any sane storage cost, and any
design that claims otherwise is either lying or quietly accepting that your
files are often unavailable.

### The consequence: durability and availability are separate purchases

They have been conflated throughout this project so far, and they should not be.

- **Durability** — the data is not lost. Needs `R = 3` distinct nodes. Uptime is
  irrelevant: a switched-off laptop still holds its bytes.
- **Availability** — you can read it *now*. Needs at least one replica on a node
  that is actually up.

So the rule becomes two rules:

> **Three replicas for durability. At least one of them on a high-availability
> node — an *anchor* — for availability.**

An anchor is any node whose measured availability exceeds
`ANCHOR_AVAILABILITY` (0.90). A Raspberry Pi that stays on qualifies. A VPS
qualifies. A laptop does not.

This makes Nicolas's "reserve" idea — his own always-on equipment absorbing the
uncertainty of everyone else's laptops — **not a nice-to-have but the thing that
makes the network work at all**. The decision that follows is that it must be a
first-class placement rule rather than a special-cased favour: any member who
runs an always-on machine becomes an anchor automatically, with no
configuration, no blessing and no central decision.

> ⚠️ **Not built.** `accounting.rs` computes `is_anchor` and `has_anchor` from
> measured availability, and that part is tested. **`itsanas-placement` knows
> nothing about anchors**: `NodeSet::replicas_for` takes an owner, a chunk and a
> count, and weights only by pledged capacity. Nothing today guarantees that one
> replica lands on a machine that is up. Wiring this is part of M5 and needs the
> node set a coordinator would publish — see [ROADMAP.md](ROADMAP.md) M5/M6.

### A network with no anchors ⬜ **specified only**

The intended behaviour: placement falls back to three replicas chosen normally,
the coordinator marks the swarm `anchorless`, and clients warn that files may be
unreadable when peers are asleep. Data stays safe; it is just not always
reachable, and the member is told so. Failing loudly is the whole point — the
alternative is discovering it at the moment you need a file.

None of the warning exists yet. There is no coordinator and placement has no
anchor concept, so **today every swarm is the anchorless case and nothing says
so**. That is the honest reading of the current state: durability is
implemented, availability is not, and the silence is the part to fix first —
before the placement rule, because a wrong belief about availability is worse
than a known absence of it.

---

## 3. What a pledge is actually worth 🟨 **partly built**

> **Superseded in part.** This section originally described a *global* ledger:
> a coordinator measuring everyone's availability, computing everyone's
> entitlement, and publishing the result. That needs a trusted accountant, which
> is the one role the rest of the design refuses to grant anybody, and it was
> giving the coordinator a job it should never have had.
>
> The direction is now **bilateral** — each pair of members tracks what it holds
> for the other — with the arithmetic below kept as a member's **self-assessment**
> of their own standing, which needs nobody's cooperation. See
> [DESIGN.md](DESIGN.md) §8 for the full argument and §4 below for what changes.

```
effective contribution  =  pledged bytes × availability
entitlement             =  effective contribution ÷ 3
```

A 1 TB always-on Pi contributes 1 TB and earns 333 GB. A 1 TB laptop online a
quarter of the time contributes 250 GB and earns 83 GB.

That is harsh, and it is correct. The alternative — counting laptop bytes at
face value — means the network promises durability it cannot deliver, and the
people who discover this are the ones who lose files.

### Bilateral, not global ⬜ **specified only**

Two members who host for each other each keep one number per counterparty:

```
what I hold for them   —   what they hold for me
```

Both sides can compute it, neither can forge the other's view of it, and no
third party is consulted. If it goes badly out of balance the injured side stops
accepting new data from the other. That is BitTorrent's tit-for-tat, which has
survived twenty years on an openly hostile network.

Two properties fall out of it that the global model had to be told explicitly:

- **The 3x ratio appears on its own.** Wanting three replicas of 100 GB means
  finding three counterparties and giving each of them 100 GB back.
- **Availability needs no third-party measurement.** A peer that is never
  reachable is worth nothing to you, and you can see that yourself. Each side
  measures the other directly, and nobody is in a position to lie to them about
  it.

What this costs: matching is less efficient. There is no common pot to draw
from, so a member has to find counterparties who want roughly what they want.
At the scale of a household, or a few friends, that is not a problem — and by
the time it is, there are enough members for it to be solvable in ways that do
not exist yet at three.

### How availability is measured, and who can lie about it

A member computes their **own** availability from their own uptime, and that
needs nobody. Where a coordinator is used, it also observes presence and can
publish an estimate — `Directory::tick` measures this from observed presence
rather than accepting a claim, and is tested — and a coordinator that does so is
in a position to lie: inflating a
colluder's availability lets them store more than they contribute; deflating an
honest member's is denial of service.

This is tolerated, deliberately, and bounded:

- A dishonest coordinator can already refuse to list a member at all. Lying
  about a number is not a *new* power, it is a smaller version of one the threat
  model already grants.
- The damage is economic, never confidential and never destructive. No amount of
  lying about uptime lets anyone read a byte or delete one.
- **Nothing a coordinator says reaches placement.** This is a rule, not an
  accident: the decision that risks *data* must never depend on a number an
  untrusted party produced. An owner records which peers hold each of its
  chunks, from peers it has itself reached — see [DESIGN.md](DESIGN.md) §8. The
  *recording* is built; the *choosing* is not, and at a household size the
  policy is "offer to every peer", which is correct by accident. A lie about uptime can therefore cost fairness and
  cannot cost a replica.

  > Owner-recorded placement is **built**: the `HOLDERS` table records which
  > peers hold each chunk, filled on every sync round, and `itsanas status`
  > reports whether the data exists anywhere but this disk.
  >
  > ⚠️ The intended reliability input — preferring hosts that answered **this
  > node's own** storage challenges, which no third party can forge — is not.
  > The challenge protocol works and is tested; nothing records the result, so
  > there is no reputation to consult.
- Members pin the last node set they saw and can change coordinator without
  losing anything, because the coordinator holds no data and no keys.

Availability is clamped to `[0.05, 1.0]`. The floor stops a member who has been
offline for a month from having their entitlement collapse to zero and being
declared in default the moment they come back — a punishment for going on
holiday.

---

## 4. Reducing what you offer ⬜ **specified only**

A member may lower their pledge at any time. Nobody should be trapped in a
network by their own disk.

What must never happen: their peers' data silently disappearing because someone
wanted their disk back.

**Lowering the pledge below what is already stored is allowed.** The node keeps
serving everything it already accepted and simply accepts nothing new. It stays
a good citizen for the data it already holds while draining naturally as peers
delete files and repair moves replicas elsewhere.

A node that wants its space back *now* uses `itsanas evict`. **This command does
not exist**; what follows is the specification for it. Today, lowering a pledge
is the only exit, and it drains at whatever rate peers delete files:

1. announces the eviction to the coordinator, so repair can start immediately;
2. keeps serving every affected chunk for `EVICTION_NOTICE` (7 days) while other
   nodes take copies;
3. only then deletes, and only chunks confirmed to be at or above the
   replication floor elsewhere.

A node that simply vanishes gets none of this, which is why repair exists. The
notice period is the difference between leaving politely and leaving rudely, and
the network survives both — it just does more work for the second.

---

## 5. When a member takes more than they give 🟨 **partly built**

Their entitlement drops below their usage. This happens innocently: a disk dies,
a machine is retired, a member's uptime falls after a job change.

The response is graduated, and the first principle is worth stating in isolation
because it constrains everything else:

> **The network never deletes a member's data as a punishment.**

Deleting someone's files because they were short on disk space is a
disproportionate response to a bookkeeping problem, and a system that can do it
by accident will eventually do it by accident. Sanctions restrict *new*
commitments; they never destroy existing ones.

| State | Condition | What changes |
| --- | --- | --- |
| **Good** | usage ≤ entitlement | Nothing. |
| **Over** | usage > entitlement | A warning in `status`. Everything keeps working. |
| **Grace** | Over for more than `GRACE` (14 days) | New writes are stored on the member's own devices and are not replicated to peers. Existing data is untouched and still repaired. |
| **Default** | Over for more than `DEFAULT_AFTER` (60 days) | Hosts may reclaim space from this member, oldest and best-replicated chunks first, never below one remaining copy on the member's own devices. |

Even in default, a member's data survives on their own machines — which is where
it was before they joined. The worst outcome of total economic failure is
returning to a local backup. That is a floor worth engineering for.

Fourteen and sixty days are arbitrary. They are chosen to be longer than a
holiday and shorter than a forgotten machine, and they are configuration, not
constants in the protocol.

### Grace is entered on a schedule, not on a spike ⬜ **specified only**

State transitions are meant to use a usage figure smoothed over seven days.
`Assessment` documents this as its caller's responsibility and is tested against
whatever figure it is handed; the caller that would do the smoothing is part of
the coordinator server and does not exist. A member who
copies a large folder in and deletes it an hour later has not defaulted on
anything, and a system that reacts within the hour would tell them they had.

---

## 6. Joining, and the bootstrapping problem 🟨 **partly built**

A new member has contributed nothing and has data to protect. Requiring
contribution before storage means nobody can ever start.

**A joining member gets `JOINING_ALLOWANCE` (10 GB) of entitlement for
`JOINING_PERIOD` (30 days), regardless of pledge.** Enough to be useful
immediately, small enough that farming it with throwaway identities buys little.

Identities are free — they are just keypairs — so this is deliberately not a
defence against a determined attacker. It is a defence against *friction*. The
real defence against identity farming is that anchors choose whom they host for,
and a network of three friends does not have this problem at all. When it
becomes a real problem, the answer is invitation, not a bigger number.

---

## 7. What the coordinator is, and is not ✅ **built**

It is a **notice board**. It holds:

| It holds | Why it is safe |
| --- | --- |
| `username → user id` | User ids *are* public keys. A coordinator that lies produces a name that maps to a key nobody can use. |
| Presence and addresses | Metadata peers exchange anyway. |
| Escrow blobs | Argon2id-sealed under a passphrase it never sees. This is the one job where centralisation is genuinely better than the alternative, because it is somewhere a rate limit can exist — see [DESIGN.md](DESIGN.md) §8. |

**Removed from this list, deliberately.** Earlier versions of this document also
gave the coordinator a signed node-set epoch, availability estimates that fed
entitlement, and the accounting state for every member. All three are gone:

| Was | Now |
| --- | --- |
| Signed node-set epoch | Nothing. Placement is recorded by the owner, so no global membership list has to be agreed. |
| Availability estimates | A member measures their own, and each pair measures each other. A coordinator may still publish a hint; nothing depends on it. |
| Accounting state | Bilateral, held by the two parties to each arrangement. |

What is left is an address book. That is the point: a component that holds
nothing vital can be switched off, replaced, self-hosted, or eventually swapped
for a DHT without any of it being a migration.

It does not hold: keys, plaintext, chunks, or the authority to delete anything.

**A member can run their own.** The coordinator address is configuration. Three
friends can run one between them; Nicolas can run one on a VPS; someone who
trusts nobody can run one alone and lose only the ability to find strangers.
This is the property that keeps the coordinator from becoming the product.

---

## 8. Constants, in one place

"Live" means a constant in the code that something reads. "Paper" means the
number is decided here and nothing consumes it.

| Identifier in the code | Value | Kind | Status |
| --- | --- | --- | --- |
| `accounting::CONTRIBUTION_RATIO` | 3 | Forced: equals the replication factor | live |
| `repair::DEFAULT_REPLICATION_FLOOR` | 3 | Judgement: smallest R where one loss is not an emergency | live |
| `accounting::ANCHOR_AVAILABILITY_PER_MILLE` | 900 (0.90) | Judgement | live, but only to *label* an anchor — placement never reads it |
| `accounting::AVAILABILITY_FLOOR_PER_MILLE` | 50 (0.05) | Judgement: stops a holiday becoming a default | live |
| `accounting::GRACE_SECONDS` | 14 days | Judgement: longer than a holiday | live |
| `accounting::DEFAULT_AFTER_SECONDS` | 60 days | Judgement: shorter than a forgotten machine | live |
| `accounting::JOINING_ALLOWANCE` | 10 GiB | Judgement | live |
| `accounting::JOINING_PERIOD_SECONDS` | 30 days | Judgement | live |
| `directory::SMOOTHING_ALPHA_PER_MILLE` | 10 | Judgement: how fast measured availability moves | live |
| `EVICTION_NOTICE` | 7 days | Judgement | **paper** — no such constant; no eviction exists |
| `USAGE_SMOOTHING` | 7 days | Judgement: longer than a transient copy | **paper** — `Assessment` documents that its caller must smooth; no caller exists |

---

## 9. Questions this document does not answer

Recorded rather than hidden, because they are the ones that will matter next.

- **What stops a host claiming to store data it discarded?** Audits, now — the
  daemon challenges every peer it contacts on a sample of what the ledger says
  that peer holds, and a host that cannot answer has the record withdrawn, so
  the chunk counts as under-replicated and is re-uploaded in the same round.
  Verified by running it: a host that deleted everything it had accepted was
  caught on the next round and the data was restored automatically.

  **The questions are drawn, and the host cannot work out the draw.** This is
  the property the whole mechanism rests on, and it took three attempts.

  The first version worked through the least recently confirmed records, which
  reads like diligence — but a push round re-stamps every record the peer
  *claims* to hold, a whole batch per clock reading, so within a batch every
  timestamp was equal and the sort fell through to its tie-break, the chunk id.
  The same sixteen lowest ids, every round, for ever. A host could keep sixteen
  chunks out of fourteen million and hold a spotless record.

  The second drew a random cursor and asked about the record at or after it.
  That is not uniform: it picks each record with probability proportional to the
  **gap** before it, and gaps between random 256-bit ids are exponentially
  distributed, so they are very uneven. Harmless only while the host cannot tell
  which of its chunks sit behind the widest gaps — and ordered by chunk id it
  could, because it received the chunks. Simulated at sixteen questions a round:

  | host keeps | discarding at random | discarding the narrow gaps |
  | --- | --- | --- |
  | 90% | passes 18% of rounds | passes **92%** |
  | 50% | passes 0.0015% | passes 6.9% |
  | 10% | passes 1e-16 | passes 2e-08 |

  A host silently losing a tenth of somebody's files would have been invisible.

  So the ledger is ordered by `BLAKE3_keyed(audit key, chunk)`, a key derived
  from the owner's master secret that never leaves their machine. The gaps stay
  exactly as uneven and become unguessable, the host's best remaining strategy
  is to discard at random, and a host keeping a fraction `f` then does survive a
  round of `n` questions with probability `f` to the `n`: 18% at nine tenths of
  the data, one in fifty million over ten rounds.

  Coverage becomes probabilistic instead of exhaustive, which is no loss at all:
  the exhaustive version was exhaustive over sixteen chunks.

  A host that fails three rounds in a row stops being sent new content, which
  is the same sanction shape as everything else here: it restricts new
  commitments, destroys nothing, keeps receiving the log so it can still relay,
  and is cleared by answering. It also keeps receiving **one chunk per round**
  — a probe, and not a rounding error. A paused peer's other records are the
  ones it is paused *for*, so drawing its questions from them would guarantee
  failure and make the suspension a ban with a friendlier message. The probe is
  written down when it is accepted, and a paused peer is audited on that chunk
  **and nothing else**.

  Two details of that probe, each of which was wrong once and each of which
  decides whether the sanction means anything.

  **The owner picks the chunk.** The first version took it from the peer's own
  answer to "what are you missing?", which handed a host its own examination
  question: name one small chunk, keep it, buy back the terabyte you threw away.

  **Answering pays off one failure, not all of them.** Zeroing the counter was
  the other half of the same mistake. A host could discard everything, take its
  three failures, keep one probe chunk for one round, be fully trusted again,
  receive the whole store, discard it, and repeat for ever — the sanction
  costing it one chunk held for one round. So three failures now cost three
  answered rounds and thirty cost thirty, bounded at thirty-five so that
  probation never becomes a ban by arithmetic. A machine that genuinely lost a
  disk is back within a quarter of an hour.

  The limit is unchanged and still honest: a challenge proves possession at a
  moment, not continuously, and a host that fetches a chunk from another replica
  just in time passes. Challenges raise the cost of lying without eliminating
  it, and the real protection is replication across parties with no reason to
  collude. Erasure coding across six or more independent nodes would change
  this; it is deferred.

  What is still missing, and worth naming: the sample is drawn from the
  **owner's ledger**, so it can only ask about chunks the owner recorded the
  peer as taking. It says nothing about a host that accepted nothing in the
  first place, which is what the pledge accounting is for.
- **What if the coordinator disappears?** Peers keep their pinned node set and
  keep syncing with known addresses. New members cannot join and addresses go
  stale. Nothing is lost; the network stops growing. A second coordinator, or a
  DHT, removes this and is deferred rather than dismissed.
- **Should members be able to share files with each other?** Not in v1. Mutual
  *storage* needs no key exchange — a host stores opaque bytes.
  `UserKeys::agree` exists, is tested, and is deliberately unused: it is the
  primitive sharing will need, kept because removing and re-adding cryptographic
  code is how mistakes enter.
- **Is a single number the right unit?** It ignores bandwidth and IOPS. A member
  offering 10 TB on a 1 Mbit uplink is worth much less than the number says.
  Bandwidth accounting is deferred, deliberately: it is easy to measure badly and
  the failure mode of measuring it badly is punishing people for their ISP.
