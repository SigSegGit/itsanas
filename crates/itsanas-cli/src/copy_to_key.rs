//! `itsanas copy-to-key DIR`: this account on a USB key, ready to start on
//! any computer it is plugged into.
//!
//! A local copy, with no coordinator in it (Nicolas, 2026-10-22): the key is
//! a portable copy (`portable.rs`, every system's program and a client-only
//! home) whose home already holds the account, sealed under a passphrase of
//! its own, with a device of its own. In this order, and nothing written on
//! the key before the passphrases and the room are checked:
//!
//! 1. the target: a full path, not inside this account's home, vault or
//!    synced folder, and not a key that already holds an account -- never
//!    overwritten;
//! 2. this machine's passphrase, which reads the secrets from the keystore
//!    (the store stays the daemon's);
//! 3. a new passphrase for the key, at least [`MIN_LENGTH`] characters and
//!    neither this machine's nor a trivially weak one ([`weak_passphrase`]);
//! 4. room on the drive for the programs and the account's files ([`room`]);
//! 5. the portable copy, then the account in its home: the master only, a
//!    fresh device, the coordinator, the synced folder `ITSaNAS/Fichiers`;
//! 6. enrolment now if the coordinator answers, or at the key's first start
//!    ([`enrol_at_start`]), said either way.
//!
//! Each step is written to `<home>/copy-to-key.status` ([`STATUS`]), so a run
//! in the background (`--detach`) can be followed by the Settings page.
//!
//! # What the key is, said every time
//!
//! Its keystore holds the account's master secret. Whoever has the key and
//! its passphrase **is** the account; removing the key from the device list
//! stops the other machines dialling it, not its holder reading and writing.
//! A device given device and data keys without the master is the real fix
//! (ROADMAP).

use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

use itsanas_node::node::Identity;

/// The most live devices an account may have: the key is one more.
const MAX_DEVICES: usize = itsanas_coord::MAX_DEVICES_PER_ACCOUNT;

use crate::{
    config::Config,
    error::{CliError, Result},
    node::Node,
    portable,
    setup::Secret,
};

mod command;
mod progress;
mod rules;

pub(crate) use command::{command, start_detached};
use progress::Status;
pub(crate) use progress::{forget_progress, progress};
pub(crate) use rules::{assess, content_of, programs_bytes};
use rules::{check_target, room, weak_passphrase};
#[cfg(test)]
use {
    command::read_passphrases,
    rules::{content_in, refusal},
};

/// The progress file, in the home of the account being copied.
pub(crate) const STATUS: &str = "copy-to-key.status";

/// The shortest passphrase a key gets. The key's keystore is the account,
/// and a lost key is an offline attack on it that Argon2 slows and does not
/// stop.
pub(crate) const MIN_LENGTH: usize = 12;

/// The synced folder on the key, beside its home.
pub(crate) const FOLDER: &str = "Fichiers";

/// The flag the background copy is started with: its two passphrases come
/// on standard input, never in its argv or environment.
pub(crate) const ON_STDIN: &str = "--passphrases-on-stdin";

/// What the window asks before the key's passphrase.
pub(crate) const KEY_PURPOSE: &str = "A NEW passphrase for the USB key, at least 12 characters. \
     Whoever opens this key with it IS your account: they can read and change all your files, \
     and removing the key from your devices later does not lock them out. Choose a long one \
     that you use nowhere else, and not this machine's.";

/// The two passphrases of a copy, wiped when dropped.
pub(crate) struct Passphrases {
    /// This machine's: it opens the keystore the secrets are read from.
    pub(crate) machine: Secret,
    /// The key's: its keystore is sealed under it.
    pub(crate) key: Secret,
}

impl std::fmt::Debug for Passphrases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrases(redacted)")
    }
}

// ---------------------------------------------------------------------------
// The copy
// ---------------------------------------------------------------------------

