//! A node's on-disk identity and state.
//!
//! ```text
//! <home>/
//!   keystore.bin   Argon2id-sealed master secret and device seed
//!   config         non-secret settings
//!   store/         this user's own data: chunks, index, log
//!   vault/         other users' sealed data, no keys anywhere near it
//!   status.snapshot  what the daemon last reported, for asking a busy node
//! ```
//!
//! # Why the device seed lives inside the keystore
//!
//! It could sit beside it in a mode-0600 file, and on Linux that would be
//! roughly fine. On Windows it would not: file permissions there are easy to
//! get wrong and easy to lose across a copy, a restore, or a sync tool. Sealing
//! it under the same passphrase costs one extra Argon2id run at startup —
//! already paid for the master secret — and removes an entire class of "the
//! secret was readable because a permission bit did not survive" bugs.

use std::fmt;
use std::path::{Path, PathBuf};

/// What the daemon leaves behind so a running node can still be asked.
///
/// The store allows one writer, and the daemon is it, so every command that
/// opens the store refuses while the node is up. The daemon writes this after
/// each round: not secret -- it is the same text `itsanas status` prints, which
/// is identifiers, paths and counts -- and not authoritative, which is why what
/// reads it says how old it is.
pub const SNAPSHOT: &str = "status.snapshot";

/// In a vault directory, the id of the device it belongs to: what tells a
/// vault from an empty directory left by a disk that is not mounted.
pub const VAULT_MARKER: &str = ".itsanas-vault";

use itsanas_crypto::{DeviceKeys, UserKeys};
use itsanas_store::{Store, Vault};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    error::{NodeError, Result},
};

mod identity;
mod lifecycle;
mod roster;
mod space;
mod vault;

pub use identity::{EncodedSecrets, Identity};
pub use space::SplitRefusal;
pub(crate) use space::{existing_ancestor, same_volume};

/// Label bound into the keystore's associated data.
///
/// Distinguishes the on-device keystore from the coordinator-hosted escrow blob,
/// so one can never be substituted for the other.
pub const KEYSTORE_LABEL: &str = "itsanas/keystore/local";

/// Label for the escrow copy a coordinator holds.
///
/// Deliberately different from [`KEYSTORE_LABEL`]. The two containers hold the
/// same secrets under different threat models — one on a disk the owner
/// controls, one on a machine that may be stolen — and a shared label would
/// mean a copy of either could be dropped in as the other.
pub const ESCROW_LABEL: &str = "itsanas/keystore/escrow";

/// The fewest characters a new passphrase may have, on every front-end.
///
/// Counted as characters, not bytes or UTF-16 units: six accented letters are
/// six on a phone, in a window and on a command line alike.
pub const MIN_PASSPHRASE_CHARS: usize = 6;

/// The most characters a new passphrase may have, on every front-end.
pub const MAX_PASSPHRASE_CHARS: usize = 128;

/// Refuse a passphrase being chosen that is outside the rule.
///
/// Asked of a passphrase being chosen, and nothing else: [`Node::open`],
/// [`Identity`], [`Node::restore_from_secrets`] and the keystore itself take
/// any length, because a member who chose a shorter one before the rule must
/// not be locked out of their own account, and a recovery container is sealed
/// under whatever the machine that lodged it had.
///
/// # Errors
///
/// A sentence with both limits and how many characters this one has.
pub fn check_new_passphrase(passphrase: &str) -> Result<()> {
    let count = passphrase.chars().count();
    if (MIN_PASSPHRASE_CHARS..=MAX_PASSPHRASE_CHARS).contains(&count) {
        return Ok(());
    }
    Err(NodeError::Usage(format!(
        "a passphrase is {MIN_PASSPHRASE_CHARS} to {MAX_PASSPHRASE_CHARS} characters; this one \
         has {count}. Nothing was changed."
    )))
}

/// The secrets a node needs to operate.
#[derive(Serialize, Deserialize)]
struct NodeSecrets {
    master: [u8; 32],
    device_seed: [u8; 32],
}

/// An opened node: identity, own store, and vault.
///
/// `Debug` is hand-written below rather than derived. See the impl for why.
pub struct Node {
    pub home: PathBuf,
    pub config: Config,
    pub store: Store,
    pub vault: Vault,
    /// This machine's signing key.
    ///
    /// Kept beside the store rather than fetched out of it: the store must
    /// never hand a key to a caller, and the transport needs one to prove which
    /// device it is.
    pub device: DeviceKeys,
    /// The account's own key schedule.
    ///
    /// Needed to sign a registration and a device enrolment, which are the two
    /// things a coordinator must not be able to forge. The store holds its own
    /// copy and will not hand it back — deliberately, since a store that could
    /// return a key would be one call away from leaking it.
    pub user: UserKeys,
    /// The secrets this machine holds, encoded exactly as the keystore has them.
    ///
    /// Kept so that an escrow copy can be sealed under a different label
    /// without deriving anything a second time. Zeroized with the node.
    pub secrets: zeroize::Zeroizing<Vec<u8>>,
    /// Opened while its configured vault's disk is not there
    /// ([`Node::open_away`]): `vault` is an empty stand-in in the home, and
    /// this machine hosts nothing for anyone until the disk is back.
    pub away: bool,
}

