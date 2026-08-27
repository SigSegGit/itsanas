//! The decision: given a file's three states, what should happen?
//!
//! Every path has three independent stories about it:
//!
//! * what is **on disk** right now,
//! * what the **store** says the file is, and
//! * what this device **last put on disk** (the ledger).
//!
//! The ledger is what turns two of those into a direction. "On disk and not in
//! the store" means nothing on its own — it could be a file the user just
//! created, or one that a peer deleted while this machine was off. Comparing
//! both against what this device last saw is what says which.
//!
//! Kept as a pure function of three hashes so that every branch — including the
//! ones that are hard to stage on a real filesystem — is an ordinary unit test.

/// What reconciliation should do about one path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Disk, store and ledger already agree.
    Nothing,
    /// The three disagree only about bookkeeping: disk and store hold the same
    /// content and the ledger is stale or missing. Fix the ledger, touch
    /// nothing else.
    RecordOnly,
    /// The file changed on disk. Put it in the store.
    Import,
    /// The file was deleted from disk. Delete it from the store.
    RemoveFromStore,
    /// The store has content this disk does not. Write it out.
    Export,
    /// The store says this file is gone. Remove it from disk.
    DeleteFromDisk,
    /// Both sides changed, differently. Keep both.
    KeepBoth,
}

/// Decide what to do about one path.
///
/// Each argument is the content hash of that view, or `None` if the file is
/// absent from it — deleted from disk, absent from the store, or never recorded.
#[must_use]
pub fn decide(
    on_disk: Option<[u8; 32]>,
    in_store: Option<[u8; 32]>,
    ledger: Option<[u8; 32]>,
) -> Decision {
    let local_changed = on_disk != ledger;
    let remote_changed = in_store != ledger;

    match (local_changed, remote_changed) {
        (false, false) => Decision::Nothing,

        (true, false) => match on_disk {
            Some(_) => Decision::Import,
            None => Decision::RemoveFromStore,
        },

        (false, true) => match in_store {
            Some(_) => Decision::Export,
            None => Decision::DeleteFromDisk,
        },

        (true, true) => {
            if on_disk == in_store {
                // Both sides moved to the same place independently — the same
                // edit made twice, or a file added by hand that a peer already
                // had. Re-uploading it would be pure waste.
                return Decision::RecordOnly;
            }

            match (on_disk, in_store) {
                // A delete that raced an edit. The edit wins, for the same
                // reason it does in the sync engine: an unexpected file costs a
                // second to delete again, and a lost edit is unrecoverable.
                (None, Some(_)) => Decision::Export,
                (Some(_), None) => Decision::Import,
                // Genuinely different content on both sides.
                _ => Decision::KeepBoth,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 32] = [0xAA; 32];
    const B: [u8; 32] = [0xBB; 32];

    #[test]
    fn nothing_to_do_when_all_three_agree() {
        assert_eq!(decide(Some(A), Some(A), Some(A)), Decision::Nothing);
        assert_eq!(decide(None, None, None), Decision::Nothing);
    }

    #[test]
    fn a_new_local_file_is_imported() {
        // On disk, nothing in the store, never seen before.
        assert_eq!(decide(Some(A), None, None), Decision::Import);
    }

    #[test]
    fn an_edited_local_file_is_imported() {
        assert_eq!(decide(Some(B), Some(A), Some(A)), Decision::Import);
    }

    #[test]
    fn a_file_the_user_deleted_is_removed_from_the_store() {
        // Gone from disk, and this device is the one that put it there.
        assert_eq!(decide(None, Some(A), Some(A)), Decision::RemoveFromStore);
    }

    #[test]
    fn a_file_that_was_never_downloaded_is_exported_not_deleted() {
        // The single most dangerous confusion in the whole design. This device
        // has never had the file, so its absence means nothing — and treating
        // it as a deletion would announce the removal of every file the user
        // owns from a machine that had simply not synced yet.
        assert_eq!(
            decide(None, Some(A), None),
            Decision::Export,
            "a never-downloaded file was mistaken for a deleted one"
        );
    }

    #[test]
    fn a_remotely_deleted_file_is_removed_from_disk() {
        assert_eq!(decide(Some(A), None, Some(A)), Decision::DeleteFromDisk);
    }

    #[test]
    fn a_remote_edit_is_written_out() {
        assert_eq!(decide(Some(A), Some(B), Some(A)), Decision::Export);
    }

    #[test]
    fn two_different_edits_keep_both() {
        assert_eq!(decide(Some(A), Some(B), None), Decision::KeepBoth);
        assert_eq!(
            decide(Some(B), Some(A), Some([0xCC; 32])),
            Decision::KeepBoth
        );
    }

    #[test]
    fn the_same_edit_made_twice_is_not_a_conflict() {
        // Both sides changed, but to the same content. Producing a conflict
        // sibling here would litter the folder for no reason, and re-uploading
        // would be pure waste.
        assert_eq!(decide(Some(A), Some(A), Some(B)), Decision::RecordOnly);
        assert_eq!(decide(Some(A), Some(A), None), Decision::RecordOnly);
    }

    #[test]
    fn a_delete_racing_an_edit_brings_the_file_back() {
        // Deleted here, edited elsewhere, and this device never saw the edit.
        // The edit wins, matching the sync engine: an unexpected file costs a
        // second to delete, a lost edit is unrecoverable.
        assert_eq!(
            decide(None, Some(B), Some(A)),
            Decision::Export,
            "a local delete destroyed a remote edit it never saw"
        );
    }

    #[test]
    fn an_edit_racing_a_delete_keeps_the_edit() {
        assert_eq!(
            decide(Some(B), None, Some(A)),
            Decision::Import,
            "a remote delete destroyed a local edit it never saw"
        );
    }

    #[test]
    fn both_sides_deleting_agrees() {
        // Gone from disk and gone from the store, but the ledger still lists
        // it. Only the bookkeeping needs fixing.
        assert_eq!(decide(None, None, Some(A)), Decision::RecordOnly);
    }

    #[test]
    fn a_stale_ledger_alone_never_moves_data() {
        // Whatever the ledger says, if disk and store agree the answer is
        // always bookkeeping — never an upload, a download or a delete.
        for ledger in [None, Some(A), Some(B)] {
            let decision = decide(Some(A), Some(A), ledger);
            assert!(
                matches!(decision, Decision::Nothing | Decision::RecordOnly),
                "a stale ledger caused {decision:?} when disk and store agreed"
            );
        }
    }

    #[test]
    fn no_input_combination_panics_or_is_undecided() {
        // Exhaustive over the shape of the problem: three views, each either
        // absent or holding one of two distinct contents.
        let values = [None, Some(A), Some(B)];
        let mut seen = 0;

        for disk in values {
            for store in values {
                for ledger in values {
                    let _ = decide(disk, store, ledger);
                    seen += 1;
                }
            }
        }

        assert_eq!(seen, 27, "the case matrix is not what it looks like");
    }

    #[test]
    fn deleting_from_the_store_only_ever_follows_a_recorded_local_file() {
        // The guard on the most destructive action. RemoveFromStore must be
        // unreachable unless the ledger says this device genuinely had the
        // file on disk.
        let values = [None, Some(A), Some(B)];

        for disk in values {
            for store in values {
                for ledger in values {
                    if decide(disk, store, ledger) == Decision::RemoveFromStore {
                        assert!(
                            disk.is_none() && ledger.is_some(),
                            "a store deletion was decided for disk={disk:?} \
                             ledger={ledger:?}, which does not describe a file \
                             this device ever had"
                        );
                    }
                }
            }
        }
    }
}
