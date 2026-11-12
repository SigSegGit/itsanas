//! How a delete travels (HANDOVER §8 13): each machine's rules, the account's
//! default, and the trash the whole account shares.
//!
//! # Rules live in the account, not on one machine
//!
//! The account's default and each machine's own rules are empty files under
//! [`RESERVED`], written to the log like any file: signed by the device that
//! wrote them, sealed under the account's key, and the same on every machine.
//! What they say is in their path, so a machine that downloads nothing (a
//! phone) reads them from the log alone. Every machine shows every machine's
//! rules, and only a device of the account can change them. No folder,
//! listing or drive shows these files ([`crate::catalogue()`]).
//!
//! # A delete says what it is
//!
//! A delete into the shared trash is a marker naming when the file is really
//! deleted, who deleted it and the file's path: the file itself stays a live
//! file of the account until then, so restoring it is taking the marker away,
//! from any machine. A definitive delete is the log's own tombstone. A machine
//! that receives one applies it by what it is, never by its own mode, so two
//! machines with different rules cannot disagree on what happened.
//!
//! # Protect
//!
//! A machine whose rules say [`Receive::Protect`] keeps, for its days, the entry
//! of a file another machine deleted outright (not out of the shared trash):
//! [`protect`] records it before the tombstone applies, garbage collection
//! keeps its chunks, and [`restore_protected`] publishes it again.

use std::collections::{BTreeMap, BTreeSet};

use itsanas_crypto::{ChunkId, DeviceId};

use crate::{FileEntry, Store, error::StoreError, oplog::Operation, trash, vault::Vault};

/// The top folder of the account that holds the rules and the shared trash.
pub const RESERVED: &str = ".itsanas";

const ACCOUNT: &str = ".itsanas/account/";
const MACHINES: &str = ".itsanas/machines/";
const SHARED: &str = ".itsanas/trash/";

/// This machine's record of what it protects: `path` -> (due, entry).
const PROTECTED: &str = "protected";

/// The account's trash as last read, next to the store: a phone's trash screen
/// reads it while a round holds the node.
const NOTED: &str = "trash-shared";

/// Whether `path` is one of the account's own files, never a person's.
#[must_use]
pub fn is_reserved(path: &str) -> bool {
    path == RESERVED || path.starts_with(".itsanas/")
}

/// What a delete made on a machine does to the rest of the account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Into the account's trash for the machine's days, restorable from any
    /// machine until then.
    #[default]
    Trash,
    /// Deleted from every machine at once.
    Direct,
    /// Gone from this machine only; the account keeps it.
    Local,
}

/// What a machine does with a file another machine deleted outright.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Receive {
    /// Keep it here for this machine's days before it goes.
    #[default]
    Protect,
    /// Let it go at once.
    Apply,
}

/// One machine's rules, or the account's default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// What a delete made here does.
    pub mode: Mode,
    /// What a delete made elsewhere does here.
    pub receive: Receive,
    /// How long a trashed or protected file waits, at least one day.
    pub days: u64,
    /// Whether a definitive delete asks who is there first.
    pub ask_auth: bool,
    /// Whether a delete asks "are you sure" first.
    pub confirm: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            mode: Mode::Trash,
            receive: Receive::Protect,
            days: trash::DEFAULT_DAYS,
            ask_auth: true,
            confirm: true,
        }
    }
}

impl Mode {
    /// The word the record, the page and the command use.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Trash => "trash",
            Self::Direct => "direct",
            Self::Local => "local",
        }
    }

    /// The mode a word names.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "trash" => Some(Self::Trash),
            "direct" => Some(Self::Direct),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    /// How much a mode keeps: the trash keeps most, a local delete keeps the
    /// account whole, a direct one nothing.
    const fn keeps(self) -> u8 {
        match self {
            Self::Trash => 2,
            Self::Local => 1,
            Self::Direct => 0,
        }
    }
}

impl Receive {
    /// The word the record, the page and the command use.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Protect => "protect",
            Self::Apply => "apply",
        }
    }

    /// The rule a word names.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "protect" => Some(Self::Protect),
            "apply" => Some(Self::Apply),
            _ => None,
        }
    }
}

