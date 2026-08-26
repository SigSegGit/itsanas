# Published Test Users

> ## ⚠️ These private keys are public
>
> The recovery phrases below are printed in full, on purpose, so that anyone can
> clone this repository and reproduce every encryption, sync and adversarial
> test byte for byte. **They belong to the entire internet.** Never use them,
> or the passphrase below, for anything real.

## Why they are safe to publish

Publishing working private keys is normally reckless. Three mechanisms make it
safe here, and each is enforced by a test.

### 1. Production refuses these identities outright

`itsanas_crypto::is_published_test_identity` returns `true` for all three user
ids. A production node must refuse to host data for them, serve their chunks, or
accept an operation-log segment signed by them. Anyone can sign as Alice — and
it buys them nothing in a live swarm.

The ban list lives in `itsanas-crypto`, not in the test kit, so production code
enforces it without depending on test-only crates.

*Enforced by:* `wellknown::tests::the_ban_list_matches_the_actual_fixture_identities`,
`wellknown::tests::ordinary_identities_are_not_banned`,
`tests::every_fixture_identity_is_banned_in_production`.

### 2. There is no test data to tamper with

Every byte of the corpus is **generated** from seeds written in
`crates/itsanas-testkit/src/lib.rs`. There is no fixture directory to swap, no
archive to poison, no binary blob in git. Changing the test data means changing
reviewed source code in a pull request.

This is the direct answer to "someone could host something malicious by updating
test data": there is no data file to update.

### 3. The corpus is pinned by digest

Every file's BLAKE3 digest and a digest over the whole corpus are constants in
source, republished below, and checked on every CI run. Any change — accidental
or hostile — fails CI *and* moves a value a reader can verify by hand:

```bash
cargo run -p itsanas-testkit --bin generate-fixtures
```

*Enforced by:* `tests::corpus_matches_its_published_digests`.

**Corpus digest:**

```
72a9f85576aaf16ecfe6a7ad8079c00690a03a9d7c9d3aec9ec05895ca88ae02
```

## How the identities are derived

Deterministically, so they are reproducible on any machine:

```
master_secret = BLAKE3::derive_key(
    context     = "itsanas test fixture entropy - NOT SECRET",
    key_material = username_as_utf8)
```

The context string is deliberately unlike any production derivation context in
`itsanas_crypto::kdf`, so fixture material can never collide with a real user's
keys.

Shared keystore passphrase for all three:

```
itsanas-test-users-are-public-do-not-reuse
```

---

## alice

A laptop user: documents, a photo, an empty file.

| Field | Value |
| --- | --- |
| Username | `alice` |
| User id | `9bac48121994630c0f436bb20cf632daefb9a941b28c239c37491f6b9fa58ffe` |
| Master secret | `d1117a667f9793821577b77cee4c15abc694b62d7586c847cc63ea39ed85b030` |
| X25519 agreement key | `d8ae6e72a0461ec36398babcb0931aec4c7d8f9d0765596c21d728a552a7c93b` |
| Canary | `ITSANAS-CANARY-ALICE-4f21c8d0` |
| Plaintext total | 528 602 bytes |

**Recovery phrase**

```
speed mesh office you junior scissors fiction want language include air fiscal
harsh force remind radio sign dinosaur body stamp paddle security school brand
```

**Files**

| Path | Bytes | BLAKE3 digest |
| --- | ---: | --- |
| `notes/architecture.md` | 111 | `703346cc4634b513141d2cdf05cdf8975623a693ea979d6637eb0a0fea6985f9` |
| `finance/taxes-2026.csv` | 4 096 | `917bfd933ae52d158534c541b77e4fd8787be9af5a0c98f3add48e1969f98c3b` |
| `photos/holiday.jpg` | 524 288 | `4ebad42d75cd47a195374ca6fadfa6e4b2c841957e3293ccedf3ced5a28e5572` |
| `empty.txt` | 0 | `af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262` |
| `shared/common.txt` | 107 | `e1bbbbc9dc8b32d986d3f7b318e7d4ead364a3db7b5740a0a943e0734239564a` |

---

## bob

A Raspberry Pi user: source code, media, a password database.

| Field | Value |
| --- | --- |
| Username | `bob` |
| User id | `36a32e8ead6bff89bdb6a6e95a6ebe6586d4d420d2e81f4141952e4d2a36a86b` |
| Master secret | `f5c2b21fd3738611f0dfd4666d2abc003a570ec080754c4b5cb298b275bb582c` |
| X25519 agreement key | `ae4f8ee544b057770e18b254be1cc590d754412cb847eb6b9ea0ee9b721df618` |
| Canary | `ITSANAS-CANARY-BOB-9e73a1b5` |
| Plaintext total | 1 066 184 bytes |

