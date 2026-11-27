//! What this machine keeps for the others: its per-device state, the room
//! and the pledge it can honour, hosting a peer, its standing with the
//! coordinator, and the checks of its vault and disks.

use std::{
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_crypto::DeviceId;
use itsanas_net::{PeerClient, PeerService, Pledge, session};
use itsanas_node::contact::SharedBoard;

use crate::{config::format_size, coordinator, node::Node};

/// Say what a device short of room did with this round.
///
/// Worth a line because none of it is a failure and all of it reads like one
/// otherwise: files arriving, files being let go of, and the case that has to be
/// said out loud -- content this device would have released and could not,
/// because no live holder is known for it. That last one means the device is
/// over its limit *and* is the only place that data exists, which is the state
/// a person most needs to be told about and the one a silent implementation
/// would hide.
pub(super) fn report_keeping(peer: &str, keeping: &crate::keeping::KeepingReport) {
    if !keeping.worth_reporting() {
        return;
    }

    let mut line = format!("{peer}: keeping");
    if keeping.fetched > 0 {
        let _ = write!(line, " {} fetched", keeping.fetched);
    }
    if keeping.released > 0 {
        let _ = write!(
            line,
            " {} let go ({} freed)",
            keeping.released,
            format_size(keeping.freed)
        );
    }
    println!("{line}");

    if keeping.not_safe_yet > 0 {
        println!(
            concat!(
                "  {} file(s) could not be let go of: letting go needs {} other ",
                "live machines,"
            ),
            keeping.not_safe_yet,
            itsanas_store::holders::SAFE_TO_RELEASE
        );
        println!("  one of which has confirmed that very chunk recently.");
        println!("  This device stays over its limit until they do.");
    }
}

/// The third half of a round: hold some of what this peer needs held.
///
/// Without it the *dialled* side does all the hosting, because `push` gives
/// this node's work away and `pull` only fetches. That shut out every member
/// behind a router they do not control -- and in a system built on members
/// holding each other's data, being unable to host is being unable to keep your
/// half of the bargain, not a missing convenience.
///
/// Runs after push and pull: a peer is given what it is owed before being asked
/// a favour, and a failure here must not cost this node the sync it came for.
/// Nothing here is fatal, including a peer too old to understand the question,
/// which answers `Refused`.
/// What the listener answers with: this node's store and vault, its pledge,
/// the relay board, and the per-account bound with this node's own claim.
/// Whether this run turns every other account away: its vault's disk is
/// away, or it is a portable copy, which travels and is nobody's host.
pub(super) fn hosts_nobody(node: &Node) -> bool {
    node.away || crate::portable::is_portable(&node.home)
}

pub(super) fn peer_service<'a>(
    node: &'a Node,
    board: &'a SharedBoard,
    disk_room: &'a (dyn Fn() -> Option<u64> + Sync),
) -> PeerService<'a> {
    PeerService::new(
        &node.store,
        &node.vault,
        Pledge {
            bytes: node.config.pledge_bytes,
        },
    )
    .with_relay(board)
    .with_disk_room(disk_room)
    // Whose each device is and what it offers, as its claim said (§8 1c).
    .with_owners(owners(node))
    .hosting_away(hosts_nobody(node))
    // Empty in `everyone`, so `WantHosted` answers as it always did; in
    // `measured` each round swaps its placement in (`placing::begin_round`).
    .with_placement(crate::placing::slot())
    .claiming(node.claim_bytes(itsanas_discover::now_unix()))
    .noting_siblings(&SIBLING_SEEN)
    .in_app_mode(&local(node).app)
}

/// When a machine of this account last presented its claim to this node's
/// listener: a contact, for `isolated`, on a machine the others dial in to.
pub(super) static SIBLING_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// When the coordinator last named hosts, and which.
pub(super) type Introduced = (Instant, Vec<(DeviceId, String)>);

/// What outlives a run of the loop, for one machine.
pub(super) struct Local {
    owners: itsanas_node::owners::ClaimBook,
    /// Read per offer by the listener and the hosting pull ([`Hosting`]).
    pub(super) answers_only: AtomicBool,
    /// The hosts the coordinator last named, and when ([`introduced`](super::dialling::introduced)).
    pub(super) introduced: std::sync::Mutex<Option<Introduced>>,
    /// Whether this machine runs in application mode and syncs in full now:
    /// what the listener answers to [`itsanas_net::protocol::Request::AppMode`],
    /// and whether a round asks its peers the same (HANDOVER §8 00).
    pub(super) app: AtomicBool,
    /// The other machines in application mode it found ([`crate::app_mode`]).
    pub(super) app_peers: std::sync::Mutex<crate::app_mode::AppPeers>,
}