impl Rules {
    /// The rules as one path component: `trash.protect.30.1.1`.
    #[must_use]
    pub fn to_word(&self) -> String {
        format!(
            "{}.{}.{}.{}.{}",
            self.mode.word(),
            self.receive.word(),
            self.days,
            u8::from(self.ask_auth),
            u8::from(self.confirm)
        )
    }

    /// Read a component written by [`Self::to_word`]; `None` when it is not one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        let mut parts = word.split('.');
        let rules = Self {
            mode: Mode::from_word(parts.next()?)?,
            receive: Receive::from_word(parts.next()?)?,
            days: parts.next()?.parse::<u64>().ok()?.clamp(1, trash::MAX_DAYS),
            ask_auth: parts.next()? != "0",
            confirm: parts.next()? != "0",
        };
        parts.next().is_none().then_some(rules)
    }

    /// What breaks a tie between two sets of rules written apart: the one that
    /// keeps more wins, the same on every machine.
    fn protection(&self) -> (u8, bool, u64, bool, bool) {
        (
            self.mode.keeps(),
            self.receive == Receive::Protect,
            self.days,
            self.ask_auth,
            self.confirm,
        )
    }

    /// Whether going from `self` to `next` keeps less, so the person must say
    /// who they are first: a mode that keeps less, the protection dropped, a
    /// shorter wait, or either question turned off.
    #[must_use]
    pub fn loosened_by(&self, next: &Self) -> bool {
        next.mode.keeps() < self.mode.keeps()
            || (self.receive == Receive::Protect && next.receive == Receive::Apply)
            || next.days < self.days
            || (self.ask_auth && !next.ask_auth)
            || (self.confirm && !next.confirm)
    }
}

/// The account's own files, whether their content is here or not.
fn reserved(store: &Store, vault: &Vault) -> crate::Result<Vec<String>> {
    crate::catalogue::reserved(store, vault)
}

/// The most protective rules among the files under `prefix`: two machines that
/// changed them apart leave two, and both read the same winner.
fn rules_under(listed: &[String], prefix: &str) -> Option<Rules> {
    listed
        .iter()
        .filter_map(|path| Rules::from_word(path.strip_prefix(prefix)?))
        .max_by_key(Rules::protection)
}

/// Write `rules` as the only rules under `prefix`.
fn replace_rules(
    store: &Store,
    vault: &Vault,
    listed: &[String],
    prefix: &str,
    rules: Option<&Rules>,
) -> crate::Result<()> {
    for path in listed.iter().filter(|path| path.starts_with(prefix)) {
        crate::remove_known(store, vault, path)?;
    }
    if let Some(rules) = rules {
        store.write_file(&format!("{prefix}{}", rules.to_word()), b"")?;
    }
    Ok(())
}

fn machine_prefix(device: &str) -> String {
    format!("{MACHINES}{device}/")
}

/// The account's default rules: what a machine follows until it has its own.
///
/// # Errors
///
/// When the store or the vault cannot be read.
pub fn account_rules(store: &Store, vault: &Vault) -> crate::Result<Rules> {
    Ok(rules_under(&reserved(store, vault)?, ACCOUNT).unwrap_or_default())
}

/// The rules this machine applies now: its own, or the account's.
///
/// # Errors
///
/// When the store or the vault cannot be read.
pub fn rules_here(store: &Store, vault: &Vault) -> crate::Result<Rules> {
    let listed = reserved(store, vault)?;
    Ok(
        rules_under(&listed, &machine_prefix(&store.device_id().to_hex()))
            .or_else(|| rules_under(&listed, ACCOUNT))
            .unwrap_or_default(),
    )
}

/// Set this machine's own rules, or `None` to follow the account's.
///
/// # Errors
///
/// When the store cannot be read or written.
pub fn set_rules_here(store: &Store, vault: &Vault, rules: Option<&Rules>) -> crate::Result<()> {
    let listed = reserved(store, vault)?;
    let prefix = machine_prefix(&store.device_id().to_hex());
    replace_rules(store, vault, &listed, &prefix, rules)?;
    store.flush_segment()?;
    Ok(())
}

