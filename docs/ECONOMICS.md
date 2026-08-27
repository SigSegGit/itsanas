# The Bargain

What a member gives, what they get, and what happens when they stop giving.

This document exists because the project had a hole in exactly this shape: the
code could store a stranger's data and could refuse to store more than a
configured limit, and that was the whole of the "mutual" in mutual storage.
Nothing measured who owed whom, nothing noticed a member who took without
giving, and nothing decided what to do about it.

Everything here is a decision, not a description. Where a decision is arbitrary,
it says so. Where it is forced by arithmetic, the arithmetic is shown.

---

## 1. The core exchange

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

## 2. The uptime problem, which is bigger than it looks

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
makes the network work at all**. It is therefore a first-class placement rule
rather than a special-cased favour, and any member who runs an always-on machine
becomes an anchor automatically. No configuration, no blessing, no central
decision.

### A network with no anchors

It still works, and it says so. Placement falls back to three replicas chosen
normally, the coordinator marks the swarm `anchorless`, and clients warn that
files may be unreadable when peers are asleep. Data is still safe; it is just
not always reachable. Failing loudly here is the whole point — the alternative
is a member discovering it at the moment they need a file.

---

## 3. What a pledge is actually worth

```
effective contribution  =  pledged bytes × availability
entitlement             =  effective contribution ÷ 3
```

A 1 TB always-on Pi contributes 1 TB and earns 333 GB. A 1 TB laptop online a
quarter of the time contributes 250 GB and earns 83 GB.

That is harsh, and it is correct. The alternative — counting laptop bytes at
face value — means the network promises durability it cannot deliver, and the
people who discover this are the ones who lose files.

### How availability is measured, and who can lie about it

The coordinator observes presence and publishes a smoothed estimate in the
signed node-set epoch. It is therefore in a position to lie: inflating a
colluder's availability lets them store more than they contribute; deflating an
honest member's is denial of service.

This is tolerated, deliberately, and bounded:

- A dishonest coordinator can already refuse to list a member at all. Lying
  about a number is not a *new* power, it is a smaller version of one the threat
  model already grants.
- The damage is economic, never confidential and never destructive. No amount of
  lying about uptime lets anyone read a byte or delete one.
- **Availability affects entitlement only.** It does not affect placement.
  Placement uses *locally observed* reliability — whether a host answered this
  node's own storage challenges — which no third party can forge. The decision
  that risks data does not depend on the untrusted party. The decision that
  risks fairness does.
- Members pin the last node set they saw and can change coordinator without
  losing anything, because the coordinator holds no data and no keys.

Availability is clamped to `[0.05, 1.0]`. The floor stops a member who has been
offline for a month from having their entitlement collapse to zero and being
declared in default the moment they come back — a punishment for going on
holiday.

---

## 4. Reducing what you offer

A member may lower their pledge at any time. Nobody should be trapped in a
network by their own disk.

What must never happen: their peers' data silently disappearing because someone
wanted their disk back.

**Lowering the pledge below what is already stored is allowed.** The node keeps
serving everything it already accepted and simply accepts nothing new. It stays
a good citizen for the data it already holds while draining naturally as peers
delete files and repair moves replicas elsewhere.

A node that wants its space back *now* uses `itsanas evict`, which:

1. announces the eviction to the coordinator, so repair can start immediately;
2. keeps serving every affected chunk for `EVICTION_NOTICE` (7 days) while other
   nodes take copies;
3. only then deletes, and only chunks confirmed to be at or above the
   replication floor elsewhere.

A node that simply vanishes gets none of this, which is why repair exists. The
notice period is the difference between leaving politely and leaving rudely, and
the network survives both — it just does more work for the second.

---

## 5. When a member takes more than they give

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

### Grace is entered on a schedule, not on a spike

State transitions use a smoothed usage figure over seven days. A member who
copies a large folder in and deletes it an hour later has not defaulted on
anything, and a system that reacts within the hour would tell them they had.

---

## 6. Joining, and the bootstrapping problem

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

## 7. What the coordinator is, and is not

It is a **notice board**. It holds:

| It holds | Why it is safe |
| --- | --- |
| `username → user id` | User ids *are* public keys. A coordinator that lies produces a name that maps to a key nobody can use. |
| Presence and addresses | Metadata peers exchange anyway. |
| Availability estimates | Affects entitlement only, never placement — see §3. |
| Signed node-set epochs | Signed and pinned; a coordinator serving two different sets is detectable by comparing epochs with any peer. |
| Escrow blobs | Argon2id-sealed under a passphrase it never sees. |
| Accounting state | Derived from data it already holds; wrong numbers are unfair, never destructive. |

It does not hold: keys, plaintext, chunks, or the authority to delete anything.

**A member can run their own.** The coordinator address is configuration. Three
friends can run one between them; Nicolas can run one on a VPS; someone who
trusts nobody can run one alone and lose only the ability to find strangers.
This is the property that keeps the coordinator from becoming the product.

---

## 8. Constants, in one place

| Name | Value | Kind |
| --- | --- | --- |
| `CONTRIBUTION_RATIO` | 3 | Forced: equals the replication factor |
| `REPLICATION_FLOOR` | 3 | Judgement: smallest R where one loss is not an emergency |
| `ANCHOR_AVAILABILITY` | 0.90 | Judgement |
| `AVAILABILITY_FLOOR` | 0.05 | Judgement: stops a holiday becoming a default |
| `GRACE` | 14 days | Judgement: longer than a holiday |
| `DEFAULT_AFTER` | 60 days | Judgement: shorter than a forgotten machine |
| `EVICTION_NOTICE` | 7 days | Judgement |
| `JOINING_ALLOWANCE` | 10 GB | Judgement |
| `JOINING_PERIOD` | 30 days | Judgement |
| `USAGE_SMOOTHING` | 7 days | Judgement: longer than a transient copy |

---

## 9. Questions this document does not answer

Recorded rather than hidden, because they are the ones that will matter next.

- **What stops a host claiming to store data it discarded?** Storage challenges
  prove possession at a moment, not continuously, and a host that fetches a
  chunk from another replica just in time passes. The honest position: challenges
  raise the cost of lying without eliminating it, and the real protection is
  replication across parties with no reason to collude. Erasure coding across
  six or more independent nodes would change this; it is deferred.
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
