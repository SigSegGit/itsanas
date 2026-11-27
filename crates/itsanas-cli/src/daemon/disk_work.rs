//! Work handed to the running daemon between rounds: a stop or restart asked
//! through `lifecycle`, and the file and disk jobs of `jobs` that need the
//! store this process holds.

use std::sync::atomic::{AtomicBool, Ordering};

use super::shutdown::{PAUSED_FOR_VAULT, RESTART_ASKED};
use crate::node::Node;

/// A stop or restart asked through the home (`lifecycle.rs`), acted on
/// between two rounds.
pub(super) fn lifecycle_request(node: &Node, shutdown: &AtomicBool) {
    match crate::lifecycle::take(&node.home) {
        Some(crate::lifecycle::Request::Stop) => {
            eprintln!("itsanas: asked to stop; stopping");
            shutdown.store(true, Ordering::SeqCst);
        }
        Some(crate::lifecycle::Request::Restart) => {
            eprintln!("itsanas: asked to restart on the program now installed; restarting");
            RESTART_ASKED.store(true, Ordering::SeqCst);
            shutdown.store(true, Ordering::SeqCst);
        }
        None => {}
    }
}

/// What is asked through the home between two rounds: a disk job, then a
/// stop or a restart.
pub(super) fn between_rounds(node: &Node, shutdown: &AtomicBool) {
    run_disk_job(node, shutdown);
    lifecycle_request(node, shutdown);
}

/// A vault move asked of this run (`jobs.rs`): done by `start_daemon` once
/// the run has ended and the node is closed, then the daemon starts again.
pub(super) static PENDING_MOVE: std::sync::Mutex<Option<crate::jobs::Job>> =
    std::sync::Mutex::new(None);