/// Make the account uniform: `rules` become its default and every machine
/// follows it, its own rules taken away.
///
/// # Errors
///
/// When the store cannot be read or written.
pub fn apply_to_all(store: &Store, vault: &Vault, rules: &Rules) -> crate::Result<()> {
    let listed = reserved(store, vault)?;
    replace_rules(store, vault, &listed, ACCOUNT, Some(rules))?;
    replace_rules(store, vault, &listed, MACHINES, None)?;
    store.flush_segment()?;
    Ok(())
}

/// The account's default, and every machine that has rules of its own, by
/// device id as hex; the others follow the default.
///
/// # Errors
///
/// When the store or the vault cannot be read.
pub fn all_rules(store: &Store, vault: &Vault) -> crate::Result<(Rules, BTreeMap<String, Rules>)> {
    let listed = reserved(store, vault)?;
    let devices: BTreeSet<&str> = listed
        .iter()
        .filter_map(|path| path.strip_prefix(MACHINES)?.split('/').next())
        .collect();
    let machines = devices
        .into_iter()
        .filter_map(|device| {
            rules_under(&listed, &machine_prefix(device)).map(|rules| (device.to_owned(), rules))
        })
        .collect();
    Ok((rules_under(&listed, ACCOUNT).unwrap_or_default(), machines))
}

/// A file in the account's trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// The file's path.
    pub path: String,
    /// When it is really deleted, in seconds since the epoch.
    pub due: u64,
    /// The device that deleted it, as hex.
    pub by: String,
}

fn parse_marker(marker: &str) -> Option<Trashed> {
    let rest = marker.strip_prefix(SHARED)?;
    let (due, rest) = rest.split_once('/')?;
    let (by, path) = rest.split_once('/')?;
    let due = due.parse().ok()?;
    (!path.is_empty() && !is_reserved(path)).then(|| Trashed {
        path: path.to_owned(),
        due,
        by: by.to_owned(),
    })
}

fn markers(store: &Store, vault: &Vault) -> crate::Result<Vec<(String, Trashed)>> {
    Ok(reserved(store, vault)?
        .into_iter()
        .filter_map(|marker| parse_marker(&marker).map(|trashed| (marker, trashed)))
        .collect())
}

/// The account's trash, by path. A file trashed twice apart keeps the later due.
///
/// # Errors
///
/// When the store or the vault cannot be read.
pub fn shared(store: &Store, vault: &Vault) -> crate::Result<BTreeMap<String, Trashed>> {
    let mut found: BTreeMap<String, Trashed> = BTreeMap::new();
    for (_, trashed) in markers(store, vault)? {
        match found.get(&trashed.path) {
            Some(known) if known.due >= trashed.due => {}
            _ => {
                found.insert(trashed.path.clone(), trashed);
            }
        }
    }
    Ok(found)
}

/// Put `path` in the account's trash for `days` from `now`.
///
/// # Errors
///
/// When `path` is one of the account's own files, or the store cannot be written.
pub fn trash_shared(store: &Store, path: &str, now: u64, days: u64) -> crate::Result<()> {
    if is_reserved(path) {
        return Err(StoreError::InvalidPath(
            path.to_owned(),
            "not a file a person can put in the trash",
        ));
    }
    let due = now.saturating_add(days.clamp(1, trash::MAX_DAYS).saturating_mul(trash::DAY));
    let marker = format!("{SHARED}{due}/{}/{path}", store.device_id().to_hex());
    store.write_file(&marker, b"")?;
    Ok(())
}

fn take_markers(store: &Store, vault: &Vault, path: &str) -> crate::Result<bool> {
    let mut any = false;
    for (marker, trashed) in markers(store, vault)? {
        if trashed.path == path {
            any |= crate::remove_known(store, vault, &marker)?;
        }
    }
    Ok(any)
}

/// Take `path` out of the account's trash: it shows again on every machine.
/// False when it was not there.
///
/// # Errors
///
/// When the store cannot be written.
pub fn restore_shared(store: &Store, vault: &Vault, path: &str) -> crate::Result<bool> {
    let removed = take_markers(store, vault, path)?;
    if removed {
        store.flush_segment()?;
    }
    Ok(removed)
}

