//! Signed releases: what a tester downloads instead of compiling, and how a
//! node will know the download is Nicolas's.
//!
//! A release is a set of binaries, one per target, plus a **manifest**: a few
//! lines of text naming the version and, for each binary, its size, BLAKE3 and
//! SHA-256. The manifest is signed with Ed25519 by the **release key**, which
//! lives on Nicolas's PC, sealed under a passphrase, with an offline copy. CI
//! builds and writes the manifest; it never sees the key (decided 2026-10-06:
//! a key in GitHub Actions would let whoever takes the GitHub account own every
//! member's machine).
//!
//! Verification is in a fixed order, and the order is the point:
//!
//! 1. the signature, over the **exact** manifest bytes, against the pinned key
//!    -- nothing in an unsigned manifest is even parsed;
//! 2. the manifest's format;
//! 3. its version, which must be newer than the running one (no downgrade: an
//!    old, correctly signed release with a known bug must not be replayable);
//! 4. each downloaded file's size, BLAKE3 and SHA-256.
//!
//! The format is plain text with no dependency because it has to be read by a
//! person checking a release by eye, and hashed byte for byte by a shell script.
//!
//! ```text
//! itsanas-release 1
//! version 0.2.0
//! file x86_64-unknown-linux-gnu itsanas-x86_64-unknown-linux-gnu 1234 <blake3> <sha256>
//! next-key <hex>            (optional: the key that signs the releases after this one)
//! ```
//!
//! Every error is one plain line saying what to do, because the person reading
//! it is a tester, not a cryptographer.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use itsanas_crypto::{KdfParams, Keystore};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

/// The public half of Nicolas's release key, generated 2026-10-07 by
/// `itsanas-release keygen`:
/// `5b951b5d5df151b4598b11ce2a7ec6599710fbd61897aff013a1645b0bc5101b`.
///
/// Every node built from here on installs only what this key signed. A build
/// with `None` (every build before this line) refuses every verification,
/// which is the safe failure: it can be installed by hand but can never be told
/// by a download that it is official. A test pins this value against the hex
/// above, so changing it is a visible decision in a diff rather than a line
/// nobody reads.
pub const RELEASE_KEY: Option<[u8; 32]> = Some([
    0x5b, 0x95, 0x1b, 0x5d, 0x5d, 0xf1, 0x51, 0xb4, 0x59, 0x8b, 0x11, 0xce, 0x2a, 0x7e, 0xc6, 0x59,
    0x97, 0x10, 0xfb, 0xd6, 0x18, 0x97, 0xaf, 0xf0, 0x13, 0xa1, 0x64, 0x5b, 0x0b, 0xc5, 0x10, 0x1b,
]);

/// The five targets a release is built for, in manifest order.
pub const TARGETS: [&str; 5] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
];

/// First line of every manifest. A format change bumps the number, so an old
/// node refuses a manifest it would misread instead of guessing.
const HEADER: &str = "itsanas-release 1";

/// Bound into the sealed key file's associated data, so a node's keystore
/// cannot be passed off as the release key file, or the other way round.
const KEY_LABEL: &str = "itsanas/release-signing-key";

/// Every way a release can be refused, each one plain line for a person.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseError {
    #[error(
        "this build has no release key pinned yet, so it cannot trust any download: install releases by hand"
    )]
    NoKeyPinned,

    #[error("this release is not signed by the ITSaNAS release key: do not install it")]
    NotSigned,

    #[error("the release manifest is damaged ({0}): download it again")]
    Damaged(String),

    #[error("release {offered} is not newer than the running {running}: nothing to install")]
    NotNewer { offered: Version, running: Version },

    #[error(
        "the download of {name} is incomplete or too long ({found} bytes, the release says {expected}): download it again"
    )]
    WrongSize {
        name: String,
        expected: u64,
        found: u64,
    },

    #[error(
        "the download of {name} is not the file that was signed: delete it and download it again"
    )]
    WrongContent { name: String },

    #[error("wrong passphrase, or the key file is damaged: try again, or use your offline copy")]
    WrongPassphrase,

    #[error("{0}")]
    Io(String),
}

pub type Result<T> = core::result::Result<T, ReleaseError>;

/// A release version, compared as numbers: 0.10.0 is newer than 0.9.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// Parse `MAJOR.MINOR.PATCH`, digits only. Strict on purpose: a version a
    /// node half-understands is a version it might wrongly call newer.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || ReleaseError::Damaged(format!("'{text}' is not a version like 0.2.0"));
        let mut parts = text.split('.');
        let mut next = || -> Result<u32> {
            let part = parts.next().ok_or_else(bad)?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            part.parse().map_err(|_| bad())
        };
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(version)
    }

    /// The version of the binary that is running, which is the workspace's.
    #[must_use]
    pub fn running() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is MAJOR.MINOR.PATCH")
    }
}