/// [`Local`] for `node`. One per device and not one per process: a phone's
/// process outlives a sign-out, and the next account's listener would have
/// asked the first one's coordinator pin about the devices presenting to it.
/// Kept for the life of the process, which sees a handful at most.
pub(super) fn local(node: &Node) -> &'static Local {
    static LOCALS: std::sync::Mutex<std::collections::BTreeMap<DeviceId, &'static Local>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let mut locals = LOCALS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locals.entry(node.store.device_id()).or_insert_with(|| {
        Box::leak(Box::new(Local {
            owners: itsanas_node::owners::ClaimBook::new()
                .asking(node.store.owner(), coordinator::asker(node)),
            answers_only: AtomicBool::new(false),
            introduced: std::sync::Mutex::new(None),
            app: AtomicBool::new(false),
            app_peers: std::sync::Mutex::new(crate::app_mode::AppPeers::default()),
        }))
    })
}

/// The claims this daemon's peers presented (§8 1c), shared by the listener,
/// which takes them, and the hosting pull, which is held to them as offers
/// are. Asks the coordinator about a device of another account when it
/// presents (§8 1c (ii)).
pub(super) fn owners(node: &Node) -> &'static itsanas_node::owners::ClaimBook {
    &local(node).owners
}

/// Ask the coordinator about the devices whose confirmation is ageing or
/// missing (§8 1c (ii)). Said only when it changes something: a withdrawal,
/// or a coordinator that would not answer while devices waited.
pub(super) fn check_standing(node: &Node) {
    match coordinator::standing(node, owners(node)) {
        Ok(report) => {
            if report.unpinned {
                println!(
                    "coordinator: not pinned, so its word about other accounts' devices is not \
                     taken and they store nothing here; pin it with \
                     `itsanas coordinator <host:port> --device <id>`"
                );
            }
            if report.withdrawn > 0 {
                println!(
                    "coordinator: {} device(s) presenting here were withdrawn by their account; \
                     they store nothing here",
                    report.withdrawn
                );
            }
            if report.unanswered > 0 {
                println!(
                    "coordinator: did not answer for {} device(s) of other accounts; until it \
                     does they store nothing here",
                    report.unanswered
                );
            }
        }
        Err(error) => println!(
            "coordinator: could not confirm the devices of other accounts ({error}); \
             until it can they store nothing here"
        ),
    }
}

/// What the disk can still take beyond the reserve, read per offer.
/// No room at all while the vault's marker is missing: a NAS that dropped
/// while the daemon runs leaves an empty local mount point, and chunks written
/// there land on the system disk and vanish under the mount when it returns
/// (Rodin, 2026-10-13). None either while the policy says answers only: an
/// audit reads what is held and is not an offer, so it is still answered, with
/// no refusal that could pass for one.
pub(super) fn disk_room(node: &Node) -> impl Fn() -> Option<u64> + Sync + use<> {
    room_at(node.vault_dir(), &local(node).answers_only)
}

/// [`disk_room`], given the vault and the flag.
pub(super) fn room_at(
    vault: std::path::PathBuf,
    answers_only: &'static AtomicBool,
) -> impl Fn() -> Option<u64> + Sync {
    move || {
        if answers_only.load(Ordering::Relaxed) || !vault_present(&vault) {
            return Some(0);
        }
        // The reserve file is space held for exactly these chunks: free to
        // them, or `reserve = on` would refuse what it was made to keep.
        fs4::available_space(&vault).ok().map(|free| {
            free.saturating_add(itsanas_node::pledges::reserved(&vault))
                .saturating_sub(itsanas_node::pledges::RESERVE)
        })
    }
}

/// Whether the vault directory still carries its marker.
pub(crate) fn vault_present(vault: &std::path::Path) -> bool {
    vault.join(itsanas_node::node::VAULT_MARKER).is_file()
}

/// The pledge as far as the disk can still honour it (`pledges::keepable`):
/// another program filling the disk must stop the hosting, not a write.
pub(super) fn keepable_pledge(node: &Node) -> u64 {
    if local(node).answers_only.load(Ordering::Relaxed) || !vault_present(&node.vault_dir()) {
        return 0;
    }
    itsanas_node::pledges::keepable(
        node.config.pledge_bytes,
        node.vault.held_bytes().unwrap_or(0),
        crate::free_for_pledge(node),
    )
}