/// Where the stand-in vault of a node whose vault's disk is away lives,
/// under the home. Emptied at every such opening.
pub const AWAY_VAULT: &str = "vault-while-away";

/// Everything but the secrets, and the secrets as a length.
///
/// # Why this is written out rather than derived
///
/// `Node` used to `#[derive(Debug)]`, and `secrets` is a
/// `Zeroizing<Vec<u8>>` holding the *plaintext* encoding of the master secret
/// and the device seed. `Zeroizing` protects the memory's lifetime, not its
/// formatting: its own `Debug` forwards straight to `Vec<u8>`, which prints
/// every byte in decimal.
///
/// Nothing formatted a `Node` — this was a loaded gun rather than a shot
/// fired — but every other secret-bearing type in this workspace has a
/// hand-written redacting `Debug` for exactly this reason: `SecretBytes`,
/// `MasterSecret`, `UserKeys`, `DeviceKeys`, `Keystore`, and `Phrase` a
/// hundred lines below. `Store` and `Vault` can derive theirs only because
/// their key fields are those redacting types. `Node` was the one struct
/// holding raw key bytes and the one that derived.
///
/// One `tracing::debug!(?node)`, one `dbg!(&node)`, or one error type that
/// embeds a `Node` and derives, and the master secret — the value the
/// twenty-four words encode, from which every chunk key descends — lands in a
/// journal, a shipped log, or a support paste. That is the whole account: the
/// signing key, every chunk key past and future, the oplog root.
///
/// The comment two hundred lines below this one already named "a stray `dbg!`
/// or a struct derive that includes it" as the likeliest way this material
/// escapes. The derive was sitting above it.
impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("home", &self.home)
            .field("config", &self.config)
            .field("store", &self.store)
            .field("vault", &self.vault)
            .field("device", &self.device)
            .field("user", &self.user)
            .field("secrets_len", &self.secrets.len())
            .field("away", &self.away)
            .finish_non_exhaustive()
    }
}

/// Which "no node here" this is.
///
/// A node home on a disk that is not mounted is an **empty directory**, and the
/// ordinary message suggests `itsanas init` -- which would create a second
/// account on the root filesystem while the real one sits on a disk nobody is
/// looking at any more. The next backup then captures the empty one.
///
/// The distinction costs one `read_dir`: a home that exists and holds nothing
/// is a mount point far more often than it is a fresh start, because a fresh
/// start usually has no directory at all.
fn missing_node(home: &Path) -> NodeError {
    let empty = std::fs::read_dir(home).is_ok_and(|mut entries| entries.next().is_none());
    if empty {
        NodeError::NodeHomeEmpty(home.to_owned())
    } else {
        NodeError::NoNode(home.to_owned())
    }
}

impl Node {
    fn keystore_path(home: &Path) -> PathBuf {
        home.join("keystore.bin")
    }

    #[must_use]
    pub fn config_path(home: &Path) -> PathBuf {
        home.join("config")
    }

    /// Where this node keeps its store.
    ///
    /// Public because a caller that wants to know whether the daemon holds the
    /// store must find it *without* opening the node, which is the step that
    /// needs the passphrase.
    #[must_use]
    pub fn store_path(home: &Path) -> PathBuf {
        home.join("store")
    }

    /// Whether a node already exists at `home`: a keystore, or a link where
    /// it goes -- dangling or not, nothing is ever created through one.
    #[must_use]
    pub fn exists(home: &Path) -> bool {
        std::fs::symlink_metadata(Self::keystore_path(home))
            .is_ok_and(|meta| meta.is_file() || meta.file_type().is_symlink())
    }

    /// Persist the configuration.
    pub fn save_config(&self) -> Result<()> {
        self.config.save(&Self::config_path(&self.home))
    }
}

