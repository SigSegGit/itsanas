//! A node's keys and configuration read from its keystore without opening
//! its store, for a command that must run while the daemon holds it.

use std::fmt;
use std::path::Path;

use itsanas_crypto::{DeviceKeys, Keystore, MasterSecret, SecretBytes, UserKeys};

use super::{KEYSTORE_LABEL, Node, NodeSecrets, missing_node, zeroize_phrase};
use crate::{
    config::Config,
    error::{NodeError, Result},
};

/// A node's configuration and keys, without opening its store.
///
/// **Why this exists.** Only one process at a time may hold a node's store, and
/// the daemon holds it whenever the node is running. So every command that
/// opened a `Node` refused to run on a working machine -- including `doctor`,
/// which is the command somebody runs *because* something is wrong. Stopping
/// the daemon to ask it then changes the answer: a node that is not running is
/// not listening, so "can anybody reach me" comes back no, for a reason that
/// is the asking.
///
/// Everything about reaching the network -- who this device is, which
/// coordinator to dial, what address is announced -- lives in the keystore and
/// the config file. Neither is the store, and neither is locked.
impl std::fmt::Debug for Identity {
    /// Names the device and nothing else.
    ///
    /// Every secret-bearing type in this workspace has a hand-written Debug for
    /// the same reason: one `tracing::debug!(?identity)` would put an account's
    /// key material in a journal. `Node` derived `Debug` once and a red-team
    /// test caught it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("device", &self.device.device_id())
            .finish_non_exhaustive()
    }
}

pub struct Identity {
    /// The non-secret settings.
    pub config: Config,
    /// This machine's key.
    pub device: DeviceKeys,
    /// The account's keys.
    pub user: UserKeys,
}

impl Identity {
    /// Read the keystore and the config, and stop there.
    ///
    /// # Errors
    ///
    /// If there is no node, the passphrase is wrong, or the config is invalid.
    fn unlock(home: &Path, passphrase: &str) -> Result<NodeSecrets> {
        Ok(postcard::from_bytes(&Self::unlock_encoded(
            home, passphrase,
        )?)?)
    }

    /// The keystore's plaintext, the encoded [`NodeSecrets`], wiped when
    /// dropped.
    fn unlock_encoded(home: &Path, passphrase: &str) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let keystore_path = Node::keystore_path(home);
        let bytes = match std::fs::read(&keystore_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(missing_node(home));
            }
            Err(error) => {
                return Err(NodeError::Io {
                    path: keystore_path,
                    source: error,
                });
            }
        };

        let keystore = Keystore::from_bytes(&bytes)?;
        Ok(zeroize::Zeroizing::new(
            keystore
                .unlock(passphrase, KEYSTORE_LABEL)
                .map_err(|_| NodeError::Unlock)?,
        ))
    }

    /// The account's secrets, encoded as [`Node::restore_from_secrets`] reads
    /// them, from this machine's keystore and its passphrase.
    ///
    /// What `itsanas copy-to-key` seals again, under the key's own
    /// passphrase. Reads the keystore only, so it works while the daemon
    /// holds the store. The bytes carry this machine's device seed too; the
    /// restore keeps only the master and gives the key a device of its own.
    ///
    /// # Errors
    ///
    /// If there is no node or the passphrase is wrong.
    pub fn secrets(home: &Path, passphrase: &str) -> Result<EncodedSecrets> {
        Self::unlock_encoded(home, passphrase).map(EncodedSecrets)
    }

    /// The account's 24 recovery words again, from this machine's keystore.
    ///
    /// The keystore holds the master secret, and the words are that secret in
    /// another encoding, so someone who lost the paper but still has a machine
    /// and its passphrase has not lost the account. Reads the keystore only:
    /// it works while the daemon holds the store.
    ///
    /// # Errors
    ///
    /// If there is no node or the passphrase is wrong.
    pub fn recovery_phrase(home: &Path, passphrase: &str) -> Result<zeroize_phrase::Phrase> {
        let secrets = Self::unlock(home, passphrase)?;
        let phrase = MasterSecret::from_bytes(secrets.master).to_recovery_phrase()?;
        Ok(zeroize_phrase::Phrase(phrase))
    }

    pub fn open(home: &Path, passphrase: &str) -> Result<Self> {
        let secrets = Self::unlock(home, passphrase)?;
        let master = MasterSecret::from_bytes(secrets.master);

        Ok(Self {
            config: Config::load(&Node::config_path(home))?,
            device: DeviceKeys::from_seed(&SecretBytes::new(secrets.device_seed)),
            user: UserKeys::derive(&master),
        })
    }
}

