//! A node's on-disk identity and state.
//!
//! ```text
//! <home>/
//!   keystore.bin   Argon2id-sealed master secret and device seed
//!   config         non-secret settings
//!   store/         this user's own data: chunks, index, log
//!   vault/         other users' sealed data, no keys anywhere near it
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

use std::path::{Path, PathBuf};

use itsanas_crypto::{
    DeviceKeys, KdfParams, Keystore, MasterSecret, SecretBytes, UserKeys,
    is_published_test_identity,
};
use itsanas_store::{Store, Vault};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    error::{CliError, Result},
};

/// Label bound into the keystore's associated data.
///
/// Distinguishes the on-device keystore from the coordinator-hosted escrow blob,
/// so one can never be substituted for the other.
pub(crate) const KEYSTORE_LABEL: &str = "itsanas/keystore/local";

/// Label for the escrow copy a coordinator holds.
///
/// Deliberately different from [`KEYSTORE_LABEL`]. The two containers hold the
/// same secrets under different threat models — one on a disk the owner
/// controls, one on a machine that may be stolen — and a shared label would
/// mean a copy of either could be dropped in as the other.
pub(crate) const ESCROW_LABEL: &str = "itsanas/keystore/escrow";

/// The secrets a node needs to operate.
#[derive(Serialize, Deserialize)]
struct NodeSecrets {
    master: [u8; 32],
    device_seed: [u8; 32],
}

/// An opened node: identity, own store, and vault.
#[derive(Debug)]
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
}

impl Node {
    fn keystore_path(home: &Path) -> PathBuf {
        home.join("keystore.bin")
    }

    pub fn config_path(home: &Path) -> PathBuf {
        home.join("config")
    }

    /// Whether a node already exists at `home`.
    #[must_use]
    pub fn exists(home: &Path) -> bool {
        Self::keystore_path(home).is_file()
    }

    /// Create a node from a fresh identity.
    ///
    /// Returns the recovery phrase, which the caller must show the user exactly
    /// once. It is not stored anywhere: a phrase kept on the machine it
    /// protects is not a backup.
    pub fn create(
        home: &Path,
        passphrase: &str,
        username: &str,
    ) -> Result<(Self, zeroize_phrase::Phrase)> {
        if Self::exists(home) {
            return Err(CliError::NodeExists(home.to_owned()));
        }

        let master = MasterSecret::generate()?;
        let phrase = master.to_recovery_phrase()?;
        let node = Self::write_new(home, passphrase, username, &master)?;

        Ok((node, zeroize_phrase::Phrase(phrase)))
    }

    /// Create a node by restoring an identity from its recovery phrase.
    pub fn restore(home: &Path, passphrase: &str, username: &str, phrase: &str) -> Result<Self> {
        if Self::exists(home) {
            return Err(CliError::NodeExists(home.to_owned()));
        }

        let master = MasterSecret::from_recovery_phrase(phrase)?;
        Self::write_new(home, passphrase, username, &master)
    }

    /// Create a node from the secrets held in a recovery container.
    ///
    /// The container carries the account identity; the device key is generated
    /// fresh, because this is a different machine and a device key identifies a
    /// machine rather than a person. Losing the old laptop then revokes one
    /// enrolment rather than rotating the whole identity.
    pub fn restore_from_secrets(
        home: &Path,
        passphrase: &str,
        username: &str,
        secrets: &[u8],
    ) -> Result<Self> {
        if Self::exists(home) {
            return Err(CliError::NodeExists(home.to_owned()));
        }

        let recovered: NodeSecrets = postcard::from_bytes(secrets)?;
        let master = MasterSecret::from_bytes(recovered.master);
        Self::write_new(home, passphrase, username, &master)
    }