/// Delete `path` from every machine for good: the file's tombstone first, the
/// marker after, so a machine that applies the tombstone still sees it came out
/// of the trash and protects nothing.
///
/// # Errors
///
/// When the store cannot be read or written.
pub fn purge(store: &Store, vault: &Vault, path: &str) -> crate::Result<bool> {
    let removed = crate::remove_known(store, vault, path)?;
    store.flush_segment()?;
    take_markers(store, vault, path)?;
    store.flush_segment()?;
    Ok(removed)
}

/// The account trash's files whose wait is over.
///
/// # Errors
///
/// When the store or the vault cannot be read.
pub fn due_shared(store: &Store, vault: &Vault, now: u64) -> crate::Result<Vec<String>> {
    Ok(shared(store, vault)?
        .into_values()
        .filter(|trashed| trashed.due <= now)
        .map(|trashed| trashed.path)
        .collect())
}

/// Note the account's trash, `path` -> due, for [`noted_shared`].
///
/// # Errors
///
/// When the note cannot be written.
pub fn note_shared(dir: &std::path::Path, shared: &BTreeMap<String, u64>) -> std::io::Result<()> {
    let mut text = String::new();
    for (path, due) in shared {
        text.push_str(&due.to_string());
        text.push('\t');
        text.push_str(path);
        text.push('\n');
    }
    trash::write_all(dir, NOTED, &text)
}

/// The account's trash as last noted, `path` -> due.
#[must_use]
pub fn noted_shared(dir: &std::path::Path) -> BTreeMap<String, u64> {
    let text = std::fs::read(dir.join(NOTED)).unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .filter_map(|line| {
            let (due, path) = line.split_once('\t')?;
            Some((path.to_owned(), due.parse().ok()?))
        })
        .collect()
}

/// What this machine protects: `path` -> (due, the entry it had).
#[must_use]
pub fn protected(dir: &std::path::Path) -> BTreeMap<String, (u64, FileEntry)> {
    std::fs::read(dir.join(PROTECTED))
        .ok()
        .and_then(|bytes| postcard::from_bytes(&bytes).ok())
        .unwrap_or_default()
}

fn save_protected(
    dir: &std::path::Path,
    kept: &BTreeMap<String, (u64, FileEntry)>,
) -> std::io::Result<()> {
    let bytes = postcard::to_allocvec(kept).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!("{PROTECTED}.new"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, dir.join(PROTECTED))
}

/// The account's own files this machine has in its index. A synced folder and
/// the sync engine have no vault: on a computer, which takes in the account,
/// the index holds every one of them.
fn known_here(store: &Store) -> crate::Result<Vec<String>> {
    Ok(store
        .list()?
        .into_iter()
        .filter(|path| is_reserved(path))
        .collect())
}

/// [`rules_here`], from the index alone.
///
/// # Errors
///
/// When the store cannot be read.
pub fn rules_known_here(store: &Store) -> crate::Result<Rules> {
    let reserved = known_here(store)?;
    Ok(
        rules_under(&reserved, &machine_prefix(&store.device_id().to_hex()))
            .or_else(|| rules_under(&reserved, ACCOUNT))
            .unwrap_or_default(),
    )
}

/// [`set_rules_here`], from the index alone: for a machine
/// that takes in the account, where every older record is in the index.
///
/// # Errors
///
/// When the store cannot be written.
pub fn set_rules_known(store: &Store, rules: Option<&Rules>) -> crate::Result<()> {
    let prefix = machine_prefix(&store.device_id().to_hex());
    for path in known_here(store)? {
        if path.starts_with(&prefix) {
            store.remove_file(&path)?;
        }
    }
    if let Some(rules) = rules {
        store.write_file(&format!("{prefix}{}", rules.to_word()), b"")?;
    }
    store.flush_segment()?;
    Ok(())
}