impl core::fmt::Display for Version {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One binary of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub target: String,
    pub name: String,
    pub size: u64,
    pub blake3: [u8; 32],
    pub sha256: [u8; 32],
}

impl FileEntry {
    /// Measure a file on disk: what CI does when it writes the manifest.
    pub fn measure(target: &str, path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ReleaseError::Io(format!("{} has no usable file name", path.display())))?
            .to_owned();
        let (size, blake3, sha256) = digest_file(path)?;
        Ok(Self {
            target: target.to_owned(),
            name,
            size,
            blake3,
            sha256,
        })
    }

    /// Check a downloaded file against this entry: size first, so a truncated
    /// download is reported as incomplete rather than as a forgery, then both
    /// hashes. Both, not either: the bootstrap installers can only check
    /// SHA-256, and a node checks BLAKE3; a file must satisfy the two.
    pub fn check_file(&self, path: &Path) -> Result<()> {
        let (size, blake3, sha256) = digest_file(path)?;
        if size != self.size {
            return Err(ReleaseError::WrongSize {
                name: self.name.clone(),
                expected: self.size,
                found: size,
            });
        }
        if blake3 != self.blake3 {
            return Err(ReleaseError::WrongContent {
                name: self.name.clone(),
            });
        }
        if sha256 != self.sha256 {
            return Err(ReleaseError::WrongContent {
                name: self.name.clone(),
            });
        }
        Ok(())
    }
}

/// Size, BLAKE3 and SHA-256 of a file, read once in blocks so a large binary
/// is never held in memory.
fn digest_file(path: &Path) -> Result<(u64, [u8; 32], [u8; 32])> {
    let io =
        |e: std::io::Error| ReleaseError::Io(format!("could not read {}: {e}", path.display()));
    let mut file = File::open(path).map_err(io)?;
    let mut blake = blake3::Hasher::new();
    let mut sha = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        blake.update(&buf[..n]);
        sha.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, *blake.finalize().as_bytes(), sha.finalize().into()))
}

/// A release manifest, parsed only after its signature was checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub files: Vec<FileEntry>,
    pub next_key: Option<[u8; 32]>,
}

impl Manifest {
    /// Build the manifest for the binaries in `dir`, named
    /// `itsanas-<target>[.exe]` as the release workflow names them.
    pub fn from_dir(version: Version, dir: &Path) -> Result<Self> {
        let mut files = Vec::new();
        for target in TARGETS {
            for name in [format!("itsanas-{target}"), format!("itsanas-{target}.exe")] {
                let path = dir.join(&name);
                if path.is_file() {
                    files.push(FileEntry::measure(target, &path)?);
                    break;
                }
            }
        }
        if files.is_empty() {
            return Err(ReleaseError::Io(format!(
                "no file named itsanas-<target> in {}: nothing to put in a release",
                dir.display()
            )));
        }
        Ok(Self {
            version,
            files,
            next_key: None,
        })
    }

    /// The binary for `target`, if this release has one.
    #[must_use]
    pub fn file_for(&self, target: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.target == target)
    }

    /// The exact text that gets signed.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut lines = vec![HEADER.to_owned(), format!("version {}", self.version)];
        for f in &self.files {
            lines.push(format!(
                "file {} {} {} {} {}",
                f.target,
                f.name,
                f.size,
                hex::encode(f.blake3),
                hex::encode(f.sha256)
            ));
        }
        if let Some(key) = self.next_key {
            lines.push(format!("next-key {}", hex::encode(key)));
        }
        lines.push(String::new());
        lines.join("\n")
    }

    /// Parse a manifest. Strict: an unknown line is damage, not a field to
    /// skip, because a field skipped by an old node is a promise it ignores.
    pub fn parse(text: &str) -> Result<Self> {
        let damaged = |line: usize, why: &str| ReleaseError::Damaged(format!("line {line}: {why}"));
        let body = text.strip_suffix('\n').unwrap_or(text);
        let mut lines = body.split('\n');
        if lines.next() != Some(HEADER) {
            return Err(damaged(
                1,
                "not an ITSaNAS release manifest, or a newer format",
            ));
        }
        let version = lines
            .next()
            .and_then(|l| l.strip_prefix("version "))
            .ok_or_else(|| damaged(2, "no version"))?;
        let version = Version::parse(version)?;
        let mut files: Vec<FileEntry> = Vec::new();
        let mut next_key = None;
        for (index, line) in lines.enumerate() {
            let number = index + 3;
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["file", target, name, size, blake, sha] => {
                    let entry = parse_file_line(target, name, size, blake, sha)
                        .map_err(|why| damaged(number, why))?;
                    if files.iter().any(|f| f.target == entry.target) {
                        return Err(damaged(number, "the same target twice"));
                    }
                    files.push(entry);
                }
                ["next-key", key] if next_key.is_none() => {
                    next_key = Some(hex32(key).ok_or_else(|| damaged(number, "bad next key"))?);
                }
                _ => return Err(damaged(number, "unexpected line")),
            }
        }
        if files.is_empty() {
            return Err(damaged(3, "no files"));
        }
        Ok(Self {
            version,
            files,
            next_key,
        })
    }
}