    fn write_new(
        home: &Path,
        passphrase: &str,
        username: &str,
        master: &MasterSecret,
    ) -> Result<Self> {
        // A device key per machine, generated locally and never derived from the
        // master secret, so losing this laptop revokes one certificate rather
        // than forcing the user to rotate their whole identity.
        let device = DeviceKeys::generate()?;

        let secrets = NodeSecrets {
            master: *master.expose(),
            device_seed: *device.seed().expose(),
        };
        let encoded = postcard::to_stdvec(&secrets)?;

        // The escrow copy of this container is held by an untrusted
        // coordinator, so the cost has to be the production one even though it
        // makes startup slower.
        debug_assert!(KdfParams::RECOMMENDED.meets_production_floor());
        let keystore =
            Keystore::lock(passphrase, KEYSTORE_LABEL, &encoded, KdfParams::RECOMMENDED)?;

        std::fs::create_dir_all(home).map_err(|error| CliError::Io {
            path: home.to_owned(),
            source: error,
        })?;

        let keystore_path = Self::keystore_path(home);
        std::fs::write(&keystore_path, keystore.to_bytes()).map_err(|error| CliError::Io {
            path: keystore_path,
            source: error,
        })?;

        let config = Config {
            username: username.to_owned(),
            ..Config::default()
        };
        config.save(&Self::config_path(home))?;

        Self::assemble(home, config, master, &device, encoded)
    }

