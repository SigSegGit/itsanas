# Security Policy

## Status

ITSaNAS is pre-release. The cryptographic core is implemented and tested; the
storage, sync and network layers are not yet written. **Do not use it for data
you cannot afford to lose or to leak.** It has had no external audit.

## Threat model

### The host is the adversary

Every machine storing your data is assumed hostile. It may:

- read every byte it stores, and keep it forever
- return corrupted, truncated, or stale bytes
- serve one chunk when asked for another
- lie about what it holds, or about its capacity
- collude with any number of other hosts

None of that may yield plaintext, and all of it must be detected. See
[docs/TESTING.md](docs/TESTING.md) — the tests asserting these attacks *fail*
are the ones that matter most.

### The coordinator is untrusted

It may lie about who exists, who is online, and what the node set is. It holds
no keys and no decryptable data. A compromised coordinator gets denial of
service and partition attempts, not data.

### Out of scope

- A compromised endpoint. If an attacker has your unlocked device, they have
  your data. Nothing in this design changes that.
- Traffic analysis. Object sizes, object counts and access timing are visible to
  a host. Hiding them requires padding and cover traffic, a deliberate non-goal
  for now.
- Availability against a fully offline swarm. Your own devices always hold your
  own data, so you are never locked out of your files, but replication targets
  need peers.

## Published test keys — not a vulnerability

## The transport is not yet encrypted

`itsanas-net` currently speaks plain TCP. Your **data** is not at risk from
this — chunk bodies and log-segment bodies are sealed before they reach the
wire, and segment envelopes are signed, so a man in the middle can neither read
a payload nor forge one a peer will accept.

Your **metadata** is. An observer on the network path sees chunk identifiers,
object sizes and timing. The threat model grants a host all three; it does not
grant them to an arbitrary network between two of your own machines. An observer
recording chunk identifiers can tell when you touch the same file again and can
correlate two of your devices.

`PeerServer::bind` therefore refuses a non-loopback address unless the caller
explicitly overrides it. Until QUIC with TLS lands, run ITSaNAS over loopback, a
VPN, or an SSH tunnel.

The recovery phrases and private keys in [docs/TEST-USERS.md](docs/TEST-USERS.md)
are public **by design**, so anyone can reproduce the test suite. Those three
identities are refused by `itsanas_store::Store::open`, which calls
`itsanas_crypto::is_published_test_identity`. The fixtures themselves reach a
store only through `Store::open_for_testing`, a separate and deliberately
awkward constructor. Every layer added above the store must apply the same check
at its own entry point.

Please do not report them as a leak. Do report it if you find a path where a
node **accepts** one of them.

## Reporting a vulnerability

Report privately through GitHub's **Report a vulnerability** button on the
Security tab of the repository, or by email to
`nicolas.girard.e@gmail.com`.

Please include what you were able to do, not just what looks wrong. A working
proof of concept — a case where a host reads plaintext, forges a signed log
entry, or causes silent data loss — will be acted on quickly.

This is a spare-time project, so expect an acknowledgement within a week rather
than within hours. Please give a reasonable window before public disclosure.

## What counts as a critical finding

Anything that breaks one of these:

1. A host can read, or confirm a guess about, data it stores.
2. A host can modify stored data without detection.
3. An attacker can forge a signed operation-log entry or head record.
4. An identity can be recovered without the recovery phrase or the passphrase.
5. Data can be silently lost — a delete that destroys a concurrent edit, or a
   repair loop that drops the last replica.
6. A published test identity is accepted by a production node.
