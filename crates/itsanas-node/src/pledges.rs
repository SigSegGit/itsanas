//! What the other nodes on this disk have promised, so two of them cannot
//! promise the same free space.
//!
//! Found on the Freebox VM (ROADMAP, "Pledges are not reserved"): two
//! accounts each pledged 30 GB on a disk that could not hold 60, because each
//! compared its pledge with the free space alone. Free space only shrinks once
//! the chunks arrive, so both promises looked kept until one could not be.
//!
//! Every node publishes a small `pledge.state` (its pledge and what it holds
//! for others) in its home, and in a shared directory when
//! [`SHARED_DIR_ENV`] names one -- the Docker clients mount one directory into
//! every project, since one container cannot see another's home. A pledge is
//! then compared with the free space plus what this node holds, minus what the
//! others still owe. Reading a file, not the neighbour's store: the store is
//! locked by its daemon.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

/// The file, in a home and (as `<device>.state`) in the shared directory.
pub const STATE_FILE: &str = "pledge.state";

/// A directory the nodes on one disk share across containers.
pub const SHARED_DIR_ENV: &str = "ITSANAS_PLEDGES_DIR";

/// A shared entry older than this is not counted, and is named: a container
/// removed without its entry would otherwise hold the disk for ever.
pub const STALE_AFTER_SECS: u64 = 7 * 24 * 3600;

/// What one node says it promised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promise {
    /// Where it was read, said back to the person when it is in the way.
    pub source: String,
    /// Bytes pledged.
    pub pledge: u64,
    /// Bytes already held for other accounts (already off the free space).
    pub held: u64,
    /// Unix seconds of the last write.
    pub updated: u64,
}

impl Promise {
    /// What this promise still needs from the free space.
    #[must_use]
    pub fn owed(&self) -> u64 {
        self.pledge.saturating_sub(self.held)
    }
}

fn render(pledge: u64, held: u64, now: u64) -> String {
    format!("pledge={pledge}\nheld={held}\nupdated={now}\n")
}

fn parse(text: &str, source: String) -> Option<Promise> {
    let mut pledge = None;
    let mut held = None;
    let mut updated = None;
    for line in text.lines() {
        let (key, value) = line.split_once('=')?;
        let value: u64 = value.trim().parse().ok()?;
        match key.trim() {
            "pledge" => pledge = Some(value),
            "held" => held = Some(value),
            "updated" => updated = Some(value),
            _ => {}
        }
    }
    Some(Promise {
        source,
        pledge: pledge?,
        held: held?,
        updated: updated?,
    })
}

/// The shared directory, if one is configured.
#[must_use]
pub fn shared_dir() -> Option<PathBuf> {
    std::env::var_os(SHARED_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Publish this node's promise. `device` names its shared entry.
///
/// # Errors
///
/// The filesystem's, for the home's copy; the shared copy is best effort.
pub fn publish(
    home: &Path,
    shared: Option<&Path>,
    device: &str,
    pledge: u64,
    held: u64,
    now: u64,
) -> std::io::Result<()> {
    let text = render(pledge, held, now);
    std::fs::write(home.join(STATE_FILE), &text)?;
    if let Some(dir) = shared {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(format!("{device}.state")), &text);
    }
    Ok(())
}

/// The others' promises on `home`'s disk, and the shared entries skipped as
/// stale (named, so the refusal can say why the space looks taken or not).
///
/// Siblings are the other homes beside this one ([`crate::config::node_homes`]
/// of its parent) on the same volume; an unknown volume counts as the same,
/// which errs towards refusing a promise, never towards a double one.
#[must_use]
pub fn others(
    home: &Path,
    shared: Option<&Path>,
    device: &str,
    now: u64,
) -> (Vec<Promise>, Vec<String>) {
    let mut found = Vec::new();
    let mut stale = Vec::new();
    let me = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_owned());
    if let Some(base) = home.parent() {
        for (name, sibling) in crate::config::node_homes(base) {
            let same = std::fs::canonicalize(&sibling).unwrap_or_else(|_| sibling.clone());
            if same == me || !crate::node::same_volume(home, &sibling).unwrap_or(true) {
                continue;
            }
            if let Some(promise) = std::fs::read_to_string(sibling.join(STATE_FILE))
                .ok()
                .and_then(|text| parse(&text, format!("instance {name}")))
            {
                found.push(promise);
            }
        }
    }
    if let Some(dir) = shared {
        let mine = format!("{device}.state");
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if file == mine
                || !Path::new(&file)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("state"))
            {
                continue;
            }
            let short: String = file.chars().take(8).collect();
            let Some(promise) = std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|text| parse(&text, format!("node {short}... (shared)")))
            else {
                continue;
            };
            if now.saturating_sub(promise.updated) > STALE_AFTER_SECS {
                stale.push(promise.source);
            } else if !found.iter().any(|f| {
                f.pledge == promise.pledge && f.held == promise.held && f.updated == promise.updated
            }) {
                found.push(promise);
            }
        }
    }
    (found, stale)
}