/// [`shared`], from the index alone.
///
/// # Errors
///
/// When the store cannot be read.
pub fn shared_known_here(store: &Store) -> crate::Result<BTreeMap<String, Trashed>> {
    let mut found: BTreeMap<String, Trashed> = BTreeMap::new();
    for trashed in known_here(store)?
        .iter()
        .filter_map(|marker| parse_marker(marker))
    {
        if found
            .get(&trashed.path)
            .is_none_or(|known| known.due < trashed.due)
        {
            found.insert(trashed.path.clone(), trashed);
        }
    }
    Ok(found)
}

/// [`restore_shared`], from the index alone; the caller seals
/// the segment.
///
/// # Errors
///
/// When the store cannot be written.
pub fn restore_shared_known(store: &Store, path: &str) -> crate::Result<bool> {
    let mut any = false;
    for marker in known_here(store)? {
        if parse_marker(&marker).is_some_and(|trashed| trashed.path == path) {
            any |= store.remove_file(&marker)?;
        }
    }
    Ok(any)
}

/// The chunks a protected entry still needs: garbage collection keeps them.
#[must_use]
pub fn protected_chunks(dir: &std::path::Path) -> BTreeSet<ChunkId> {
    protected(dir)
        .into_values()
        .flat_map(|(_, entry)| entry.chunks)
        .collect()
}

/// Whether this machine's own rules, as far as its index knows them, protect.
/// The sync engine has no vault: it reads what is downloaded here, which on a
/// computer is the account; a machine that knows no rules protects.
fn protects_here(reserved: &[String], store: &Store) -> (bool, u64) {
    let rules = rules_under(reserved, &machine_prefix(&store.device_id().to_hex()))
        .or_else(|| rules_under(reserved, ACCOUNT))
        .unwrap_or_default();
    (rules.receive == Receive::Protect, rules.days)
}

/// Before `operation` from `device` is applied here: when it deletes outright a
/// file this machine holds, and this machine protects, keep its entry for its
/// days. A delete out of the shared trash, or this machine's own, protects
/// nothing.
///
/// # Errors
///
/// When the store cannot be read, or the record cannot be written.
pub fn protect(
    store: &Store,
    device: &DeviceId,
    operation: &Operation,
    now: u64,
) -> crate::Result<()> {
    let Operation::Remove { path, .. } = operation else {
        return Ok(());
    };
    if *device == store.device_id() || is_reserved(path) {
        return Ok(());
    }
    let reserved = known_here(store)?;
    let (protects, days) = protects_here(&reserved, store);
    let trashed = reserved
        .iter()
        .filter_map(|marker| parse_marker(marker))
        .any(|trashed| trashed.path == *path);
    if !protects || trashed {
        return Ok(());
    }
    let Some(entry) = store.stat(path)? else {
        return Ok(());
    };
    let mut kept = protected(store.root());
    let due = now.saturating_add(days.clamp(1, trash::MAX_DAYS).saturating_mul(trash::DAY));
    kept.insert(path.clone(), (due, entry));
    save_protected(store.root(), &kept).map_err(|error| StoreError::io(path, error))
}

/// Publish a protected file again, from this machine: every machine has it back.
/// False when it was not protected here.
///
/// # Errors
///
/// When the store cannot be written.
pub fn restore_protected(store: &Store, path: &str) -> crate::Result<bool> {
    let mut kept = protected(store.root());
    let Some((_, entry)) = kept.remove(path) else {
        return Ok(false);
    };
    // Still a file here (the delete lost to an edit it did not see): nothing to
    // bring back, and publishing it again would write a version for nothing.
    if store.stat(path)?.is_none() {
        store.republish(path, &entry)?;
        store.flush_segment()?;
    }
    save_protected(store.root(), &kept).map_err(|error| StoreError::io(path, error))?;
    Ok(true)
}

/// Let go of protected files: those named, or every one whose days are over.
///
/// # Errors
///
/// When the record cannot be written.
pub fn release_protected(
    dir: &std::path::Path,
    paths: Option<&[String]>,
    now: u64,
) -> std::io::Result<usize> {
    let mut kept = protected(dir);
    let before = kept.len();
    kept.retain(|path, (due, _)| match paths {
        Some(named) => !named.iter().any(|name| name == path),
        None => *due > now,
    });
    let gone = before - kept.len();
    if gone > 0 {
        save_protected(dir, &kept)?;
    }
    Ok(gone)
}