    /// Open an existing node.
    pub fn open(home: &Path, passphrase: &str) -> Result<Self> {
        let keystore_path = Self::keystore_path(home);
        let bytes = match std::fs::read(&keystore_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CliError::NoNode(home.to_owned()));
            }
            Err(error) => {
                return Err(CliError::Io {
                    path: keystore_path,
                    source: error,
                });
            }
        };

        let keystore = Keystore::from_bytes(&bytes)?;
        let plaintext = keystore
            .unlock(passphrase, KEYSTORE_LABEL)
            .map_err(|_| CliError::Unlock)?;

        let secrets: NodeSecrets = postcard::from_bytes(&plaintext)?;
        let master = MasterSecret::from_bytes(secrets.master);
        let device = DeviceKeys::from_seed(&SecretBytes::new(secrets.device_seed));

        let config = Config::load(&Self::config_path(home))?;
        Self::assemble(home, config, &master, &device, plaintext)
    }

    fn assemble(
        home: &Path,
        config: Config,
        master: &MasterSecret,
        device: &DeviceKeys,
        secrets: Vec<u8>,
    ) -> Result<Self> {
        let user = UserKeys::derive(master);

        // Belt and braces: `Store::open` performs this check too, but failing
        // here produces a message about the *account* rather than about a
        // storage path, which is what the person reading it needs.
        if is_published_test_identity(&user.user_id()) {
            return Err(CliError::Usage(
                "this recovery phrase belongs to one of the published test \
                 identities in docs/TEST-USERS.md. Its private keys are printed \
                 in the documentation, so anyone at all can read data stored \
                 under it. Refusing to open it as a real account."
                    .to_owned(),
            ));
        }

        let store = Store::open(
            home.join("store"),
            user,
            DeviceKeys::from_seed(&device.seed()),
        )?;
        let vault = Vault::open(home.join("vault"))?;

        Ok(Self {
            home: home.to_owned(),
            config,
            store,
            vault,
            device: DeviceKeys::from_seed(&device.seed()),
            user: UserKeys::derive(master),
            secrets: zeroize::Zeroizing::new(secrets),
        })
    }

    /// Persist the configuration.
    pub fn save_config(&self) -> Result<()> {
        self.config.save(&Self::config_path(&self.home))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const PASSPHRASE: &str = "a genuinely long passphrase for the tests";

    /// Whether `needle` appears in any file under `directory`.
    fn scan(directory: &Path, needle: &str) -> bool {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if scan(&path, needle) {
                    return true;
                }
            } else if let Ok(bytes) = std::fs::read(&path)
                && bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn a_created_node_reopens_with_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");

        let owner = {
            let (node, phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
            assert_eq!(
                phrase.as_str().split_whitespace().count(),
                24,
                "a recovery phrase must be 24 words"
            );
            node.store.owner()
        };

        let reopened = Node::open(&home, PASSPHRASE).unwrap();
        assert_eq!(
            reopened.store.owner(),
            owner,
            "reopening produced a different identity, so the data is orphaned"
        );
        assert_eq!(reopened.config.username, "nicolas");
    }

    #[test]
    fn the_device_identity_also_survives_a_restart() {
        // If the device key changed on every start, every restart would look
        // like a brand-new device to the version vectors and history would
        // fragment.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");

        let device = Node::create(&home, PASSPHRASE, "nicolas")
            .unwrap()
            .0
            .store
            .device_id();

        assert_eq!(
            Node::open(&home, PASSPHRASE).unwrap().store.device_id(),
            device,
            "the device identity changed across a restart"
        );
    }

    #[test]
    fn the_wrong_passphrase_does_not_open_the_node() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        Node::create(&home, PASSPHRASE, "nicolas").unwrap();

        assert!(matches!(
            Node::open(&home, "not the passphrase"),
            Err(CliError::Unlock)
        ));
    }

    #[test]
    fn creating_over_an_existing_node_is_refused() {
        // Overwriting would destroy the master secret and make every chunk
        // stored under it permanently unreadable.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        Node::create(&home, PASSPHRASE, "nicolas").unwrap();

        assert!(matches!(
            Node::create(&home, PASSPHRASE, "someone-else"),
            Err(CliError::NodeExists(_))
        ));
        assert!(matches!(
            Node::restore(&home, PASSPHRASE, "nicolas", "irrelevant"),
            Err(CliError::NodeExists(_))
        ));
    }

    #[test]
    fn opening_a_missing_node_says_what_to_do_about_it() {
        let dir = tempfile::tempdir().unwrap();
        let error = Node::open(&dir.path().join("nothing-here"), PASSPHRASE).unwrap_err();

        let message = error.to_string();
        assert!(matches!(error, CliError::NoNode(_)));
        assert!(
            message.contains("itsanas init") && message.contains("itsanas login"),
            "the error does not tell the user what to do: {message}"
        );
    }

    #[test]
    fn a_phrase_round_trips_through_restore() {
        let dir = tempfile::tempdir().unwrap();
        let first_home = dir.path().join("first");
        let second_home = dir.path().join("second");

        let (first, phrase) = Node::create(&first_home, PASSPHRASE, "nicolas").unwrap();
        let owner = first.store.owner();

        let restored = Node::restore(
            &second_home,
            "a different passphrase",
            "nicolas",
            phrase.as_str(),
        )
        .unwrap();

        assert_eq!(
            restored.store.owner(),
            owner,
            "restoring from the phrase produced a different account"
        );
        assert_ne!(
            restored.store.device_id(),
            first.store.device_id(),
            "a restored node reused the original device identity; two machines \
             would then share a sequence counter and fork the log"
        );
    }

    #[test]
    fn a_published_test_phrase_is_refused_as_a_real_account() {
        let dir = tempfile::tempdir().unwrap();
        let alice = itsanas_testkit_phrase();

        let error = Node::restore(&dir.path().join("node"), PASSPHRASE, "alice", &alice)
            .expect_err("a published test identity must not open as a real account");

        assert!(
            error.to_string().contains("published test"),
            "the refusal does not explain itself: {error}"
        );
    }

    /// Alice's phrase, derived the same way `itsanas-testkit` does.
    ///
    /// Duplicated rather than depending on the testkit, so this crate's
    /// production dependency list stays free of the fixture users entirely.
    fn itsanas_testkit_phrase() -> String {
        let master = MasterSecret::from_bytes(blake3::derive_key(
            "itsanas test fixture entropy - NOT SECRET",
            b"alice",
        ));
        master.to_recovery_phrase().unwrap().as_str().to_owned()
    }

    #[test]
    fn the_phrase_is_not_written_anywhere_under_the_node_directory() {
        // A recovery phrase stored on the machine it protects is not a backup,
        // and is an extra copy for an attacker to find.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (_node, phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();

        let first_word = phrase
            .as_str()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        let needle = format!("{first_word} ");

        assert!(
            !scan(&home, &needle),
            "the recovery phrase appears in plaintext under the node directory"
        );
    }

    #[test]
    fn the_phrase_does_not_leak_through_debug() {
        let dir = tempfile::tempdir().unwrap();
        let (_node, phrase) = Node::create(&dir.path().join("node"), PASSPHRASE, "n").unwrap();

        let rendered = format!("{phrase:?}");
        assert_eq!(rendered, "Phrase(redacted)");
        assert!(!rendered.contains(phrase.as_str().split_whitespace().next().unwrap()));
    }
}