/// What the signing step checks before Nicolas's key touches a draft: the
/// manifest in `dir` names the version of `tag`, and every binary it lists is
/// in `dir` with exactly the signed size and hashes. Returns the manifest so
/// the caller can print each SHA-256.
///
/// Why: a draft release is writable by anyone with write access to the
/// repository or by a compromised action in the job that creates it. Signing
/// its manifest without checking the binaries would put the genuine key on an
/// attacker's hashes, and every install and self-update would then accept
/// them. Checking here makes a tampered draft a draft nobody signs.
pub fn check_draft(dir: &Path, tag: &str) -> Result<Manifest> {
    let path = dir.join("manifest.txt");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| ReleaseError::Io(format!("could not read {}: {e}", path.display())))?;
    let manifest = Manifest::parse(&text)?;
    let tagged = Version::parse(tag.strip_prefix('v').unwrap_or(tag))?;
    if manifest.version != tagged {
        return Err(ReleaseError::Io(format!(
            "the draft {tag} carries a manifest for {}: it was not made by the release workflow for this tag, do not sign it",
            manifest.version
        )));
    }
    for file in &manifest.files {
        file.check_file(&dir.join(&file.name))?;
    }
    Ok(manifest)
}

fn parse_file_line(
    target: &str,
    name: &str,
    size: &str,
    blake: &str,
    sha: &str,
) -> core::result::Result<FileEntry, &'static str> {
    let target_ok = !target.is_empty()
        && target
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !target_ok {
        return Err("bad target");
    }
    // The name is derived from the target, never free text: an installer saves
    // the download under this name, and a name like `../../.profile` would let
    // a manifest choose where on a tester's disk a file lands.
    let plain = format!("itsanas-{target}");
    if name != plain && name != format!("{plain}.exe") {
        return Err("a file name that does not match its target");
    }
    if size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
        return Err("bad size");
    }
    Ok(FileEntry {
        target: target.to_owned(),
        name: name.to_owned(),
        size: size.parse().map_err(|_| "bad size")?,
        blake3: hex32(blake).ok_or("bad BLAKE3")?,
        sha256: hex32(sha).ok_or("bad SHA-256")?,
    })
}

/// Lowercase hex of exactly 32 bytes, nothing else.
fn hex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || text.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(text, &mut out).ok()?;
    Some(out)
}

/// The keys a node accepts a release from: the pinned one, plus any a signed
/// manifest named as the next.
#[derive(Debug, Clone)]
pub struct Trust {
    keys: Vec<VerifyingKey>,
}

impl Trust {
    /// The key compiled into this build, [`RELEASE_KEY`].
    pub fn pinned() -> Result<Self> {
        Self::from_pinned(RELEASE_KEY)
    }

    /// Trust built from what a build has pinned. `None` refuses outright, with
    /// its own message, rather than producing an empty set that would fail
    /// later as "not signed" and send a tester hunting for a forgery.
    pub fn from_pinned(pinned: Option<[u8; 32]>) -> Result<Self> {
        match pinned {
            Some(key) => Ok(Self {
                keys: vec![verifying_key(&key)?],
            }),
            None => Err(ReleaseError::NoKeyPinned),
        }
    }

    /// Trust one public key given as hex, for `itsanas-release verify --key`.
    pub fn from_hex(text: &str) -> Result<Self> {
        let key = hex32(text.trim()).ok_or_else(|| {
            ReleaseError::Io("--key wants the 64-character public key keygen printed".into())
        })?;
        Self::from_pinned(Some(key))
    }

