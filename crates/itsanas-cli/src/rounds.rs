//! The machines to talk to, and one round against them.
//!
//! One list for every path that dials, whichever shell asks.

use itsanas_node::Node;

/// The machines to talk to: the account's own from the coordinator, pinned,
/// beside any address typed in.
///
/// One list for every path that dials. The phone read `peers` alone and said
/// "no machine is configured" while the owner's Windows node ran on the same
/// account (2026-10-31), first for a round and then for a fetch. A parked
/// machine (`relay:`) is left to the desktop's dialler for now.
pub fn targets(node: &Node) -> Vec<(String, Option<itsanas_crypto::DeviceId>)> {
    let mut targets: Vec<(String, Option<itsanas_crypto::DeviceId>)> = node
        .config
        .peers
        .iter()
        .map(|peer| (peer.clone(), None))
        .collect();
    if node.config.coordinator.is_some()
        && let Ok(found) = itsanas_node::coordinator::peers(node, node.store.owner())
    {
        for (device, address) in found {
            if !address.starts_with(itsanas_node::contact::RELAY_PREFIX)
                && !targets.iter().any(|(known, _)| *known == address)
            {
                targets.push((address, Some(device)));
            }
        }
    }
    targets
}

/// What a phone says when there is nobody to talk to.
pub const NO_MACHINE_YET: &str = "no other machine of this account is known yet: is ITSaNAS \
                              running on another of your machines?";

/// What one round against a list of machines did.
#[derive(Debug, Default)]
pub struct RoundTotals {
    pub reached: usize,
    pub adopted: usize,
    pub sent: u64,
    pub released: usize,
    pub freed: u64,
    pub unreachable: Vec<String>,
}

/// Run one round against each of `targets`; one that fails is counted, not
/// fatal.
pub fn run_round(
    node: &Node,
    targets: &[(String, Option<itsanas_crypto::DeviceId>)],
    scope: itsanas_net::session::Scope,
) -> RoundTotals {
    let mut totals = RoundTotals::default();
    for (target, pinned) in targets {
        let Ok(mut client) = itsanas_net::PeerClient::connect(
            itsanas_net::AddressList(target.as_str()),
            &node.device,
            node.store.owner(),
            *pinned,
        ) else {
            totals.unreachable.push(target.clone());
            continue;
        };
        let _ = node.introduce(&mut client, itsanas_discover::now_unix());

        match itsanas_node::round(
            &node.store,
            &node.vault,
            &node.config.keeping(),
            &mut client,
            scope,
        ) {
            Ok((report, keeping)) => {
                totals.reached += 1;
                totals.adopted += report.pull.adopted;
                totals.sent = totals.sent.saturating_add(report.push.bytes_sent);
                totals.released += keeping.released;
                totals.freed = totals.freed.saturating_add(keeping.freed);
            }
            Err(_) => totals.unreachable.push(target.clone()),
        }
    }
    totals
}
