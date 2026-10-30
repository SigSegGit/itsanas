# How it works

One file, from your disk to other people's disks and back. The full version, with
every constant, is [ARCHITECTURE](https://github.com/SigSegGit/itsanas/blob/main/docs/ARCHITECTURE.md);
this page is the shape.

## The life of a file

```mermaid
flowchart TD
  F["A file lands in your synced folder"] --> C["Split into chunks by content<br/>(FastCDC)"]
  C --> S["Sealed on your device<br/>client-side encryption"]
  S --> N["Stored under a blinded address<br/>the host cannot map back to a name"]
  N --> R["Copies placed on other members' machines"]
  R --> Q{"Audit: can the host prove<br/>it still holds a chunk?"}
  Q -- "yes" --> R
  Q -- "no" --> W["Record withdrawn,<br/>chunk counts as under-replicated"]
  W --> U["Re-sent in the same round"]
  U --> R
```

The audit draws chunks in an order the host cannot predict. That unpredictability
is the whole mechanism, and it took three attempts to get right; the story is in
[ROADMAP](https://github.com/SigSegGit/itsanas/blob/main/docs/ROADMAP.md).

## Getting your data back on a new machine

```mermaid
sequenceDiagram
  participant M as New machine
  participant K as Coordinator
  participant H as Hosts
  M->>K: login with username and passphrase
  K-->>M: sealed recovery container (cannot be opened without the passphrase)
  M->>K: register this device, ask where the account's machines are
  K-->>M: published addresses
  M->>H: sync
  H-->>M: your encrypted chunks
  Note over M: opened and checked on this machine only
```

This was run on the real fleet with the writing machine switched off: the files
came back byte for byte. The evidence is in
[MVP](https://github.com/SigSegGit/itsanas/blob/main/docs/MVP.md) section 6, test D.

## Layers

Every arrow points down: no lower layer knows about a higher one, and
`itsanas-crypto` depends on nothing else in the project, which is what makes it
auditable alone.

```mermaid
flowchart TD
  shells["itsanas CLI · Android app · trays"]
  node["itsanas-node<br/>keystore, configuration, one sync round"]
  logic["itsanas-sync · itsanas-policy · itsanas-placement<br/>merge, what a device keeps, where copies go"]
  io["itsanas-store · itsanas-folder · itsanas-net<br/>chunks and blobs, watched folder, peer protocol"]
  base["itsanas-crypto · itsanas-discover<br/>identity and sealing, finding peers on the LAN"]
  shells --> node --> logic --> io --> base
```

Back to [Home](Home) · [Roadmap](Roadmap).