/// Copy the account at `home` to `<dir>/ITSaNAS`, the passphrases already
/// asked; what to tell the person. Every check is made again here, before
/// anything is written: a background run trusts nothing its parent checked.
///
/// `programs` is the release's size when known, `fetch` puts the programs
/// in `bin/` ([`portable::make_with`]), `register` enrols the key's node.
///
/// # Errors
///
/// Any refusal of the module's list, or the disk. A failed enrolment is not
/// one: the key enrols itself at its first start.
pub(crate) fn copy(
    home: &Path,
    dir: &Path,
    passes: &Passphrases,
    programs: Option<u64>,
    fetch: impl FnOnce(&Path) -> Result<Vec<String>>,
    register: impl FnOnce(&Node) -> Result<()>,
    places: impl FnOnce(&Identity) -> Result<Option<usize>>,
) -> Result<String> {
    let result = copy_steps(home, dir, passes, programs, fetch, (register, places));
    Status(home).ended(&result);
    result
}

/// Whether the account has a place for the key, before a byte is written,
/// and how many are left after it. The key is a device of its own, and an
/// account holds at most `MAX_DEVICES_PER_ACCOUNT`: on a full account it used
/// to be written whole -- the programs, the sealed account -- and then refused
/// at enrolment. A coordinator that cannot be asked is said, not a refusal
/// (the key enrols itself at its first start).
fn places_left(
    home: &Path,
    passes: &Passphrases,
    places: impl FnOnce(&Identity) -> Result<Option<usize>>,
) -> Result<String> {
    Ok(
        match Identity::open(home, &passes.machine).and_then(|me| places(&me)) {
            Ok(Some(live)) if live >= MAX_DEVICES => {
                return Err(CliError::Usage(format!(
                    "the account already has {live} devices, the most it may have, and the key would be one more: withdraw one first (itsanas device list, then itsanas device withdraw). Nothing was written"
                )));
            }
            Ok(Some(live)) => format!(
                "The key takes one of the account's {MAX_DEVICES} device places; {} left after it",
                MAX_DEVICES - live - 1
            ),
            Ok(None) => "The coordinator could not count the account's devices; the key takes one place when it enrols"
                .to_owned(),
            Err(error) => format!(
                "The account's device places could not be checked ({error}); the key takes one when it enrols"
            ),
        },
    )
}

/// [`copy`], before its end is recorded.
fn copy_steps(
    home: &Path,
    dir: &Path,
    passes: &Passphrases,
    programs: Option<u64>,
    fetch: impl FnOnce(&Path) -> Result<Vec<String>>,
    (register, places): (
        impl FnOnce(&Node) -> Result<()>,
        impl FnOnce(&Identity) -> Result<Option<usize>>,
    ),
) -> Result<String> {
    let status = Status(home);
    status.step("checking", "checking the key");
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let config = Config::load(&Node::config_path(home))?;
    let root = check_target(home, &config, dir)?;

    status.step("passphrase", "opening this machine's keys");
    let secrets = Identity::secrets(home, &passes.machine)?;
    if let Some(why) = weak_passphrase(&passes.key, &passes.machine, &config.username) {
        return Err(CliError::Usage(format!(
            "the key's passphrase {why}. Nothing was written"
        )));
    }

    status.step("places", "checking the account has a place for the key");
    let places_said = places_left(home, passes, places)?;

    status.step("space", "checking the room on the key");
    let content = content_of(home);
    let counted =
        room(programs, content, crate::drives::free_bytes(dir)).map_err(CliError::Usage)?;

    // One copy at a time on a key, for the whole copy: a second run (a
    // second page, a second terminal) would race this one to the keystore.
    portable::no_link(&root)?;
    std::fs::create_dir_all(&root).map_err(|source| CliError::Io {
        path: root.clone(),
        source,
    })?;
    let _lock = CopyLock::take(&root)?;
    // Again, under the lock: a copy that finished meanwhile left an account.
    let root = check_target(home, &config, dir)?;
    let key_home = root.join("home");
    // Nothing written through a link on the key: what `make_with` writes it
    // checks itself; the home's own entries and the synced folder here.
    portable::no_links_in(&key_home)?;
    portable::no_link(&root.join(FOLDER))?;

    status.step(
        "programs",
        "writing the programs for every system on the key",
    );
    let made = portable::make_with(dir, fetch)?;

    status.step(
        "account",
        "sealing the account on the key under its own passphrase",
    );
    let before = entries(&key_home);
    let mut node =
        Node::restore_from_secrets(&key_home, &passes.key, &config.username, secrets.as_bytes())?;
    drop(secrets);
    let folder = root.join(FOLDER);
    if let Err(error) = settle(&mut node, &config, &folder) {
        // What this run wrote in the home goes -- the keystore, the config,
        // the store and the vault of a device that will never run -- so no
        // sealed master is left behind a failure and a retry starts clean.
        drop(node);
        let removed = entries(&key_home)
            .difference(&before)
            .all(|name| remove_entry(&key_home.join(name)));
        let after = if removed {
            "the account written on the key by this run was removed, so it can be tried again"
                .to_owned()
        } else {
            format!(
                "the account written on the key by this run could not all be removed: delete \
                 {} before trying again",
                key_home.display()
            )
        };
        return Err(CliError::Usage(format!("{error}; {after}")));
    }

    status.step("enrolling", "enrolling the key with the coordinator");
    let enrolled = match &node.config.coordinator {
        None => "No coordinator is set, so it finds your other machines on the same network \
                 only"
            .to_owned(),
        Some(address) => match register(&node) {
            Ok(()) => format!("It is enrolled with {address} as a device of its own"),
            Err(error) => format!(
                "It is not enrolled yet ({error}): it enrols itself the first time it is \
                 started, with nothing to do"
            ),
        },
    };

    Ok(format!(
        "The key is ready, in {root}, with its own device {device}.\n\
         {enrolled}.\n\
         {places_said}.\n\
         {made}.\n\
         {counted}.\n\
         To use it: on Windows double-click ITSaNAS.cmd; on a Mac double-click \
         itsanas.command; on Linux run `sh itsanas.sh`. It asks the key's passphrase, not this \
         machine's, and your files appear in {folder}.\n\
         Whoever has this key and its passphrase has your account: removing it from your \
         devices does not lock them out. Keep it like a house key.",
        root = root.display(),
        device = node.store.device_id(),
        folder = folder.display(),
    ))
}