/// The most this node may pledge: free space, plus what it already holds for
/// others (that is already off the free space), minus what the others still
/// owe.
#[must_use]
pub fn room(free: u64, my_held: u64, others: &[Promise]) -> u64 {
    let owed = others
        .iter()
        .fold(0u64, |total, p| total.saturating_add(p.owed()));
    free.saturating_add(my_held).saturating_sub(owed)
}

/// Kept free on the disk whatever the pledge says: a disk filled to the last
/// byte breaks the node's own index writes, and everything else on the machine.
pub const RESERVE: u64 = 256 * 1024 * 1024;

/// What can still be honoured of `pledge` with `free` bytes left: a host that
/// takes chunks its disk cannot hold fails the person who trusted it, so it
/// stops taking them and says so (`status`), rather than failing a write.
/// An unknown free space keeps the pledge as it is.
#[must_use]
pub fn keepable(pledge: u64, held: u64, free: Option<u64>) -> u64 {
    match free {
        None => pledge,
        Some(free) => pledge.min(held.saturating_add(free.saturating_sub(RESERVE))),
    }
}

/// The others, one line each, for a refusal.
#[must_use]
pub fn describe(others: &[Promise], stale: &[String], size: impl Fn(u64) -> String) -> String {
    let mut out = String::new();
    for promise in others {
        let _ = write!(
            out,
            "\n  {} pledged {} and holds {}",
            promise.source,
            size(promise.pledge),
            size(promise.held)
        );
    }
    for source in stale {
        let _ = write!(out, "\n  {source} not counted: not updated for over 7 days");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(base: &Path, name: &str) -> PathBuf {
        let home = base.join(name);
        std::fs::create_dir_all(&home).unwrap();
        // What `Node::exists` looks for.
        std::fs::write(home.join("keystore.bin"), b"x").unwrap();
        home
    }

    #[test]
    fn red_team_a_sibling_promise_on_the_same_disk_is_counted() {
        let dir = tempfile::tempdir().unwrap();
        let a = home(dir.path(), ".itsanas-a");
        let b = home(dir.path(), ".itsanas-b");
        publish(&a, None, "aa", 30, 0, 100).unwrap();
        let (found, _) = others(&b, None, "bb", 100);
        assert_eq!(
            room(40, 0, &found),
            10,
            "a second account on one disk saw the first one's 30 as free: both promised the \
             same space, and one of them fails once the chunks arrive"
        );
    }

    #[test]
    fn red_team_a_shared_entry_from_another_container_is_counted_and_a_stale_one_named() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("pledges");
        let a = home(dir.path(), "a-home");
        let b = home(dir.path(), "b-home");
        publish(&a, Some(&shared), "aa", 30, 5, 100).unwrap();
        let (found, stale) = others(&b, Some(&shared), "bb", 100);
        assert_eq!(
            room(40, 0, &found),
            15,
            "a Docker client could not see its neighbour's promise: containers do not share homes"
        );
        assert!(stale.is_empty());

        let (found, stale) = others(&b, Some(&shared), "bb", 100 + STALE_AFTER_SECS + 1);
        assert!(
            found.is_empty() && stale.len() == 1,
            "an entry left by a removed container held the disk for ever, or vanished unsaid"
        );
    }

    #[test]
    fn red_team_a_disk_that_filled_up_stops_taking_chunks_before_the_last_byte() {
        let gb = 1024 * 1024 * 1024;
        assert_eq!(
            keepable(30 * gb, 10 * gb, Some(5 * gb)),
            15 * gb - RESERVE,
            "a host kept accepting chunks its disk could not hold, up to the pledge"
        );
        assert_eq!(keepable(30 * gb, 0, Some(100 * gb)), 30 * gb);
        assert_eq!(keepable(30 * gb, 0, None), 30 * gb);
    }

    #[test]
    fn its_own_entries_are_not_counted_against_itself() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("pledges");
        let a = home(dir.path(), ".itsanas");
        publish(&a, Some(&shared), "aa", 30, 0, 100).unwrap();
        let (found, _) = others(&a, Some(&shared), "aa", 100);
        assert!(
            found.is_empty(),
            "a node counted its own pledge twice and could not keep it"
        );
    }
}
