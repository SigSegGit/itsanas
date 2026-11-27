//! Application mode (HANDOVER §8 00): asking a peer whether it runs one,
//! the peers found that do, and the light round dialled to them.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use itsanas_crypto::DeviceId;
use itsanas_folder::{Folder, Watcher};
use itsanas_net::PeerClient;
use itsanas_policy::Scope as PolicyScope;

use super::{pledge::local, reconciling::reconcile_once, round::sync_once, shutdown::halted};
use crate::{control, node::Node};

/// Ask `client` whether it runs in application mode, if this machine does:
/// the others never dial anybody every minute, so the answer would be a round
/// trip for nothing. `None` when not asked or not answered.
pub(super) fn ask_app_mode(node: &Node, peer: &str, client: &mut PeerClient) -> Option<bool> {
    if !local(node).app.load(Ordering::Relaxed) {
        return None;
    }
    client.app_mode().unwrap_or_else(|error| {
        println!("{peer}: could not ask whether it runs in application mode ({error})");
        None
    })
}

/// Whether a light round would dial anybody now.
pub(super) fn app_peers_known(node: &Node) -> bool {
    local(node)
        .app_peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .any(Instant::now())
}

/// Note what `device`, reached at `peer`, said of its application mode.
///
/// Kept only for a machine that has something to do with this account: one of
/// its own, one that holds some of its data or took some this session, or one
/// already kept. A stranger that says "on" costs nothing to mint and would be
/// dialled every minute. `None` (not asked, too old, or no answer) changes
/// nothing: a peer that stopped answering ages out ([`crate::app_mode`]).
pub(super) fn note_app_peer(
    node: &Node,
    device: DeviceId,
    peer: &str,
    through: Option<&(DeviceId, String)>,
    answer: Option<bool>,
    earned_trust: bool,
) {
    let Some(on) = answer else {
        return;
    };
    let mut peers = local(node)
        .app_peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let ours = || {
        earned_trust
            || node.store.is_member(&device).unwrap_or(false)
            || node.store.bytes_held_by(&device).unwrap_or(0) > 0
    };
    if !on || peers.knows(&device) || ours() {
        peers.heard(device, peer, through, on, Instant::now());
    }
}

/// Dial the other machines in application mode this one knows, the one heard
/// from last first, and note each that answered in `reached` so the rest of
/// the round does not dial it twice.
pub(super) fn dial_app_peers(
    node: &Node,
    shutdown: &AtomicBool,
    reached: &mut BTreeSet<DeviceId>,
    scope: PolicyScope,
) {
    let due = local(node)
        .app_peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .due(Instant::now());
    for (device, peer) in due {
        if halted(node, shutdown) {
            break;
        }
        if reached.contains(&device) {
            continue;
        }
        let through = peer.through.as_ref();
        if let Some(outcome) = sync_once(
            node,
            &peer.address,
            through,
            Some(device),
            false,
            false,
            scope,
        ) {
            reached.insert(outcome.device);
        }
    }
}

/// A light round (application mode): what changed here is taken in and sent
/// to the other machines in application mode, and nobody else is dialled --
/// not the coordinator, not the hosts of other accounts, not the LAN. Those
/// wait for the full round, at its own interval.
pub(super) fn light_round(
    node: &Node,
    shutdown: &AtomicBool,
    folder: Option<&(Folder, Option<Watcher>)>,
    scope: PolicyScope,
) {
    let folder = folder.map(|(folder, _)| folder);
    if let Some(folder) = folder.filter(|_| !control::paused_on_disk(&node.home)) {
        reconcile_once(node, folder, false);
    }
    let mut reached = BTreeSet::new();
    dial_app_peers(node, shutdown, &mut reached, scope);
    // What they sent back is written out now, not a minute later.
    if let Some(folder) = folder.filter(|_| !control::paused_on_disk(&node.home)) {
        reconcile_once(node, folder, false);
    }
}