/// The 24 words out of whatever a person saved: one line, one word a line, or
/// the numbered grid `init` printed, copied off the screen. The numbers are
/// dropped; words are never guessed at, so a wrong word still fails the
/// checksum rather than restoring some other account.
#[must_use]
pub fn phrase_words(text: &str) -> String {
    text.split_whitespace()
        .filter(|token| {
            !token.strip_suffix('.').is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A recovery phrase that wipes itself when dropped.
pub mod zeroize_phrase {
    use std::fmt;

    /// Wraps the phrase so it is not accidentally logged or kept.
    ///
    /// `Debug` deliberately prints nothing useful: the single most likely way
    /// for a recovery phrase to escape is a stray `dbg!` or a struct derive
    /// that includes it in an error message.
    pub struct Phrase(pub zeroize::Zeroizing<String>);

    impl Phrase {
        #[must_use]
        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    impl fmt::Debug for Phrase {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Phrase(redacted)")
        }
    }
}

/// The sealed secrets' plaintext, pinned (HANDOVER §8 17): a change here is a
/// node that can no longer open its own account after an upgrade.
#[cfg(test)]
mod disk_bytes {
    use super::NodeSecrets;

    #[test]
    fn red_team_the_sealed_secrets_keep_their_bytes() {
        // Sabotage: swap `master` and `device_seed`.
        let bytes = postcard::to_stdvec(&NodeSecrets {
            master: [1; 32],
            device_seed: [2; 32],
        })
        .expect("encode");
        itsanas_testkit::check_pinned(
            &[("NodeSecrets", itsanas_testkit::fold(&bytes))],
            &[("NodeSecrets", 12_813_034_092_718_475_717)],
        );
    }

    /// The coordinator answers a name with no recovery container with a decoy
    /// it shapes from a length it cannot read off a real one. If what a node
    /// seals, or how, grows, the decoys keep the old size and every real
    /// container stands out by its length: a list of the accounts worth a
    /// guessing attack, to anybody who asks.
    #[test]
    fn the_coordinators_decoy_has_the_shape_of_a_real_recovery_container() {
        let secrets = postcard::to_stdvec(&NodeSecrets {
            master: [1; 32],
            device_seed: [2; 32],
        })
        .expect("encode");
        let real = crate::coordinator::recovery_container("a passphrase", &secrets).expect("seal");
        let decoy = itsanas_crypto::Keystore::decoy(
            &itsanas_crypto::SecretBytes::new([3; 32]),
            b"personne",
            itsanas_coord::directory::RECOVERY_PAYLOAD_LEN,
            itsanas_crypto::KdfParams::RECOMMENDED,
        )
        .to_bytes();
        assert_eq!(
            (decoy.len(), &decoy[..11]),
            (real.len(), &real[..11]),
            "a decoy and a real container differ in size or header: RECOVERY_PAYLOAD_LEN no longer matches what set_escrow seals"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) const PASSPHRASE: &str = "a genuinely long passphrase for the tests";

    #[test]
    fn red_team_printing_a_node_does_not_print_the_master_secret() {
        // `Node` derived `Debug`, and `secrets` is the plaintext encoding of
        // the master secret and the device seed. `Zeroizing` protects the
        // memory's lifetime, not its formatting -- its `Debug` forwards to
        // `Vec<u8>`, which prints every byte in decimal.
        //
        // Nothing formatted a `Node`, so this was never a leak. It was one
        // `tracing::debug!(?node)` away from being the whole account in a
        // journal: the signing key, every chunk key past and future, the value
        // the twenty-four words encode. Every other secret-bearing type in the
        // workspace has a redacting `Debug` for this reason; this was the one
        // that derived, and it derived directly above the comment naming "a
        // struct derive that includes it" as the way this material escapes.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (node, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();

        assert!(
            !node.secrets.is_empty(),
            "the node holds no secrets to leak"
        );
        let printed = format!("{node:?}");

        // A `Vec<u8>` prints as its bytes in decimal, comma-separated. Looking
        // for a run of eight rather than for one byte: single small numbers
        // occur in any output by accident, eight consecutive ones do not.
        let run: String = node
            .secrets
            .iter()
            .take(8)
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        assert!(
            !printed.contains(&run),
            "the encoded master secret is in the debug output: {printed}"
        );

        // Any window of eight, not only the first: a future field could carry
        // the same bytes from a different offset under a different name.
        for window in node.secrets.windows(8) {
            let run: String = window
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            assert!(!printed.contains(&run), "secret bytes reached the output");
        }

        // And what it must still say, so a rewrite that redacts by deleting
        // the whole impl has to be a deliberate act rather than a side effect.
        assert!(printed.contains("secrets_len"), "got: {printed}");
        assert!(
            printed.contains("nicolas"),
            "the useful part was redacted too: {printed}"
        );
    }

    #[test]
    fn the_phrase_does_not_leak_through_debug() {
        let dir = tempfile::tempdir().unwrap();
        let (_node, phrase) = Node::create(&dir.path().join("node"), PASSPHRASE, "n").unwrap();

        let rendered = format!("{phrase:?}");
        assert_eq!(rendered, "Phrase(redacted)");
        // Whole words: a substring check failed on ARM once a phrase began with
        // "act", which is inside "redacted" and leaks nothing.
        let words: Vec<&str> = phrase.as_str().split_whitespace().collect();
        assert!(
            !rendered
                .split(|c: char| !c.is_alphanumeric())
                .any(|token| words.contains(&token)),
            "a word of the recovery phrase is printed by Debug: {rendered}"
        );
    }
}