/// A node's encoded secrets ([`Identity::secrets`]), wiped when dropped and
/// never printed.
pub struct EncodedSecrets(zeroize::Zeroizing<Vec<u8>>);

impl EncodedSecrets {
    /// The bytes, for [`Node::restore_from_secrets`].
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for EncodedSecrets {
    /// Says what it is and nothing of it: a derived `Debug` (or
    /// `Zeroizing`'s) would print the master secret byte by byte.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncodedSecrets(redacted)")
    }
}

#[cfg(test)]
mod tests {
    use itsanas_store::Store;

    use super::*;

    /// THE COMMAND NOBODY COULD RUN: only one process may hold a node's store,
    /// and the daemon holds it on every machine that is working. Anything that
    /// opened a `Node` therefore refused to run on exactly the machines
    /// somebody asks about -- including `doctor`, which is what a person runs
    /// *because* something is wrong. Stopping the daemon to ask then changes
    /// the answer, because a node that is not running is not listening.
    ///
    /// The keys and the config are not the store, and nothing locks them.
    #[test]
    fn the_keys_can_be_read_while_the_store_is_held_by_another_process() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");

        let (node, _phrase) = Node::create(&home, "a passphrase", "nicolas").expect("create");
        let device = node.device.device_id();
        // `node` is still alive, so the store is locked exactly as it is when
        // the daemon is running.
        assert!(
            Store::is_locked(Node::store_path(&home)),
            "the fixture must hold the lock"
        );

        let identity = Identity::open(&home, "a passphrase")
            .expect("the keys must be readable while the node is running");

        assert_eq!(identity.device.device_id(), device);
        assert_eq!(identity.user.user_id(), node.store.owner());
        assert_eq!(identity.config.username, "nicolas");

        // And a wrong passphrase still gets nothing.
        assert!(Identity::open(&home, "not the passphrase").is_err());
    }

    /// THE LOST SHEET (hand test, 2026-10-22): the 24 words of the test
    /// account were cut to the clipboard and lost, while the laptop still had
    /// the account and its passphrase. The keystore holds the master secret,
    /// so the words come back -- the same words, while the daemon runs, and
    /// not for a wrong passphrase.
    #[test]
    fn red_team_the_24_words_come_back_from_the_keystore_and_its_passphrase_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let (_node, phrase) = Node::create(&home, "a passphrase", "test").expect("create");

        let again = Identity::recovery_phrase(&home, "a passphrase")
            .expect("the words must come back while the node is running");
        assert_eq!(again.as_str(), phrase.as_str());
        assert_eq!(again.as_str().split_whitespace().count(), 24);

        assert!(Identity::recovery_phrase(&home, "not the passphrase").is_err());
    }

    /// `copy-to-key` reads the secrets while the daemon runs and seals them
    /// again elsewhere: the same account, a new device, and the bytes never
    /// printed by a stray `{:?}`.
    #[test]
    fn the_encoded_secrets_restore_the_account_and_never_print() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let (node, _) = Node::create(&home, "a passphrase", "test").expect("create");

        assert!(Identity::secrets(&home, "not the passphrase").is_err());
        let secrets = Identity::secrets(&home, "a passphrase").expect("read while running");
        assert_eq!(format!("{secrets:?}"), "EncodedSecrets(redacted)");

        let copy = Node::restore_from_secrets(
            &dir.path().join("key"),
            "another passphrase",
            "test",
            secrets.as_bytes(),
        )
        .expect("restore");
        assert_eq!(copy.store.owner(), node.store.owner());
        assert_ne!(copy.store.device_id(), node.store.device_id());
    }
}
