//! The synced folder: opening it, reconciling it with the store, and waiting
//! for it to change or for the next round to be due.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_folder::{Folder, Watcher, watch};
use itsanas_net::session;

use super::{SETTLE_LIMIT, SHUTDOWN_POLL, round::report_drain_refusals};
use crate::{error::Result, node::Node};

/// Take in what changed: the folder (or, with none, the disk's room), then
/// what peers pushed into the vault. Not called at all while paused -- a
/// `match` arm that ignored a result computed anyway was the first version of
/// this guard, and it drained the vault of a paused node (#243's review).
///
/// Whether the folder changed here: files to send out.
pub(super) fn take_in(node: &Node, folder: Option<&(Folder, Option<Watcher>)>, deep: bool) -> bool {
    let mut changed = false;
    // What waited its days in the trash is deleted now, from every machine.
    if let Err(error) = crate::phone::purged_trash(node) {
        eprintln!(
            "itsanas: the trash could not be emptied: {}",
            error.message()
        );
    }
    if let Some((folder, _)) = folder {
        changed = reconcile(node, folder, deep);
        crate::freeing::release_all(node);
    } else if let Err(error) = node.bound_writes() {
        // `reconcile_once` refreshes the bound; with no folder nothing
        // did, and the pulls below would meet the ceiling `Node::open`
        // sets, which is none (8.1b).
        eprintln!("itsanas: could not work out this disk's room: {error}");
    }

    // Anything a peer pushed into this node's vault while it was serving.
    // Without this a node that never dials anybody — because it has no
    // peers configured, or because its peers are behind NAT and can only
    // push — would hold its own data and never look at it.
    match session::drain_vault(&node.store, &node.vault) {
        Ok(report) => {
            if report.changed_anything() {
                println!(
                    "pushed to us: {} files, {} conflicts",
                    report.adopted, report.conflicted
                );
                if let Some((folder, _)) = folder {
                    changed |= reconcile(node, folder, false);
                }
            }
            report_drain_refusals(&report);
        }
        Err(error) => eprintln!("itsanas: could not apply pushed data: {error}"),
    }
    changed
}

/// Block until the folder changes, the next sync is due, or shutdown.
pub(super) fn wait_for_work(
    folder: Option<&(Folder, Option<Watcher>)>,
    next_sync: Instant,
    shutdown: &AtomicBool,
) {
    let budget = next_sync.saturating_duration_since(Instant::now());

    if let Some((_, Some(watcher))) = folder {
        // Chopped into slices so a Ctrl-C during a long quiet period is still
        // noticed promptly.
        let slice = budget.min(Duration::from_secs(2));
        if watcher.wait_for_quiet(slice, watch::DEFAULT_DEBOUNCE, SETTLE_LIMIT) {
            return;
        }
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        return;
    }

    // No watcher: plain sleep, still chopped for responsiveness.
    let deadline = Instant::now() + budget.min(Duration::from_secs(2));
    while Instant::now() < deadline && !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(SHUTDOWN_POLL);
    }
}

/// Open the configured folder and start watching it, if one is configured.
pub(super) fn open_folder(node: &Node) -> Result<Option<(Folder, Option<Watcher>)>> {
    let Some(path) = &node.config.folder else {
        return Ok(None);
    };

    let folder = Folder::open(path)?;

    // A watcher is a latency optimisation. If the platform will not give us
    // one — too many watches, an unusual filesystem — the periodic rescan
    // still does the job, so this is a warning rather than a failure.
    let watcher = match Watcher::new(folder.root()) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            eprintln!(
                "itsanas: could not watch {}: {error}. Falling back to periodic \
                 scanning only, so local edits will take up to one interval to \
                 be noticed.",
                folder.root().display()
            );
            None
        }
    };

    Ok(Some((folder, watcher)))
}

/// One folder pass. Never propagates an error.
///
/// Two outcomes here are not failures of this daemon and must not read as
/// noise: a storage that is not mounted, and a pass that held deletions. Both
/// mean somebody has to do something, and both are said loudly, because the
/// alternative -- a line in a log nobody reads -- is how a disk stays unmounted
/// for a week.
pub(crate) fn reconcile_once(node: &Node, folder: &Folder, deep: bool) {
    reconcile(node, folder, deep);
}

/// Whether a pass found something changed *here*: a file taken in from the
/// folder or deleted from it. What it wrote out from the store came from
/// elsewhere and has nobody to go to.
pub(super) fn changed_here(report: &itsanas_folder::ReconcileReport) -> bool {
    !report.imported.is_empty() || !report.removed_from_store.is_empty()
}

/// [`reconcile_once`], saying whether the folder changed here.
pub(super) fn reconcile(node: &Node, folder: &Folder, deep: bool) -> bool {
    // Counting what other devices wrote since the last pass. On failure the
    // bound from the last pass stays, which is never none: opening the node
    // set one.
    if let Err(error) = node.bound_writes() {
        eprintln!("itsanas: could not count the account's files elsewhere: {error}");
    }
    match folder.reconcile(&node.store, deep) {
        Ok(report) => {
            if report.changed_anything() {
                println!("folder: {}", report.summary());
                // Named, so that a log says which files moved and a rename
                // reads as one, but bounded: a first pass over a full
                // library would otherwise flood it.
                for line in report.lines(Some(itsanas_folder::LINES_IN_A_LOG)) {
                    println!("{line}");
                }
            }
            if report.held_anything() {
                println!(
                    "folder: HELD {} deletion(s): that is most of this folder, and a \
                     folder somebody emptied looks exactly like a disk that is not \
                     mounted",
                    report.held_deletions
                );
                println!("  Nothing was removed from the account, here or anywhere else.");
                println!("  If they really are meant to go: `itsanas folder --confirm`.");
            }
            for (path, why) in &report.failed {
                eprintln!("itsanas: {path}: {why}");
            }
            changed_here(&report)
        }
        Err(error) => {
            eprintln!("itsanas: folder scan failed: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only what changed here sends a round early: files written out from
    /// the store came from a peer, and two daemons answering each other's
    /// writes with rounds would never be quiet.
    #[test]
    fn only_a_change_made_here_calls_for_an_early_round() {
        let mut report = itsanas_folder::ReconcileReport::default();
        report.exported.push("a".into());
        report.deleted_from_disk.push("b".into());
        assert!(!changed_here(&report), "a peer's file called for a round");
        let mut imported = itsanas_folder::ReconcileReport::default();
        imported.imported.push("c".into());
        assert!(changed_here(&imported));
        let mut deleted = itsanas_folder::ReconcileReport::default();
        deleted.removed_from_store.push("d".into());
        assert!(changed_here(&deleted));
    }
}