/// The names directly in `dir`; none when it cannot be read.
fn entries(dir: &Path) -> std::collections::BTreeSet<std::ffi::OsString> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name())
        .collect()
}

/// Remove a file or a folder this run made; a link only as a link.
fn remove_entry(path: &Path) -> bool {
    let removed = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => true,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// The key's node, past its keystore: the coordinator, the synced folder
/// beside the home, a listening port of its own.
fn settle(node: &mut Node, config: &Config, folder: &Path) -> Result<()> {
    node.config.coordinator.clone_from(&config.coordinator);
    node.config
        .coordinator_device
        .clone_from(&config.coordinator_device);
    node.save_config()?;
    std::fs::create_dir_all(folder).map_err(|source| CliError::Io {
        path: folder.to_path_buf(),
        source,
    })?;
    crate::set_folder(node, folder)?;
    crate::settle_listen_port(node)
}

/// The name of the lock a copy holds in `<key>/ITSaNAS`.
pub(crate) const LOCK: &str = ".copy-lock";

/// `<key>/ITSaNAS/.copy-lock`, made with `create_new` and held for the
/// whole copy; removed when dropped, whether the copy worked or not.
struct CopyLock(PathBuf);

impl CopyLock {
    fn take(root: &Path) -> Result<Self> {
        let path = root.join(LOCK);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                Ok(Self(path))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(CliError::Usage(format!(
                    "another copy to this key is under way; nothing was written. If none is \
                     (a copy stopped by a crash), remove {} and try again",
                    path.display()
                )))
            }
            Err(source) => Err(CliError::Io { path, source }),
        }
    }
}

impl Drop for CopyLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// At a daemon's start: a node with a coordinator and no `registered`
/// marker -- a key that could not enrol when it was made -- enrols once,
/// through `register`, and writes the marker. Failing, it carries on
/// unenrolled and tries again at the next start. What to print, `None` when
/// there was nothing to do.
pub(crate) fn enrol_at_start(
    node: &Node,
    register: impl FnOnce(&Node) -> Result<()>,
) -> Option<String> {
    let address = node.config.coordinator.as_deref()?;
    let marker = node.home.join(crate::REGISTERED);
    if marker.exists() {
        return None;
    }
    Some(match register(node) {
        Ok(()) => {
            let _ = std::fs::write(&marker, address);
            format!("  enrolled  with {address}, at this first start")
        }
        Err(error) => format!(
            "  enrolled  not yet ({error}); it is tried again at the next start, and the \
             machines on this network sync meanwhile"
        ),
    })
}

#[cfg(test)]
mod tests;