**Recovery phrase**

```
volume better margin plunge debris angry sell whisper grid harsh pyramid about
pistol manual acoustic attitude era foot coach cousin chef tank gaze naive
```

**Files**

| Path | Bytes | BLAKE3 digest |
| --- | ---: | --- |
| `code/main.rs` | 86 | `54ad78937a0dec1f1e4a78760fa49783fe22f0d661417e9712f53eb2fec4f7de` |
| `secrets/passwords.kdbx` | 17 415 | `b29337a12662c2d275e0b32c4a01b01e3b71d7681796ba86c6f53885894dd5dd` |
| `music/track.flac` | 1 048 576 | `b2ae8395456a3bb4482f3f97176ef9ebedf8e923355d7846b917def820db3936` |
| `shared/common.txt` | 107 | `e1bbbbc9dc8b32d986d3f7b318e7d4ead364a3db7b5740a0a943e0734239564a` |

---

## carol

A virtual-machine user: a thesis, measurement data, logs.

| Field | Value |
| --- | --- |
| Username | `carol` |
| User id | `a9b01ef62c7ff2433c2adb7721ba23b7a3da021b70c189e8031cf1b440ec7d13` |
| Master secret | `1d8e5cf85f5de7835c1b632b678b15539c8c2f0abae565214291f15efe6ddba1` |
| X25519 agreement key | `dfa1f215d909bcbc4eaad39ad83aad956512ba5aaf746bd5ee4f50d65b53f14c` |
| Canary | `ITSANAS-CANARY-CAROL-2c60f8ae` |
| Plaintext total | 335 979 bytes |

**Recovery phrase**

```
budget indicate dignity salt taxi script idea hockey clock detail shed poet
silver bleak cliff frequent gown anxiety piece tired useful dad hub climb
```

**Files**

| Path | Bytes | BLAKE3 digest |
| --- | ---: | --- |
| `thesis/chapter-1.tex` | 8 192 | `d668f7764c267d34db15542481e18bb465d9f88ea12ef0768c178fe902ac923a` |
| `data/measurements.parquet` | 262 144 | `9976f7b7a7daf0bce8e04dd256e765d1d5e893f2eb60ed9470627b01b7449fa7` |
| `logs/system.log` | 65 536 | `e1f5617b13e8500293a27d9ac1fde94289b464bc8e2f5e8dcbc31db36861b26a` |
| `shared/common.txt` | 107 | `e1bbbbc9dc8b32d986d3f7b318e7d4ead364a3db7b5740a0a943e0734239564a` |

---

## How the corpus is designed to catch bugs

Nothing here is filler for its own sake. Each element exists to make a specific
class of failure visible.

| Element | The bug it catches |
| --- | --- |
| **Canary strings** — one per user, present in that user's plaintext and nowhere else | Encryption silently not happening. Tests scan a host's entire storage directory for another user's canary; a hit means plaintext reached disk on a machine that must never see it. |
| **`shared/common.txt`** — byte-identical across all three users | Cross-user deduplication or address collision. If any two users derive the same chunk id for these bytes, a host could prove they hold the same file. Blinding is what prevents it. |
| **`empty.txt`** — zero bytes | Off-by-one and empty-input handling in the chunker, the sealer, and the index. Empty files are a classic source of panics. |
| **`secrets/passwords.kdbx`** — 17 415 bytes, deliberately not a round number | Chunk-boundary arithmetic. Every other size being a clean multiple of 1 KiB would hide off-by-one errors at the final chunk. |
| **`music/track.flac`** — 1 MiB of high-entropy bytes | Multi-chunk paths, and incompressible data that cannot be accidentally "deduplicated" into nothing. |
| **`photos/holiday.jpg`** — 512 KiB high-entropy | A second multi-chunk file, so tests can distinguish per-file from per-chunk bugs. |
| **Text files with real structure** (Markdown, CSV, LaTeX, logs) | Content-defined chunking behaving differently on low-entropy text than on binary data. |
| **Three users, not two** | Two-party tests miss collusion. Three lets a test ask whether Bob *and* Carol together can read Alice's data. |

## Regenerating

```bash
cargo run -p itsanas-testkit --bin generate-fixtures
```

Output must match this document exactly. If it does not, either the key schedule
or the corpus changed — both are breaking changes, and this document plus the
pinned constants in `crates/itsanas-testkit/src/lib.rs` and
`crates/itsanas-crypto/src/wellknown.rs` must be updated together.