/// `reserve = on`: size the reserve file to what is promised and not yet
/// held, before this round takes anything in; `off`, or a vault away, takes
/// it away. A failure is said and changes nothing else.
pub(super) fn hold_the_pledge(node: &Node) {
    let dir = node.vault_dir();
    if !node.config.reserve || !vault_present(&dir) {
        itsanas_node::pledges::unreserve(&dir);
        return;
    }
    let Ok(free) = fs4::available_space(&dir) else {
        return;
    };
    if let Err(error) = itsanas_node::pledges::reserve(
        &dir,
        node.config.pledge_bytes,
        node.vault.held_bytes().unwrap_or(0),
        free,
    ) {
        eprintln!("itsanas: the pledge could not be held on disk ({error})");
    }
}

pub(super) fn take_on_hosting(node: &Node, peer: &str, client: &mut PeerClient) {
    let pledge = Pledge::bytes(keepable_pledge(node));
    let bound: (&dyn itsanas_net::Owners, &itsanas_store::Store) = (owners(node), &node.store);
    match session::host_for_bounded(&node.vault, client, pledge, Some(bound)) {
        // A peer that wanted nothing has nothing to say, and saying it every
        // five minutes would fill a journal with silence. A peer that wanted
        // something always leaves a line, even when the answer was "already
        // held" -- because that is the case that was silent, and a feature
        // whose success looks exactly like never having run is not verifiable.
        //
        // Found by watching for a line that never came: on this project's own
        // fleet the reciprocal half ran for hours against a peer with three
        // under-replicated chunks and printed nothing at all, because they were
        // chunks this node already had.
        Ok(report) if report.wanted == 0 => {}
        Ok(report) => {
            let mut line = format!("{peer}: asked to hold {} chunks", report.wanted);
            if report.taken > 0 {
                let _ = write!(
                    line,
                    "; took {} ({})",
                    report.taken,
                    format_size(report.bytes_taken)
                );
            }
            if report.already_held > 0 {
                let _ = write!(line, "; {} already held", report.already_held);
            }
            if let Some(why) = &report.bounded {
                let _ = write!(line, "; took no more: {why}");
            }
            if report.pledge_full {
                line.push_str("; this node is full");
            }
            println!("{line}");
        }
        // A machine of another account never lets this one read what it
        // holds for itself (`NOT_YOUR_ACCOUNT`, protocol 11): between accounts
        // the route is the owner's push. Said every round, the expected answer
        // buried the line that mattered (`tests/route.rs`, 2026-10-30).
        Err(itsanas_net::NetError::Refused(why)) if why == itsanas_net::NOT_YOUR_ACCOUNT => {}
        Err(error) => println!("{peer}: could not take on hosting ({error})"),
    }
}

/// Tell the coordinator where this machine is, and learn where the account's
/// When the account check last ran, unix seconds (once every six hours).
pub(super) static DELETED_CHECKED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Ask the coordinator which accounts this host keeps something for were
/// deleted, and free what it keeps for them (HANDOVER §8 0w (8)). Asked at
/// the rounds rather than told once, so a host offline on the day still frees
/// it. If this machine's own account was deleted, it records its departure
/// and stops, as `leave` does: its keys can no longer register anywhere.
pub(super) fn forget_deleted(node: &Node, shutdown: &AtomicBool) {
    let now = itsanas_discover::now_unix();
    if now.saturating_sub(DELETED_CHECKED.load(Ordering::Relaxed)) < 6 * 3600 {
        return;
    }
    DELETED_CHECKED.store(now, Ordering::Relaxed);
    let mut owners = node.vault.owners().unwrap_or_default();
    let me = node.store.owner();
    if !owners.contains(&me) {
        owners.push(me);
    }
    let Ok(deleted) = coordinator::deleted_among(node, &owners) else {
        return;
    };
    for owner in deleted {
        if owner == me {
            println!(
                "this account was deleted at its coordinator; this machine leaves and stops. \
                 Remove its data with install/clean (--purge-account)."
            );
            if let Err(error) = crate::record_departure(&node.home, now) {
                eprintln!("itsanas: could not record the departure: {error}");
            }
            shutdown.store(true, Ordering::SeqCst);
            continue;
        }
        match node.vault.forget_owner(owner) {
            Ok((chunks, segments)) => println!(
                "an account this machine kept data for was deleted: freed {chunks} chunk(s) \
                 and {segments} log segment(s)"
            ),
            Err(error) => eprintln!("itsanas: could not free a deleted account's data: {error}"),
        }
    }
}

