//! What a node may write and must keep free: the split between keeping and
//! pledging, the account's pledge, the write budget and the disk room.

use std::fmt;
use std::path::{Path, PathBuf};

use itsanas_store::{FolderCopy, Presence, WriteBudget};

use super::Node;
use crate::{
    config::Config,
    error::{NodeError, Result},
};

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

impl Node {
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
        config
            .pledge_bytes
            .saturating_add(Self::others_pledged(home))
    }

    /// What this account's other machines pledge, as the coordinator last
    /// listed them; nothing, if it never did.
    #[must_use]
    pub fn others_pledged(home: &Path) -> u64 {
        std::fs::read_to_string(Self::others_pledged_path(home))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0)
    }

    /// How long ago [`Self::remember_others_pledged`] last wrote, at `now`;
    /// `None` when it never did. A clock set back reads as just written.
    #[must_use]
    pub fn others_pledged_age(
        home: &Path,
        now: std::time::SystemTime,
    ) -> Option<std::time::Duration> {
        let written = std::fs::metadata(Self::others_pledged_path(home))
            .and_then(|meta| meta.modified())
            .ok()?;
        Some(now.duration_since(written).unwrap_or_default())
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
        // Published every time the bound is set (each round, each write), so a
        // neighbour on this disk sees what this node still owes. Best effort:
        // a read-only home must not stop the node writing its own files.
        let _ = crate::pledges::publish(
            &self.home,
            crate::pledges::shared_dir().as_deref(),
            &self.store.device_id().to_hex(),
            self.config.offered_bytes(),
            held,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        );
        self.store.set_write_budget(Some(WriteBudget {
            allowed: Self::allowed_for(
                &self.config,
                Self::account_pledge(&self.home, &self.config),
            ),
            elsewhere,
            // The pledge is owed on the vault's disk: only when the vault
            // shares the home's disk does it come out of the room for our own.
            local_ceiling: Self::disk_room(
                free,
                if same_volume(&self.home, &self.vault_dir()).unwrap_or(true) {
                    self.config.offered_bytes()
                } else {
                    0
                },
                held,
            )
            .map(|room| local.saturating_add(room)),
            folder_copy: self.folder_copy(local),
        }))?;
        Ok(())
    }

    /// Where a pull's folder copy lands, for [`Store::pull_room`].
    ///
    /// Unsure counts as the same volume: charging the home's disk twice errs
    /// towards refusing, never towards filling the space the pledge owes.
    fn folder_copy(&self, local_then: u64) -> FolderCopy {
        let Some(folder) = &self.config.folder else {
            return FolderCopy::None;
        };
        if same_volume(&self.home, folder).unwrap_or(true) {
            return FolderCopy::SameVolume { local_then };
        }
        match fs4::available_space(existing_ancestor(folder)) {
            Ok(room) => FolderCopy::OtherVolume { local_then, room },
            Err(_) => FolderCopy::SameVolume { local_then },
        }
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
    /// Two running totals, no walk: this runs on every reconcile pass. It
    /// opens no blob directory either, so asking does not make the vault list
    /// us among the accounts it hosts.
    ///
    /// # Errors
    ///
    /// If the vault cannot be read.
    pub fn held_for_others(&self) -> Result<u64> {
        // The running total, equal to `stats().bytes` without the walk.
        let all = self.vault.held_bytes()?;
        let owner = self.store.owner();
        // A running total, not `stats_for`: this runs on every reconcile
        // pass, and `stats_for` walks every one of our blobs twice.
        let ours = self.vault.held_bytes_for(owner)?;
        Ok(all.saturating_sub(ours))
    }

    /// What this disk can take for the account's own files: `free` less what
    /// `pledge` still owes beyond the `held` bytes already hosted. `None` when
    /// the free space could not be read; `Some(0)` when it is really 0.
    #[must_use]
    pub fn disk_room(free: Option<u64>, pledge: u64, held: u64) -> Option<u64> {
        free.map(|free| free.saturating_sub(pledge.saturating_sub(held)))
    }

    /// [`crate::pledges::disk_refusal`] for this node: `bytes` promised on its
    /// vault's disk, against what it has free and what the other nodes on that
    /// disk promised. A disk that reads 0 free is one whose filesystem would
    /// not say; `pledge` and `space --apply` both ask here, so neither is the
    /// way around the other.
    #[must_use]
    pub fn disk_refusal(&self, bytes: u64, held: u64) -> Option<crate::pledges::DiskRefusal> {
        // The reserve file holds this node's own pledge: free to it.
        let reserved = crate::pledges::reserved(&self.vault_dir());
        let free = fs4::available_space(self.vault_dir())
            .ok()
            .filter(|free| *free > 0)
            .map(|free| free.saturating_add(reserved));
        self.disk_refusal_given(free, bytes, held)
    }

    /// [`Self::disk_refusal`] with what is free given, not read: the phone's
    /// setters pass what they read themselves, and a test what it pretends.
    #[must_use]
    pub fn disk_refusal_given(
        &self,
        free: Option<u64>,
        bytes: u64,
        held: u64,
    ) -> Option<crate::pledges::DiskRefusal> {
        let shared = crate::pledges::shared_dir();
        let (others, stale) = crate::pledges::others(
            &self.home,
            shared.as_deref(),
            &self.store.device_id().to_hex(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        );
        crate::pledges::disk_refusal(bytes, free, held, others, stale)
    }
}

/// The nearest ancestor of `path` that exists: a folder not created yet
/// still has a volume, its parent's.
pub(crate) fn existing_ancestor(path: &Path) -> &Path {
    path.ancestors()
        .find(|candidate| candidate.exists())
        .unwrap_or(path)
}

/// Whether `a` and `b` are on one volume; `None` when that cannot be told.
#[cfg(unix)]
pub(crate) fn same_volume(a: &Path, b: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let device = |path: &Path| {
        std::fs::metadata(existing_ancestor(path))
            .ok()
            .map(|m| m.dev())
    };
    Some(device(a)? == device(b)?)
}

/// Whether `a` and `b` are on one volume; `None` when that cannot be told.
///
/// By drive (the path prefix after canonicalising). A volume mounted in a
/// folder of another reads as the same: that errs towards charging twice.
#[cfg(windows)]
pub(crate) fn same_volume(a: &Path, b: &Path) -> Option<bool> {
    let prefix = |path: &Path| {
        std::fs::canonicalize(existing_ancestor(path))
            .ok()?
            .components()
            .next()
            .map(|first| first.as_os_str().to_ascii_lowercase())
    };
    Some(prefix(a)? == prefix(b)?)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn same_volume(_: &Path, _: &Path) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::tests::PASSPHRASE;

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
                local_ceiling: None,
                folder_copy: FolderCopy::None,
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

    /// `bound_writes` must tell the store that a pull also writes the synced
    /// folder: a folder beside the home is on its volume, so the pull is
    /// charged twice (`red_team_a_pull_charges_the_folder_copy_too`).
    #[test]
    fn red_team_a_folder_beside_the_home_is_charged_on_its_volume() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        node.bound_writes().unwrap();
        assert_eq!(
            node.store.write_budget().unwrap().map(|b| b.folder_copy),
            Some(FolderCopy::None)
        );
        node.config.folder = Some(dir.path().join("Documents"));
        node.bound_writes().unwrap();
        assert!(
            matches!(
                node.store.write_budget().unwrap().map(|b| b.folder_copy),
                Some(FolderCopy::SameVolume { .. })
            ),
            "a pull into a folder on the home's disk was charged once"
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
}