/// The move this run was stopped for, if any.
pub(crate) fn take_pending_move() -> Option<crate::jobs::Job> {
    PENDING_MOVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

/// Do the disk job waiting in the home, if any (`jobs.rs`). An export or a
/// restore runs here, on the open node; a move stops this run, as a disk
/// that went away does, and `start_daemon` does it with the node closed.
pub(super) fn run_disk_job(node: &Node, shutdown: &AtomicBool) {
    use crate::jobs::{self, Kind, State};
    let Some(job) = jobs::take(&node.home) else {
        return;
    };
    let job = match job {
        Ok(job) => job,
        Err(why) => {
            eprintln!("itsanas: {why}");
            return;
        }
    };
    let target = job.path.display().to_string();
    let outcome = match job.kind {
        Kind::MoveVault { .. } if node.away => {
            Err("the vault's disk is away; plug it back in before moving the vault".to_owned())
        }
        Kind::MoveVault { .. } => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                &format!("moving the vault to {target}"),
            );
            eprintln!("itsanas: moving the vault to {target}; this run stops for the copy");
            *PENDING_MOVE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job);
            PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
            shutdown.store(true, Ordering::SeqCst);
            return;
        }
        Kind::Export => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                &format!("writing a backup to {target}"),
            );
            crate::backup::export(node, &job.path)
                .map(|moved| {
                    crate::backup::export_summary(&moved, &target)
                        + &crate::drives::warning_for(&job.path, "the backup")
                            .map(|warning| format!(". Note: {warning}"))
                            .unwrap_or_default()
                })
                .map_err(|error| error.to_string())
        }
        Kind::List
        | Kind::Put
        | Kind::Get
        | Kind::Remove { .. }
        | Kind::Trash
        | Kind::Free { .. }
        | Kind::Move
        | Kind::Versions => crate::file_job(node, &job),
        Kind::ConvertPacks => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                "moving this machine's chunks into pack files",
            );
            eprintln!("itsanas: converting to pack files; this run stops for it");
            *PENDING_MOVE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job);
            PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
            shutdown.store(true, Ordering::SeqCst);
            return;
        }
        Kind::Restore => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                &format!("reading the backup at {target}"),
            );
            crate::backup::restore(node, &job.path)
                .map(|(moved, report)| crate::backup::restore_summary(&moved, &report, &target))
                .map_err(|error| error.to_string())
        }
    };
    match outcome {
        Ok(said) => {
            eprintln!("itsanas: {said}");
            jobs::report(&node.home, job.id, State::Done, &said);
        }
        Err(why) => {
            eprintln!("itsanas: the disk job on {target} failed: {why}");
            jobs::report(&node.home, job.id, State::Failed, &why);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use super::super::shutdown::resume_after_vault_pause;

    /// `itsanas ls`, `put`, `get` and `rm` refused to start while the daemon
    /// ran, which is always on a machine set up as a service (HANDOVER §9,
    /// "One process per node"). They hand the work to it now, and it does it
    /// on the node it holds. Sabotage: the daemon refuses file jobs.
    #[test]
    fn red_team_the_file_commands_work_while_the_daemon_holds_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let (node, _) = Node::create(&home, "a passphrase", "camille").expect("create");
        assert!(
            crate::daemon_holds(&home),
            "the fixture's node does not hold the store"
        );
        let source = dir.path().join("note.txt");
        std::fs::write(&source, b"stored through the daemon").expect("source");
        let back = dir.path().join("back.txt");
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::SeqCst) {
                    run_disk_job(&node, &AtomicBool::new(false));
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let outcome = (|| -> crate::Result<()> {
                crate::put(&home, "notes/note.txt", &source)?;
                crate::list(&home)?;
                crate::get(&home, "notes/note.txt", Some(&back))?;
                crate::remove(&home, "notes/note.txt", false)?;
                crate::trash_command(&home, Some(crate::deleting::TrashAction::List))?;
                crate::trash_command(&home, Some(crate::deleting::TrashAction::Rules))?;
                crate::trash_command(&home, Some(crate::deleting::TrashAction::Days { days: 12 }))?;
                crate::trash_command(
                    &home,
                    Some(crate::deleting::TrashAction::Restore {
                        path: "notes/note.txt".to_owned(),
                    }),
                )?;
                crate::remove(&home, "notes/note.txt", true)
            })();
            stop.store(true, Ordering::SeqCst);
            outcome.expect("a file command refused while the daemon held the store");
        });
        assert_eq!(
            std::fs::read(&back).expect("the file got back"),
            b"stored through the daemon",
            "`get` through the daemon did not give back what `put` stored"
        );
        assert_eq!(
            node.store.read_file("notes/note.txt").expect("read"),
            None,
            "`rm --now` through the daemon left the file in the account"
        );
        assert_eq!(
            itsanas_store::deletion::rules_here(&node.store, &node.vault)
                .expect("rules")
                .days,
            12,
            "`trash days` through the daemon did not change this machine's rules"
        );
    }

    /// A command that opens the store (`keep`, `peer`, `device`...) refused
    /// while the daemon ran. It now asks the daemon to stop, waits for the
    /// store to be let go, and opens it. Sabotage: open without stopping it.
    #[test]
    fn red_team_a_command_that_opens_the_store_stops_the_daemon_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let (node, _) = Node::create(&home, "a passphrase", "camille").expect("create");
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // The daemon's side: a stop asked through the home ends the run.
                let deadline = std::time::Instant::now() + Duration::from_secs(60);
                while std::time::Instant::now() < deadline {
                    if crate::lifecycle::take(&home) == Some(crate::lifecycle::Request::Stop) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(node);
            });
            crate::pause_daemon(&home).expect("the daemon is asked to stop");
            assert!(
                !crate::daemon_holds(&home),
                "the command went on while the daemon still held the store: it would refuse, as before"
            );
            crate::resume_paused_daemon(&home);
        });
    }

    /// `itsanas convert-packs` refused while the service ran, and a service
    /// always runs. Handed to it, the run stops with the job pending, as for
    /// a vault move, and `start_daemon` converts once the store is closed.
    /// Sabotage: the daemon converts nothing and reports it done.
    #[test]
    fn red_team_convert_packs_while_the_service_runs_stops_the_run_and_waits_for_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let (node, _) = Node::create(&home, "a passphrase", "camille").expect("create");
        let id = crate::jobs::ask(&home, crate::jobs::Kind::ConvertPacks, &home).expect("asked");
        let shutdown = AtomicBool::new(false);
        run_disk_job(&node, &shutdown);
        let pending = take_pending_move();
        let resumed = resume_after_vault_pause();
        assert!(
            shutdown.load(Ordering::SeqCst) && resumed,
            "the run did not stop for the conversion: packs cannot be written beside an open store"
        );
        assert_eq!(
            pending.map(|job| (job.id, job.kind)),
            Some((id, crate::jobs::Kind::ConvertPacks)),
            "the conversion was not left for start_daemon to do once the store is closed"
        );
        drop(node);
        crate::convert_stores(&home).expect("convert with the store closed");
        assert!(
            itsanas_store::BlobStore::open(Node::store_path(&home))
                .expect("store")
                .packs()
                .is_some(),
            "the store is not in pack files after the conversion"
        );
    }

    /// An old store in one file per chunk goes into packs on its own at the
    /// daemon's start, never on a disk without room for the copy the
    /// conversion makes first, and never with packs turned off. Sabotage:
    /// drop the free-space check from `conversion_due`.
    #[test]
    fn red_team_an_old_store_converts_itself_but_never_fills_its_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("store");
        let blobs = itsanas_store::BlobStore::open_files(&root).expect("files");
        blobs
            .put(
                &itsanas_crypto::ChunkId::from_bytes([1; 32]),
                &vec![7u8; 300_000],
            )
            .expect("put");
        drop(blobs);
        let roots = vec![root.clone()];
        assert!(
            !crate::conversion_due(&roots, true, &|_| Some(u64::MAX)),
            "converted although packs are turned off"
        );
        assert!(
            !crate::conversion_due(&roots, false, &|_| Some(1 << 20)),
            "a conversion was started on a disk without room for its copy: it fills the disk"
        );
        assert!(
            crate::conversion_due(&roots, false, &|_| Some(u64::MAX)),
            "an old store with room to spare is never converted"
        );
        itsanas_store::BlobStore::convert_to_packs(&root).expect("convert");
        assert!(
            !crate::conversion_due(&roots, false, &|_| Some(u64::MAX)),
            "a store already in packs is converted again"
        );
    }
}