/// Write to the vault once a round, and keep in `unwritable` why it failed.
///
/// Free space is not the only way to break the pledge: a disk remounted
/// read-only after errors, a permission changed, a quota reached, all keep
/// their room and refuse every chunk pushed to them. Only a write finds it,
/// so it is made here, once a round, and never from `status --brief`, which
/// the trays poll. Said in the log when it starts and when it ends, not every
/// round. A node away from its vault, or pledging nothing, promised nobody.
pub(super) fn probe_vault(node: &Node, unwritable: &mut Option<String>) {
    match crate::vault_refuses(node) {
        Some(error) => {
            if unwritable.is_none() {
                eprintln!(
                    "itsanas: the vault at {} cannot be written ({error}); it keeps no \
                     promise to the others until it can",
                    node.vault_dir().display()
                );
            }
            *unwritable = Some(error);
        }
        None => {
            if unwritable.take().is_some() {
                println!(
                    "the vault at {} takes writes again",
                    node.vault_dir().display()
                );
            }
        }
    }
}

/// Check a slice of the vault's index and of this node's own chunks against
/// the disk, sized so each completes a pass within `REFRESH_AFTER`.
///
/// Since §8 4c a peer whose summary agrees with this node's is re-stamped
/// without being asked, so a summary has to stop counting a blob the disk has
/// lost, or this node keeps counting as a copy it cannot serve. Never fails
/// the round: a check that cannot run is said, and the next round tries again.
pub(super) fn check_disks(
    node: &Node,
    interval: Duration,
    own_cursor: &mut Option<itsanas_crypto::ChunkId>,
) {
    let every = interval.as_secs();
    // In batches: one call holds at most `MAX_CHECK_BATCH` keys, however large
    // the slice this round owes.
    let vault_pass = node.vault.chunk_rows().and_then(|rows| {
        let mut owed = itsanas_store::holders::rows_per_round(rows, every);
        let mut total = itsanas_store::DiskCheck::default();
        while owed > 0 {
            let check = node.vault.check_disk(owed)?;
            total.checked += check.checked;
            total.removed += check.removed;
            if check.checked == 0 {
                break;
            }
            owed = owed.saturating_sub(check.checked);
        }
        Ok(total)
    });
    match vault_pass {
        Ok(check) if check.removed > 0 => println!(
            "vault: {} chunk(s) held for others were no longer on this disk; their owners \
             will send them again",
            check.removed
        ),
        Ok(_) => {}
        Err(error) => eprintln!("itsanas: could not check the vault against the disk: {error}"),
    }
    let own_pass = node.store.live_chunk_count().and_then(|rows| {
        let mut owed = itsanas_store::holders::rows_per_round(rows, every);
        let mut missing = 0usize;
        while owed > 0 {
            let batch = owed.min(itsanas_store::MAX_CHECK_BATCH);
            let (lost, next) = node.store.check_disk(own_cursor.as_ref(), batch)?;
            missing += lost;
            *own_cursor = next;
            owed -= batch;
            if own_cursor.is_none() {
                break;
            }
        }
        Ok((missing, ()))
    });
    match own_pass {
        Ok((missing, ())) => {
            if missing > 0 {
                println!(
                    "{missing} chunk(s) of this account are no longer on this disk; \
                     repair will fetch them from their holders"
                );
            }
        }
        Err(error) => {
            eprintln!("itsanas: could not check this node's chunks against the disk: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_a_vault_that_drops_while_running_takes_no_chunk() {
        static ANSWERS_ONLY: AtomicBool = AtomicBool::new(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("nas").join("vault");
        std::fs::create_dir_all(&vault).expect("vault");
        std::fs::write(vault.join(itsanas_node::node::VAULT_MARKER), "device").expect("marker");
        let room = room_at(vault.clone(), &ANSWERS_ONLY);
        assert_ne!(room(), Some(0), "a mounted vault with room offered none");
        // The NAS unmounts: an empty local mount point is left.
        std::fs::remove_file(vault.join(itsanas_node::node::VAULT_MARKER)).expect("unmount");
        assert_eq!(
            room(),
            Some(0),
            "chunks were still accepted into a vault whose disk dropped: they would land on \
             the system disk and vanish under the mount when it returns"
        );
    }
}
