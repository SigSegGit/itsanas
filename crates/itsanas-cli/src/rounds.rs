//! The machines to talk to, and one round against them.
//!
//! One list for every path that dials, whichever shell asks.

use std::collections::BTreeSet;

use itsanas_crypto::DeviceId;
use itsanas_node::Node;

/// The machines to talk to: any address typed in, the account's own from the
/// coordinator, pinned, then the address book's, a parked one through its
/// relay: what `itsanas sync` and `get` dial (`account_targets`).
///
/// One list for every path that dials. The phone read `peers` alone and said
/// "no machine is configured" while the owner's Windows node ran on the same
/// account (2026-10-31), first for a round and then for a fetch. Until
/// 2026-11-17 it also dropped every machine parked on a relay (§8 0v), so a
/// phone away from home reached none of them, and knew nobody when the
/// coordinator was down.
pub fn targets(node: &Node) -> Vec<(String, Option<DeviceId>)> {
    crate::account_targets(node, None)
}

/// Dial `target`, through its relay when it is parked (`connect_peer`).
///
/// # Errors
///
/// Whatever the dial or the handshake fails on.
pub fn connect(
    node: &Node,
    target: &str,
    pinned: Option<DeviceId>,
) -> itsanas_net::Result<itsanas_net::PeerClient> {
    crate::connect_peer(target, &node.device, node.store.owner(), pinned)
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

/// Run one round against each machine of `targets`; one that fails is
/// counted, not fatal.
///
/// One round per machine: the coordinator lists one at each of its addresses
/// (the LAN one and the public one, both up at home), and a phone ran a whole
/// round at each, pushing and pulling twice for one machine.
pub fn run_round(
    node: &Node,
    targets: &[(String, Option<DeviceId>)],
    scope: itsanas_net::session::Scope,
) -> RoundTotals {
    let mut totals = RoundTotals::default();
    let mut met = BTreeSet::new();
    for (target, pinned) in targets {
        if pinned.is_some_and(|device| met.contains(&device)) {
            continue;
        }
        let Ok(mut client) = connect(node, target, *pinned) else {
            totals.unreachable.push(target.clone());
            continue;
        };
        met.insert(client.peer_device());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine the coordinator lists at two addresses got a round at each.
    /// Sabotage: dial every address of a machine already met.
    #[test]
    fn red_team_a_machine_listed_at_two_addresses_gets_one_round() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (pi, phrase) =
                crate::testing::phone(&dir.path().join("pi"), coordinator, "camille", None);
            let (phone, _) = crate::testing::phone(
                &dir.path().join("phone"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let pi_device = pi.store.device_id();
            let totals = crate::testing::serving(&pi, |at| {
                let twice = [
                    (at.to_string(), Some(pi_device)),
                    (format!("localhost:{}", at.port()), Some(pi_device)),
                ];
                run_round(&phone, &twice, itsanas_net::session::Scope::Everything)
            });
            assert_eq!(
                totals.reached, 1,
                "a machine listed at two addresses got a round at each: a phone's sync pushes and pulls twice for one machine ({totals:?})"
            );
        });
    }

    /// A machine parked on a relay (§8 0v) is a phone's target, pinned and in
    /// the form the relayed dial takes; the phone's list dropped it, so away
    /// from home a phone reached no parked machine. Sabotage: drop `relay:`
    /// addresses from the phone's targets.
    #[test]
    fn red_team_a_phone_dials_a_parked_machine_through_its_relay() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (mut laptop, phrase) =
                crate::testing::phone(&dir.path().join("laptop"), coordinator, "camille", None);
            let (phone, _) = crate::testing::phone(
                &dir.path().join("phone"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let now = itsanas_discover::now_unix();
            itsanas_node::coordinator::register_with(&laptop, None, now).expect("register");
            let relay = DeviceId::from_bytes([7; 32]);
            let parked = format!(
                "{}{}@203.0.113.5:9797",
                itsanas_node::contact::RELAY_PREFIX,
                relay.to_hex()
            );
            laptop.config.announce = Some(parked.clone());
            itsanas_node::coordinator::announce(&laptop, &parked, now).expect("announce");

            let found = targets(&phone);
            assert!(
                found.contains(&(parked.clone(), Some(laptop.store.device_id()))),
                "a machine parked on a relay is not among a phone's targets, pinned: away from home a phone reaches none of them ({found:?})"
            );
        });
    }
}
