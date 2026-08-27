//! What this device last saw on disk, for each path.
//!
//! Without this, a synced folder cannot tell the two most important cases
//! apart:
//!
//! * a file is **missing from disk because the user deleted it**, which must
//!   propagate as a delete to every other device, and
//! * a file is **missing from disk because it was never downloaded**, which must
//!   propagate as nothing at all and be materialised instead.
//!
//! Both look identical from the filesystem — the file is not there. The only
//! thing that separates them is whether this device ever put it there. Guessing
//! wrong in one direction resurrects deleted files forever; guessing wrong in
//! the other silently deletes a user's data across every machine they own,
//! because a device that had simply never synced would announce the deletion of
//! everything.
//!
//! So the ledger is not an optimisation. It is the record that makes a delete
//! safe to act on.

use serde::{Deserialize, Serialize};

/// The state of one file on disk, as this device last observed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalState {
    /// Size in bytes when last observed.
    pub size: u64,
    /// Modification time when last observed, seconds since the Unix epoch.
    pub modified_unix: u64,
    /// BLAKE3 of the content when last observed.
    ///
    /// The authority. Size and mtime are a fast pre-filter; this is what
    /// actually decides whether a file changed.
    pub content_hash: [u8; 32],
}

impl LocalState {
    /// Whether a file with this size and mtime is *probably* unchanged.
    ///
    /// A pre-filter, not a decision. Re-hashing every file on every scan would
    /// make a 100 GB folder unusable to watch, so a matching size and mtime is
    /// taken as "unchanged" — the same trade rsync and Syncthing make.
    ///
    /// The gap it leaves is real and worth naming: a file edited **within the
    /// same second** as its previous write, ending at **exactly the same
    /// size**, is missed. In practice that needs a program writing twice in one
    /// second without changing the length. A periodic deep scan closes it, and
    /// the daemon runs one.
    #[must_use]
    pub const fn probably_matches(&self, size: u64, modified_unix: u64) -> bool {
        self.size == size && self.modified_unix == modified_unix
    }

    /// Whether the content genuinely differs.
    #[must_use]
    pub fn differs_from(&self, content_hash: &[u8; 32]) -> bool {
        self.content_hash != *content_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> LocalState {
        LocalState {
            size: 100,
            modified_unix: 1_700_000_000,
            content_hash: [7; 32],
        }
    }

    #[test]
    fn the_fast_path_accepts_an_identical_size_and_mtime() {
        assert!(state().probably_matches(100, 1_700_000_000));
    }

    #[test]
    fn a_changed_size_or_mtime_forces_a_rehash() {
        assert!(!state().probably_matches(101, 1_700_000_000));
        assert!(!state().probably_matches(100, 1_700_000_001));
    }

    #[test]
    fn the_hash_is_what_actually_decides() {
        // The pre-filter can say "probably changed" and the hash can still say
        // "no it did not" — a touched file, or one saved with identical
        // content. Acting on the pre-filter alone would re-upload it.
        assert!(!state().differs_from(&[7; 32]));
        assert!(state().differs_from(&[8; 32]));
    }

    #[test]
    fn it_round_trips_through_the_index_encoding() {
        let encoded = postcard::to_stdvec(&state()).unwrap();
        assert_eq!(
            postcard::from_bytes::<LocalState>(&encoded).unwrap(),
            state()
        );
    }
}
