//! Creating, restoring and opening a node from its keystore, and sealing
//! that keystore again under a new passphrase.

use std::path::Path;

use itsanas_crypto::{
    DeviceKeys, KdfParams, Keystore, MasterSecret, SecretBytes, UserKeys,
    is_published_test_identity,
};
use itsanas_store::{FolderCopy, Store, WriteBudget};

use super::{
    KEYSTORE_LABEL, Node, NodeSecrets, check_new_passphrase, missing_node, zeroize_phrase,
};
use crate::{
    config::Config,
    error::{NodeError, Result},
};

impl Node {
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
            return Err(NodeError::NodeExists(home.to_owned()));
        }
        check_new_passphrase(passphrase)?;

        let master = MasterSecret::generate()?;
        let phrase = master.to_recovery_phrase()?;
        let node = Self::write_new(home, passphrase, username, &master)?;

        Ok((node, zeroize_phrase::Phrase(phrase)))
    }

    /// Create a node by restoring an identity from its recovery phrase.
    pub fn restore(home: &Path, passphrase: &str, username: &str, phrase: &str) -> Result<Self> {
        if Self::exists(home) {
            return Err(NodeError::NodeExists(home.to_owned()));
        }
        check_new_passphrase(passphrase)?;

        let master = MasterSecret::from_recovery_phrase(phrase)?;
        Self::write_new(home, passphrase, username, &master)
    }

    /// Create a node from the secrets held in a recovery container.
    ///
    /// The container carries the account identity; the device key is generated
    /// fresh, because this is a different machine and a device key identifies a
    /// machine rather than a person. Losing the old laptop then withdraws one
    /// enrolment rather than rotating the whole identity.
    pub fn restore_from_secrets(
        home: &Path,
        passphrase: &str,
        username: &str,
        secrets: &[u8],
    ) -> Result<Self> {
        if Self::exists(home) {
            return Err(NodeError::NodeExists(home.to_owned()));
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
        // master secret, so losing this laptop withdraws one enrolment rather
        // than forcing the user to rotate their whole identity. The keystore
        // below holds the master secret as well, so a *stolen* node with its
        // passphrase is the whole account; see `itsanas-coord`'s `claim.rs`.
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

        std::fs::create_dir_all(home).map_err(|error| NodeError::Io {
            path: home.to_owned(),
            source: error,
        })?;

        // A new file or nothing: `create_new` neither follows a link at the
        // name nor replaces a keystore another writer made after the
        // `exists` check, whose master would be lost.
        let keystore_path = Self::keystore_path(home);
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| NodeError::Io { path, source }
        };
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&keystore_path)
            .and_then(|mut file| {
                std::io::Write::write_all(&mut file, &keystore.to_bytes())?;
                file.sync_all()
            })
            .map_err(io(&keystore_path))?;

        // From here a failure takes back what this call wrote, so no sealed
        // master is left behind a refusal and a retry starts clean.
        let config_path = Self::config_path(home);
        let config_was_there = std::fs::symlink_metadata(&config_path).is_ok();
        let rest = (|| {
            let config = Config {
                username: username.to_owned(),
                ..Config::default()
            };
            if !config_was_there {
                // A fresh home: the config is new too, never written through
                // a link that appeared at its name.
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&config_path)
                    .map_err(io(&config_path))?;
            }
            config.save(&config_path)?;
            Self::assemble(home, config, master, &device, encoded)
        })();
        if rest.is_err() {
            let _ = std::fs::remove_file(&keystore_path);
            if !config_was_there {
                let _ = std::fs::remove_file(&config_path);
            }
        }
        rest
    }

    /// Open an existing node.
    pub fn open(home: &Path, passphrase: &str) -> Result<Self> {
        Self::open_as(home, passphrase, false)
    }

    /// Open a node whose configured vault's disk is not there (a USB disk
    /// unplugged): its own files keep syncing, through an empty stand-in
    /// vault under the home ([`AWAY_VAULT`]), and it hosts nothing.
    /// Whatever the stand-in gathers (siblings' segments and chunks on their
    /// way to this store) is thrown away at the next opening; it is all
    /// still on the machines that sent it.
    ///
    /// # Errors
    ///
    /// As [`Node::open`], and if the stand-in cannot be made.
    ///
    /// [`AWAY_VAULT`]: super::AWAY_VAULT
    pub fn open_away(home: &Path, passphrase: &str) -> Result<Self> {
        Self::open_as(home, passphrase, true)
    }

    fn open_as(home: &Path, passphrase: &str, away: bool) -> Result<Self> {
        let keystore_path = Self::keystore_path(home);
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
        let plaintext = keystore
            .unlock(passphrase, KEYSTORE_LABEL)
            .map_err(|_| NodeError::Unlock)?;

        let secrets: NodeSecrets = postcard::from_bytes(&plaintext)?;
        let master = MasterSecret::from_bytes(secrets.master);
        let device = DeviceKeys::from_seed(&SecretBytes::new(secrets.device_seed));

        let config = Config::load(&Self::config_path(home))?;
        Self::assemble_as(home, config, &master, &device, plaintext, away)
    }

    fn assemble(
        home: &Path,
        config: Config,
        master: &MasterSecret,
        device: &DeviceKeys,
        secrets: Vec<u8>,
    ) -> Result<Self> {
        Self::assemble_as(home, config, master, device, secrets, false)
    }

    fn assemble_as(
        home: &Path,
        config: Config,
        master: &MasterSecret,
        device: &DeviceKeys,
        secrets: Vec<u8>,
        away: bool,
    ) -> Result<Self> {
        let user = UserKeys::derive(master);

        // Belt and braces: `Store::open` performs this check too, but failing
        // here produces a message about the *account* rather than about a
        // storage path, which is what the person reading it needs.
        if is_published_test_identity(&user.user_id()) {
            return Err(NodeError::Usage(
                "this recovery phrase belongs to one of the published test \
                 identities in docs/TEST-USERS.md. Its private keys are printed \
                 in the documentation, so anyone at all can read data stored \
                 under it. Refusing to open it as a real account."
                    .to_owned(),
            ));
        }

        let store = Store::open(
            Self::store_path(home),
            user,
            DeviceKeys::from_seed(&device.seed()),
        )?;
        store.set_write_budget(Some(WriteBudget {
            allowed: Self::allowed_for(&config, Self::account_pledge(home, &config)),
            elsewhere: 0,
            // Set by `bound_writes`, which every writing path calls first:
            // the vault it needs is opened below.
            local_ceiling: None,
            folder_copy: FolderCopy::None,
        }))?;
        let vault = if away {
            Self::stand_in_vault(home, &device.device_id())?
        } else {
            Self::open_vault(home, &config, &device.device_id())?
        };
        // Arms this account's roster: from now on a chain signed by a key no
        // claim of this account names is not applied here (HANDOVER §10 7).
        store.admit_member(&device.device_id(), false)?;

        Ok(Self {
            home: home.to_owned(),
            config,
            store,
            vault,
            device: DeviceKeys::from_seed(&device.seed()),
            user: UserKeys::derive(master),
            secrets: zeroize::Zeroizing::new(secrets),
            away,
        })
    }

    /// Re-seal this machine's keystore under a new passphrase.
    ///
    /// Reads the keystore file and nothing else, so it works while the daemon
    /// holds the store: the daemon unlocked its keys at start and keeps them.
    /// Its *next* start is the problem, because whatever supplies the passphrase
    /// non-interactively still has the old one; the caller has to say so.
    ///
    /// The new container is written beside the old one and renamed over it, so
    /// a crash leaves either the old keystore or the new one and never a
    /// half-written file that opens with neither passphrase. The secrets are not
    /// regenerated: same account, same device id.
    pub fn change_passphrase(home: &Path, current: &str, new: &str) -> Result<()> {
        check_new_passphrase(new)?;
        let keystore_path = Self::keystore_path(home);
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

        let plaintext = zeroize::Zeroizing::new(
            Keystore::from_bytes(&bytes)?
                .unlock(current, KEYSTORE_LABEL)
                .map_err(|_| NodeError::Unlock)?,
        );
        let sealed = Keystore::lock(new, KEYSTORE_LABEL, &plaintext, KdfParams::RECOMMENDED)?;

        let pending = home.join("keystore.bin.new");
        let io = |path: &Path| {
            let path = path.to_owned();
            move |error| NodeError::Io {
                path: path.clone(),
                source: error,
            }
        };
        {
            let mut file = std::fs::File::create(&pending).map_err(io(&pending))?;
            std::io::Write::write_all(&mut file, &sealed.to_bytes()).map_err(io(&pending))?;
            // Without this the rename can reach the disk before the bytes do,
            // and a power cut leaves a keystore of zeroes under the real name.
            file.sync_all().map_err(io(&pending))?;
        }
        std::fs::rename(&pending, &keystore_path).map_err(io(&keystore_path))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Identity;
    use crate::node::tests::PASSPHRASE;

    /// THE SECOND ACCOUNT: a node home on a disk that is not mounted is an
    /// empty directory, and "no node found, run `itsanas init`" is then advice
    /// to create a **second** account on the root filesystem -- while the real
    /// one sits on a disk nobody is looking at any more, and the next backup
    /// captures the empty one. The two cases must not read the same.
    #[test]
    fn an_empty_node_home_reads_as_unmounted_storage_rather_than_a_fresh_start() {
        let dir = tempfile::tempdir().expect("temp dir");

        let mount_point = dir.path().join("mounted-nowhere");
        std::fs::create_dir(&mount_point).expect("create");
        let said = Node::open(&mount_point, "whatever")
            .expect_err("an empty home has no node")
            .to_string();
        assert!(
            said.contains("not mounted"),
            "an empty directory must be read as storage that is missing; it said {said:?}"
        );
        assert!(
            !said.contains("Run `itsanas init`"),
            "it suggested creating a second account beside the missing one"
        );

        // A path that does not exist at all is an ordinary fresh start.
        let nowhere = dir.path().join("never-existed");
        let said = Node::open(&nowhere, "whatever")
            .expect_err("no node there either")
            .to_string();
        assert!(
            said.contains("itsanas init"),
            "a directory that does not exist is where somebody starts; it said {said:?}"
        );
    }

    /// A passphrase the rule refuses leaves nothing a retry would trip on.
    fn assert_nothing_written(home: &Path, what: &str) {
        assert!(
            !Node::exists(home) && !home.exists(),
            "{what} refused and yet wrote into {}: the next attempt reads a node that was \
             never made",
            home.display()
        );
    }

    /// THE RULE FOR A NEW PASSPHRASE. 5 and 129 characters are refused by
    /// `create`, `restore` and `change_passphrase`, naming both numbers, and
    /// write nothing; 6 and 128 are accepted, and six accented letters are six
    /// characters, not twelve bytes. Sabotage: drop `check_new_passphrase` from
    /// each of the three in turn; count bytes instead of characters.
    #[test]
    fn red_team_a_new_passphrase_obeys_the_rule_on_every_path_and_writes_nothing_when_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, phrase) = Node::create(&dir.path().join("seed"), PASSPHRASE, "camille").unwrap();

        for bad in ["x".repeat(5), "x".repeat(129)] {
            let home = dir.path().join(format!("create-{}", bad.len()));
            let said = Node::create(&home, &bad, "camille")
                .expect_err("a passphrase outside the rule made an account")
                .to_string();
            assert!(
                said.contains('6') && said.contains("128"),
                "the refusal does not say the rule, so the person guesses again: {said}"
            );
            assert_nothing_written(&home, "create");

            let home = dir.path().join(format!("restore-{}", bad.len()));
            assert!(
                Node::restore(&home, &bad, "camille", phrase.as_str()).is_err(),
                "a passphrase outside the rule restored an account"
            );
            assert_nothing_written(&home, "restore");

            let before = std::fs::read(Node::keystore_path(&seed.home)).unwrap();
            assert!(
                Node::change_passphrase(&seed.home, PASSPHRASE, &bad).is_err(),
                "a passphrase outside the rule re-sealed the keystore"
            );
            assert_eq!(
                std::fs::read(Node::keystore_path(&seed.home)).unwrap(),
                before,
                "a refused new passphrase changed the keystore"
            );
        }

        for (name, good) in [
            ("six", "x".repeat(6)),
            ("max", "x".repeat(128)),
            ("accents", "\u{e9}".repeat(6)),
        ] {
            let home = dir.path().join(name);
            Node::create(&home, &good, "camille")
                .unwrap_or_else(|error| panic!("{name} was refused: {error}"));
        }
        assert!(
            check_new_passphrase(&"\u{e9}".repeat(5)).is_err(),
            "five accented letters passed as ten bytes"
        );
    }

    /// AN EXISTING MEMBER IS NEVER LOCKED OUT. A node sealed under a passphrase
    /// outside today's rule (one made before the rule, or recovered through a
    /// container sealed under whatever the first machine had) keeps opening,
    /// and keeps giving its words. Sabotage: call `check_new_passphrase` from
    /// `write_new`, or from `open_as`.
    #[test]
    fn red_team_a_passphrase_from_before_the_rule_still_opens_its_node() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, _) = Node::create(&dir.path().join("seed"), PASSPHRASE, "camille").unwrap();
        let secrets = Identity::secrets(&seed.home, PASSPHRASE).unwrap();

        for (name, legacy) in [("short", "abc".to_owned()), ("long", "y".repeat(200))] {
            let home = dir.path().join(name);
            let restored =
                Node::restore_from_secrets(&home, &legacy, "camille", secrets.as_bytes())
                    .unwrap_or_else(|error| {
                        panic!("a {name} escrow passphrase was refused: {error}")
                    });
            let owner = restored.store.owner();
            drop(restored);
            let reopened = Node::open(&home, &legacy).unwrap_or_else(|error| {
                panic!("a member with a {name} passphrase is locked out: {error}")
            });
            assert_eq!(reopened.store.owner(), owner);
            assert!(
                Identity::recovery_phrase(&home, &legacy).is_ok(),
                "a member with a {name} passphrase cannot get their words back"
            );
        }
    }

    /// Words that do not parse leave no keystore behind: the app decides between
    /// Unlock and the first screen on that file alone, so one left by a refused
    /// login would ask for ever for a passphrase nobody finished choosing.
    #[test]
    fn a_refused_phrase_writes_no_keystore() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("phone");
        for bad in [
            "not twenty four words".to_owned(),
            "abandon ".repeat(24),
            "zzzz ".repeat(24),
        ] {
            assert!(
                Node::restore(&home, PASSPHRASE, "camille", &bad).is_err(),
                "{bad:?} restored an account"
            );
            assert!(
                !Node::exists(&home),
                "a refused phrase left a keystore: the app is stuck on Unlock"
            );
        }
    }

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
            Err(NodeError::Unlock)
        ));
    }

    #[test]
    fn a_changed_passphrase_opens_the_same_node_and_the_old_one_no_longer_does() {
        // A passphrase that cannot be changed is one that stays in whatever
        // file or shell history it was first typed into. Changing it must not
        // regenerate anything: a new device id would look like a new machine
        // to every version vector, and a new master secret would be a new
        // account with nothing in it.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (before, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        let (user, device) = (before.store.owner(), before.store.device_id());
        drop(before);

        Node::change_passphrase(&home, PASSPHRASE, "a different long passphrase").unwrap();

        let after = Node::open(&home, "a different long passphrase").unwrap();
        assert_eq!(
            after.store.owner(),
            user,
            "the account changed with the passphrase"
        );
        assert_eq!(
            after.store.device_id(),
            device,
            "the device id changed with the passphrase"
        );
        drop(after);
        assert!(
            matches!(Node::open(&home, PASSPHRASE), Err(NodeError::Unlock)),
            "the old passphrase still opens the keystore"
        );
        assert!(
            !home.join("keystore.bin.new").exists(),
            "the pending keystore was left behind"
        );
    }

    #[test]
    fn a_wrong_current_passphrase_changes_nothing() {
        // Otherwise anybody at an unlocked terminal could lock the owner out of
        // their own machine by choosing a passphrase for it.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        let before = std::fs::read(home.join("keystore.bin")).unwrap();

        assert!(matches!(
            Node::change_passphrase(&home, "not the passphrase", "chosen by somebody else"),
            Err(NodeError::Unlock)
        ));
        assert_eq!(
            std::fs::read(home.join("keystore.bin")).unwrap(),
            before,
            "a refused change still rewrote the keystore"
        );
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
            Err(NodeError::NodeExists(_))
        ));
        assert!(matches!(
            Node::restore(&home, PASSPHRASE, "nicolas", "irrelevant"),
            Err(NodeError::NodeExists(_))
        ));
    }

    /// A LINK WHERE THE KEYSTORE GOES: a dangling `keystore.bin` (planted on
    /// a USB key, pointing anywhere) is a node that exists, so nothing is
    /// created through it; and a keystore is only ever written as a new
    /// file, never over one that appeared after the check -- the sealed
    /// master of whoever was first stays.
    #[cfg(unix)]
    #[test]
    fn red_team_a_keystore_is_never_written_through_a_link_or_over_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        std::fs::create_dir_all(&home).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, home.join("keystore.bin")).unwrap();
        assert!(
            Node::exists(&home),
            "a dangling keystore link reads as no node"
        );
        assert!(matches!(
            Node::create(&home, PASSPHRASE, "nicolas"),
            Err(NodeError::NodeExists(_))
        ));
        // Past the check, as a second writer racing the first would be.
        let master = MasterSecret::generate().unwrap();
        assert!(Node::write_new(&home, PASSPHRASE, "nicolas", &master).is_err());
        assert!(
            !elsewhere.exists(),
            "the keystore was written through the link"
        );

        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("keystore.bin"), b"the first one's").unwrap();
        assert!(Node::write_new(&other, PASSPHRASE, "nicolas", &master).is_err());
        assert_eq!(
            std::fs::read(other.join("keystore.bin")).unwrap(),
            b"the first one's"
        );
    }

    #[test]
    fn opening_a_missing_node_says_what_to_do_about_it() {
        let dir = tempfile::tempdir().unwrap();
        let error = Node::open(&dir.path().join("nothing-here"), PASSPHRASE).unwrap_err();

        let message = error.to_string();
        assert!(matches!(error, NodeError::NoNode(_)));
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

    /// A recovery phrase stored on the machine it protects is not a backup,
    /// and is an extra copy for an attacker to find.
    ///
    /// # Why the needle is three words and not one
    ///
    /// It used to search for the phrase's **first word followed by a space**.
    /// A BIP39 word can be three letters, so the needle could be four ASCII
    /// bytes, hunted through hundreds of kilobytes of keystore and redb pages:
    /// it collides by chance. On 2026-09-16 this test failed in CI's coverage
    /// job and **passed on a re-run of the identical commit**, which is the
    /// proof — 125 local runs never reproduced it, so the rate is low and the
    /// test still fires often enough to be noticed.
    ///
    /// That is the worst failure mode a security test has. It does not merely
    /// waste a run: it teaches everybody that this particular alarm is noise,
    /// so the day it catches a real leak nobody believes it.
    ///
    /// Three words is ~33 bits of BIP39 entropy, which does not collide, and
    /// it is the form a leak would take anyway — `RecoveryPhrase::as_str` is
    /// space-joined and is the only spelling this code ever produces.
    #[test]
    fn the_phrase_is_not_written_anywhere_under_the_node_directory() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (_node, phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();

        let words: Vec<&str> = phrase.as_str().split_whitespace().take(3).collect();
        assert_eq!(words.len(), 3, "a recovery phrase should have 24 words");
        let needle = words.join(" ");

        // The control first, because "nothing found" is also what a broken
        // search prints. `C scan` in the acceptance kit does the same, for the
        // same reason, and found a wrong path that way.
        let planted = home.join("control.bin");
        std::fs::write(&planted, format!("x{needle}x")).unwrap();
        assert!(
            scan(&home, &needle),
            "the control could not find a planted phrase, so this test proves \
             nothing about the real one"
        );
        std::fs::remove_file(&planted).unwrap();

        assert!(
            !scan(&home, &needle),
            "the recovery phrase appears in plaintext under the node directory"
        );
    }
}
