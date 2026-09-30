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

use itsanas_crypto::{
    DeviceKeys, KdfParams, Keystore, MasterSecret, SecretBytes, UserKeys,
    is_published_test_identity,
};
use itsanas_store::{Presence, Store, Vault, WriteBudget};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    error::{NodeError, Result},
};

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

/// A `keep` the pledge does not earn, from [`Node::check_split`].
///
/// A value rather than a sentence so each caller can phrase it; its `Display`
/// is the one `keep` and `pledge` print, with the command that sets both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRefusal {
    /// What was to be kept here.
    pub keep: u64,
    /// The pledge that earns it, rounded up.
    pub needed: u64,
    /// The pledge asked for.
    pub pledge: u64,
}

impl fmt::Display for SplitRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use crate::config::{format_size, size_argument};
        write!(
            f,
            "keeping {} needs {} pledged, and this node offers {}. \
             `itsanas space --pledge {} --keep {} --apply` sets both, or keep less.",
            format_size(self.keep),
            size_argument(self.needed),
            format_size(self.pledge),
            size_argument(self.needed),
            size_argument(self.keep),
        )
    }
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
}

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
            .finish_non_exhaustive()
    }
}

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
    pub fn open(home: &Path, passphrase: &str) -> Result<Self> {
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
        let plaintext = keystore
            .unlock(passphrase, KEYSTORE_LABEL)
            .map_err(|_| NodeError::Unlock)?;

        let secrets: NodeSecrets = postcard::from_bytes(&plaintext)?;
        let master = MasterSecret::from_bytes(secrets.master);

        Ok(Self {
            config: Config::load(&Node::config_path(home))?,
            device: DeviceKeys::from_seed(&SecretBytes::new(secrets.device_seed)),
            user: UserKeys::derive(&master),
        })
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

    /// What `pledged_bytes` earns under this node's split, and never less than
    /// the joining allowance.
    ///
    /// One function so that `keep`, which passes this machine's pledge, and a
    /// write, which passes the account's, cannot disagree about the rule. The
    /// allowance applies whatever the account's age, as it does for `keep`:
    /// this node does not know when its account joined -- only the coordinator
    /// does -- and a limit that shrank on day thirty-one would refuse, on a
    /// timer, the files the same rule accepted the day before.
    #[must_use]
    pub fn allowed_for(config: &Config, pledged_bytes: u64) -> u64 {
        config
            .split
            .room_earned(pledged_bytes)
            .max(itsanas_coord::accounting::JOINING_ALLOWANCE)
    }

    /// Refuse a `pledge` and `keep` pair this machine's split does not allow:
    /// keeping more than the pledge earns.
    ///
    /// The one rule every setter asks before it saves -- `keep`, `pledge`,
    /// `space --apply` and the phone's `setKeep` and `setPledge`. Until
    /// 2026-09-30 only `keep` and `space` asked, so `keep 70G` then
    /// `pledge 1G` left a node keeping far more than it earned, and nobody
    /// found out until a coordinator refused it. `None` keeps everything,
    /// which no pledge refuses: the account's size is bounded by writes, not
    /// here.
    ///
    /// Only the honest client asks. A rebuilt one skips it; bounding owners
    /// on the host is §8 1c.
    ///
    /// # Errors
    ///
    /// A [`SplitRefusal`] naming the pledge that would make `keep` legal.
    pub fn check_split(
        config: &Config,
        pledge: u64,
        keep: Option<u64>,
    ) -> std::result::Result<(), SplitRefusal> {
        match keep {
            Some(keep) if keep > Self::allowed_for(config, pledge) => Err(SplitRefusal {
                keep,
                needed: config.split.pledge_needed_for(keep),
                pledge,
            }),
            _ => Ok(()),
        }
    }

    /// What the account's machines pledge together, as far as this one knows:
    /// its own pledge as configured now, plus the others' as the coordinator
    /// last listed them.
    ///
    /// # Why the sum, and not this machine's pledge
    ///
    /// Entitlement belongs to the account -- `accounting::assess` sums every
    /// device -- and a write adds to the account. Bounded by one machine's
    /// pledge, a laptop that pledges nothing, which is the default, would be
    /// held to the joining allowance for the whole account however much the Pi
    /// beside it lends. That was the rule first written for this step, taken
    /// from `keep`, whose question is what *this machine* holds; an audit found
    /// it before it merged.
    ///
    /// A machine that has never heard from a coordinator counts the others as
    /// nothing: it cannot vouch for pledges it has never been told about. The
    /// figure comes from the coordinator and is only as honest as it is, which
    /// is the whole of what a bound on the honest client can promise.
    #[must_use]
    pub fn account_pledge(home: &Path, config: &Config) -> u64 {
        let others = std::fs::read_to_string(Self::others_pledged_path(home))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0);
        config.pledge_bytes.saturating_add(others)
    }

    fn others_pledged_path(home: &Path) -> PathBuf {
        home.join("others-pledged")
    }

    /// Remember what the account's *other* machines pledge, for
    /// [`Self::account_pledge`].
    ///
    /// A file of its own rather than a configuration line: it is a copy of the
    /// coordinator's answer that nobody types, and the configuration parser
    /// refuses a key it does not know, so a line added there would stop an
    /// older binary opening this node at all.
    pub fn remember_others_pledged(&self, bytes: u64) -> Result<()> {
        let path = Self::others_pledged_path(&self.home);
        std::fs::write(&path, format!("{bytes}\n")).map_err(|error| NodeError::Io {
            path: path.clone(),
            source: error,
        })
    }

    /// Bound this node's writes by what the account's pledges earn, counting
    /// the files of the account this device has not downloaded.
    ///
    /// Opening a node already bounds writes, counting only what is here: that
    /// needs no walk and cannot be forgotten by a new caller. This adds what
    /// only the vault knows, at the cost of walking it, so the callers that
    /// are about to write call it and nobody else pays. A walk that stopped at
    /// [`MAX_SEGMENTS_WALKED`](itsanas_store::catalogue::MAX_SEGMENTS_WALKED)
    /// counts less than the account holds, which errs towards accepting.
    pub fn bound_writes(&self) -> Result<()> {
        let listing = itsanas_store::catalogue(&self.store, &self.vault)?;
        let elsewhere = listing
            .files
            .iter()
            .filter(|known| known.presence == Presence::Absent)
            .fold(0u64, |total, known| total.saturating_add(known.size));
        // `ok()`, not `unwrap_or(0)`: a disk that is really full reads 0, and
        // that is when the bound matters most -- it must not read as unknown.
        let free = fs4::available_space(&self.home).ok();
        let held = self.held_for_others()?;
        let local = self.store.local_bytes()?;
        self.store.set_write_budget(Some(WriteBudget {
            allowed: Self::allowed_for(
                &self.config,
                Self::account_pledge(&self.home, &self.config),
            ),
            elsewhere,
            local_ceiling: Self::disk_room(free, self.config.pledge_bytes, held)
                .map(|room| local.saturating_add(room)),
        }))?;
        Ok(())
    }

    /// Bytes this vault holds for *other* accounts: what counts against the
    /// pledge.
    ///
    /// Not the vault's whole size. This account's other devices push to it too
    /// (that is how a machine that cannot be dialled gets its work out), and
    /// those chunks and segments are ours, not a debt paid to anybody. Counted
    /// as hosted, they shrank what the pledge still owes and so loosened the
    /// reserve by exactly the size of our own backlog.
    ///
    /// Only asks about our own account if the vault already has it:
    /// `stats_for` opens the owner's blob directory, creating it, and a vault
    /// with a directory for us then lists us among the accounts it hosts.
    ///
    /// # Errors
    ///
    /// If the vault cannot be read.
    pub fn held_for_others(&self) -> Result<u64> {
        let all = self.vault.stats()?.bytes;
        let owner = self.store.owner();
        if !self.vault.owners()?.contains(&owner) {
            return Ok(all);
        }
        let ours = self.vault.stats_for(owner)?.bytes;
        Ok(all.saturating_sub(ours))
    }

    /// What this disk can take for the account's own files: `free` less what
    /// `pledge` still owes beyond the `held` bytes already hosted. `None` when
    /// the free space could not be read; `Some(0)` when it is really 0.
    #[must_use]
    pub fn disk_room(free: Option<u64>, pledge: u64, held: u64) -> Option<u64> {
        free.map(|free| free.saturating_sub(pledge.saturating_sub(held)))
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
            return Err(NodeError::NodeExists(home.to_owned()));
        }

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

        let keystore_path = Self::keystore_path(home);
        std::fs::write(&keystore_path, keystore.to_bytes()).map_err(|error| NodeError::Io {
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
        }))?;
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

    /// Every write goes through `node.store`, and the store refuses nothing it
    /// has not been told about. A node that opened without telling it would
    /// leave the CLI, the folder and the phone writing without a bound while
    /// every store test stayed green.
    /// The disk room is the free space less what the pledge still owes: bytes
    /// already hosted are no longer owed, and an unreadable free space
    /// bounds nothing rather than refusing every write.
    #[test]
    fn red_team_the_disk_room_sets_aside_what_the_pledge_still_owes() {
        const GB: u64 = 1_000_000_000;
        assert_eq!(
            Node::disk_room(Some(100 * GB), 70 * GB, 20 * GB),
            Some(50 * GB),
            "free 100, pledged 70 of which 20 already hosted: 50 still owed"
        );
        assert_eq!(
            Node::disk_room(Some(40 * GB), 70 * GB, 0),
            Some(0),
            "owed exceeds free"
        );
        assert_eq!(
            Node::disk_room(Some(40 * GB), 10 * GB, 30 * GB),
            Some(40 * GB),
            "over-hosted owes nothing"
        );
        assert_eq!(
            Node::disk_room(None, 70 * GB, 0),
            None,
            "unknown free space"
        );
        assert_eq!(
            Node::disk_room(Some(0), 70 * GB, 0),
            Some(0),
            "a disk that is really full must bound, not read as unknown"
        );
    }

    #[test]
    fn red_team_an_opened_node_bounds_its_writes_by_what_its_pledge_earns() {
        use itsanas_coord::accounting::JOINING_ALLOWANCE;
        const GB: u64 = 1_000_000_000;

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        node.config.pledge_bytes = 700 * GB;
        node.save_config().unwrap();
        drop(node);

        let node = Node::open(&home, PASSPHRASE).unwrap();
        assert_eq!(
            node.store.write_budget().unwrap(),
            Some(WriteBudget {
                allowed: 300 * GB,
                elsewhere: 0,
                local_ceiling: None
            }),
            "a node pledging 700 GB at 30/70 must be held to the 300 GB that earns"
        );

        node.bound_writes().unwrap();
        assert_eq!(
            node.store
                .write_budget()
                .unwrap()
                .map(|budget| budget.allowed),
            Some(300 * GB)
        );

        let mut config = node.config.clone();
        config.pledge_bytes = 0;
        assert_eq!(
            Node::allowed_for(&config, 0),
            JOINING_ALLOWANCE,
            "a node pledging nothing must still have the joining allowance, as `keep` does"
        );
    }

    /// A phone keeping a sliver of an account knows the rest only from the
    /// laptop's log in its vault. `bound_writes` has to count those files, or
    /// the phone writes as though the account were the sliver.
    #[test]
    fn red_team_files_this_machine_has_not_downloaded_count_against_its_writes() {
        let dir = tempfile::tempdir().unwrap();
        let (laptop, phrase) =
            Node::create(&dir.path().join("laptop"), PASSPHRASE, "nicolas").unwrap();
        let phone =
            Node::restore(&dir.path().join("phone"), PASSPHRASE, "nicolas", &phrase.0).unwrap();

        laptop
            .store
            .write_file("big.bin", &vec![7u8; 300_000])
            .unwrap();
        laptop.store.flush_segment().unwrap();
        for envelope in laptop.store.segments().unwrap() {
            phone.vault.put_segment(&envelope).unwrap();
        }

        phone.bound_writes().unwrap();
        assert_eq!(
            phone
                .store
                .write_budget()
                .unwrap()
                .map(|budget| budget.elsewhere),
            Some(300_000),
            "a file known only from another machine's log was not counted, so this \
             machine can write past what the account may hold"
        );
    }

    /// The laptop writes, the Pi lends. The laptop pledges nothing -- the
    /// default -- and the account is entitled by the Pi's pledge, because
    /// entitlement is the account's. Bounded by its own pledge, the laptop
    /// would refuse every file past the joining allowance for the whole
    /// account, while the machine lending for it sat half empty.
    #[test]
    fn red_team_a_machine_that_lends_nothing_writes_by_what_the_account_lends() {
        const GB: u64 = 1_000_000_000;

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("laptop");
        let (node, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        assert_eq!(
            node.config.pledge_bytes, 0,
            "the fixture is the default laptop"
        );
        node.remember_others_pledged(700 * GB).unwrap();
        drop(node);

        let node = Node::open(&home, PASSPHRASE).unwrap();
        assert_eq!(
            node.store
                .write_budget()
                .unwrap()
                .map(|budget| budget.allowed),
            Some(300 * GB),
            "a laptop pledging nothing in an account whose Pi lends 700 GB must be \
             held to the 300 GB the account earns, not to the joining allowance"
        );

        // And again after the refresh every writer calls, which is the bound a
        // write actually meets.
        node.bound_writes().unwrap();
        assert_eq!(
            node.store
                .write_budget()
                .unwrap()
                .map(|budget| budget.allowed),
            Some(300 * GB),
            "the refresh before a write fell back to this machine's own pledge"
        );
    }

    /// The laptop pushes its own files into the Pi's vault; somebody else's
    /// data sits there too. Only the second is paid against the Pi's pledge.
    #[test]
    fn red_team_our_own_chunks_in_the_vault_do_not_count_as_hosted() {
        let dir = tempfile::tempdir().unwrap();
        let (pi, _phrase) = Node::create(&dir.path().join("pi"), PASSPHRASE, "nicolas").unwrap();
        let (stranger, _phrase) =
            Node::create(&dir.path().join("stranger"), PASSPHRASE, "somebody").unwrap();

        let address = itsanas_crypto::ChunkId::from_bytes([3u8; 32]);
        pi.vault
            .put_chunk(pi.store.owner(), &address, &vec![1u8; 40_000])
            .unwrap();
        assert_eq!(
            pi.held_for_others().unwrap(),
            0,
            "this account's own chunks counted as hosted for others: the \
             reserve for the pledge shrinks by our own backlog and the disk \
             fills with it"
        );

        // Asking must not enrol anybody: a fresh vault stays a vault that
        // hosts no account, as `status` reports it.
        let (fresh, _phrase) =
            Node::create(&dir.path().join("fresh"), PASSPHRASE, "fresh").unwrap();
        fresh.bound_writes().unwrap();
        assert_eq!(
            fresh.vault.stats().unwrap().owners,
            0,
            "working out the disk room made this machine list itself among \
             the accounts it hosts"
        );

        pi.vault
            .put_chunk(stranger.store.owner(), &address, &vec![2u8; 10_000])
            .unwrap();
        assert_eq!(
            pi.held_for_others().unwrap(),
            10_000,
            "another account's chunk is exactly what the pledge is paid in"
        );
    }

    /// A pledge lowered under what `keep` needs is refused, with the pledge
    /// that would earn it; a keep inside the allowance, or none, is refused by
    /// no pledge. Sabotage: make `check_split` return `Ok(())` -- a node then
    /// keeps 20 GiB on a 1 MiB pledge and only a coordinator notices.
    #[test]
    fn red_team_a_pledge_under_what_keep_needs_is_refused() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let config = Config::default();

        let refusal = Node::check_split(&config, 1024 * 1024, Some(20 * GIB))
            .expect_err("a 1 MiB pledge accepted for a 20 GiB keep");
        assert_eq!(refusal.needed, config.split.pledge_needed_for(20 * GIB));
        assert!(
            Node::check_split(&config, refusal.needed, Some(20 * GIB)).is_ok(),
            "the pledge the refusal names is refused in turn"
        );
        let said = refusal.to_string();
        assert!(
            said.contains("itsanas space --pledge 47G --keep 20G --apply"),
            "the refusal lost the command that fixes it: {said}"
        );

        assert!(
            Node::check_split(
                &config,
                0,
                Some(itsanas_coord::accounting::JOINING_ALLOWANCE)
            )
            .is_ok(),
            "a keep inside the joining allowance needs no pledge"
        );
        assert!(
            Node::check_split(&config, 0, None).is_ok(),
            "keeping everything is bounded by writes, not by the pledge"
        );

        // The node's own split, not the default: a stricter one refuses what
        // 30/70 allows. 50 GiB earns 21.4 GiB at 30/70 and 12.5 GiB at 20/80.
        assert!(Node::check_split(&config, 50 * GIB, Some(20 * GIB)).is_ok());
        let strict = Config {
            split: itsanas_coord::accounting::Split::new(20, 80).unwrap(),
            ..Config::default()
        };
        assert!(
            Node::check_split(&strict, 50 * GIB, Some(20 * GIB)).is_err(),
            "the check read the default split, not this node's stricter one"
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

    #[test]
    fn the_phrase_does_not_leak_through_debug() {
        let dir = tempfile::tempdir().unwrap();
        let (_node, phrase) = Node::create(&dir.path().join("node"), PASSPHRASE, "n").unwrap();

        let rendered = format!("{phrase:?}");
        assert_eq!(rendered, "Phrase(redacted)");
        assert!(!rendered.contains(phrase.as_str().split_whitespace().next().unwrap()));
    }
}