    /// Accept the key a verified manifest names as the next one, for this
    /// `Trust` value only. This is the building block of key rotation, not
    /// rotation itself: nothing calls it outside tests, nothing persists a
    /// learned key, and it never drops the old key, so rotation is not built.
    pub fn learn(&mut self, manifest: &Manifest) -> Result<()> {
        if let Some(next) = manifest.next_key {
            let key = verifying_key(&next)?;
            if !self.keys.contains(&key) {
                self.keys.push(key);
            }
        }
        Ok(())
    }
}

fn verifying_key(bytes: &[u8; 32]) -> Result<VerifyingKey> {
    VerifyingKey::from_bytes(bytes)
        .map_err(|_| ReleaseError::Damaged("a release key that is not a valid public key".into()))
}

/// Steps 1 and 2: the signature over the exact bytes, then the format.
pub fn verify_signed(manifest: &[u8], signature: &str, trust: &Trust) -> Result<Manifest> {
    let mut raw = [0u8; 64];
    hex::decode_to_slice(signature.trim(), &mut raw).map_err(|_| ReleaseError::NotSigned)?;
    let signature = Signature::from_bytes(&raw);
    if !trust
        .keys
        .iter()
        .any(|key| key.verify_strict(manifest, &signature).is_ok())
    {
        return Err(ReleaseError::NotSigned);
    }
    let text =
        core::str::from_utf8(manifest).map_err(|_| ReleaseError::Damaged("not text".into()))?;
    Manifest::parse(text)
}

/// Steps 1 to 3: signed, well formed, and newer than `running`.
pub fn verify_release(
    manifest: &[u8],
    signature: &str,
    trust: &Trust,
    running: Version,
) -> Result<Manifest> {
    let parsed = verify_signed(manifest, signature, trust)?;
    if parsed.version <= running {
        return Err(ReleaseError::NotNewer {
            offered: parsed.version,
            running,
        });
    }
    Ok(parsed)
}

/// The secret half of a release key. Only ever on Nicolas's PC.
pub struct ReleaseKey {
    signing: SigningKey,
}

impl core::fmt::Debug for ReleaseKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReleaseKey")
            .field("public", &self.public_hex())
            .finish_non_exhaustive()
    }
}

impl ReleaseKey {
    /// A fresh key from the operating system's randomness.
    pub fn generate() -> Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut())
            .map_err(|e| ReleaseError::Io(format!("the system gave no randomness: {e}")))?;
        Ok(Self {
            signing: SigningKey::from_bytes(&seed),
        })
    }

    #[must_use]
    pub fn public_bytes(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    #[must_use]
    pub fn public_hex(&self) -> String {
        hex::encode(self.public_bytes())
    }

    /// Seal under a passphrase with the node keystore's own scheme (Argon2id
    /// then its AEAD), at production cost: there is no cheaper setting to
    /// reach for by mistake.
    pub fn seal(&self, passphrase: &str) -> Result<Vec<u8>> {
        let seed = Zeroizing::new(self.signing.to_bytes());
        Keystore::lock(passphrase, KEY_LABEL, seed.as_ref(), KdfParams::RECOMMENDED)
            .map(|k| k.to_bytes())
            .map_err(|_| ReleaseError::Io("could not seal the key".into()))
    }

    /// Open a sealed key file. A wrong passphrase and a damaged file are one
    /// error, because the AEAD cannot tell them apart and guessing would lie.
    pub fn unseal(sealed: &[u8], passphrase: &str) -> Result<Self> {
        let keystore = Keystore::from_bytes(sealed).map_err(|_| ReleaseError::WrongPassphrase)?;
        let seed = Zeroizing::new(
            keystore
                .unlock(passphrase, KEY_LABEL)
                .map_err(|_| ReleaseError::WrongPassphrase)?,
        );
        let seed: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| ReleaseError::WrongPassphrase)?;
        let seed = Zeroizing::new(seed);
        Ok(Self {
            signing: SigningKey::from_bytes(&seed),
        })
    }

    /// Sign a manifest, after checking it parses: the key never signs bytes
    /// that a node would then refuse as damaged, or that are not a manifest.
    /// Returns the parsed manifest, for saying what was signed, and the
    /// signature as one line of hex.
    pub fn sign_manifest(&self, manifest: &[u8]) -> Result<(Manifest, String)> {
        let text =
            core::str::from_utf8(manifest).map_err(|_| ReleaseError::Damaged("not text".into()))?;
        let parsed = Manifest::parse(text)?;
        let signature = self.signing.sign(manifest);
        Ok((parsed, format!("{}\n", hex::encode(signature.to_bytes()))))
    }
}
