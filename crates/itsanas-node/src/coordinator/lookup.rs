//! Reading where an account's devices are, each presence checked against
//! the device that signed it and the claim that makes it the account's;
//! what they pledge; other members and hosts found by the coordinator.

use itsanas_coord::claim::{ClaimedPresence, MAX_ADDRESS_LEN, SignedPresence};
use itsanas_coord::protocol::{EnrolledDevice, Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, DeviceKeys, UserId};

use super::{dial, dial_as, is_private_address};
use crate::node::Node;
use crate::{NodeError as CliError, Result};

/// Keep the chain length each sibling signed in its presence (`head`), every
/// presence here already checked against its device. Failing to keep one
/// costs a warning that a listing is short, never the read.
pub fn note_heads(node: &Node, presences: &[SignedPresence]) {
    for signed in presences {
        let presence = &signed.presence;
        if presence.device != node.store.device_id() {
            let _ = node.store.note_known_head(&presence.device, presence.head);
        }
    }
}

/// Where the other devices of `user` say they are.
pub fn peers(node: &Node, user: UserId) -> Result<Vec<(DeviceId, String)>> {
    let mut client = dial(node)?;
    let read = located(&mut client, &node.config, &node.device, user, true)?;
    if user == node.store.owner() {
        note_heads(node, &read.presences);
    }
    let mut found: Vec<(DeviceId, String)> = read
        .found
        .into_iter()
        .filter(|(device, _)| *device != node.store.device_id())
        .collect();
    reachable_first(&mut found);
    Ok(found)
}

/// Put the addresses that can work from anywhere before the ones that cannot.
///
/// A private address means something only on the network its announcer was on.
/// Read at home it is every one of your machines; read from a friend's house it
/// is nothing, or worse, somebody else's machine at the same number -- refused,
/// because the device id is pinned, but only after a connection was spent on
/// it. A round dials in order and has a budget, so the order decides what gets
/// tried at all on a laptop that has three dead entries and one live one.
///
/// This is the receiver's own judgement about an address, not a claim carried
/// in the presence. A peer does not get to sort itself first, for the same
/// reason its clock does not get to order its announcements.
///
/// Stable, so the coordinator's own order -- most recently seen first -- still
/// decides between two addresses of the same kind.
pub fn reachable_first(peers: &mut [(DeviceId, String)]) {
    peers.sort_by_key(|(_, address)| u8::from(is_private_address(address)));
}

/// The same order, for addresses with no device id beside them -- the peers
/// somebody typed into the configuration, which a round dials before anything
/// else and which are usually the LAN addresses of a house.
pub fn reachable_first_addresses(addresses: &mut [String]) {
    addresses.sort_by_key(|address| u8::from(is_private_address(address)));
}

/// Every device the coordinator lists for an account, this one included.
///
/// `peers` drops this machine, because dialling yourself is not useful. Listing
/// them for a person is the other case: leaving this device out of a list of
/// your devices makes the list wrong, and makes the one you are looking for --
/// the one you are trying to tell from the others -- invisible.
///
/// # Errors
///
/// If the coordinator cannot be reached, refuses, or answers with something
/// else.
pub fn devices(node: &Node, user: UserId) -> Result<Vec<(DeviceId, String)>> {
    devices_as(&node.config, &node.device, user)
}

/// The same, without an open store. See [`dial_as`].
///
/// # Errors
///
/// As [`devices`].
pub fn devices_as(
    config: &crate::config::Config,
    device: &DeviceKeys,
    user: UserId,
) -> Result<Vec<(DeviceId, String)>> {
    let mut client = dial_as(config, device)?;
    // A one-off command has no history with this coordinator to hold it to.
    located(&mut client, config, device, user, true).map(|read| read.found)
}

/// What one read of an account's devices brought back.
#[derive(Debug, Default)]
pub struct Located {
    /// Each device and the address it published, in the coordinator's order.
    pub found: Vec<(DeviceId, String)>,
    /// How many presences were dropped because their device did not sign them
    /// ([`verified`]), or because their claim did not make them a live device
    /// of the account asked about ([`verified_claimed`]). An honest
    /// coordinator checked every one on arrival, so anything but zero means
    /// it is lying.
    pub forged: usize,
    /// Whether the list was signed. `false` means the coordinator hung up on
    /// `SignedPeers` and `Peers` answered: every address is on its word.
    pub signed: bool,
    /// The presences behind `found`, each checked against its device. Empty
    /// when `signed` is false. Kept so the address book can write them and
    /// check them again when it reads them back.
    pub presences: Vec<SignedPresence>,
    /// The same presences with the owner's claim on each, checked. Empty
    /// when the coordinator is older than `ClaimedPeers`: then this read has
    /// nothing a peer could be handed on, which is the safe way to be short.
    pub claimed: Vec<ClaimedPresence>,
}

/// Where the devices of `user` are, each address checked against the device
/// that published it.
///
/// Asks `ClaimedPeers` and keeps what [`verified_claimed`] keeps; a
/// coordinator older than that hangs up, and a fresh connection asks
/// `SignedPeers` and keeps what [`verified`] keeps, with nothing relayable.
/// A coordinator older than that request hangs up on it too, which looks
/// like an outage. With
/// `accept_unsigned`, `client` is then replaced by a fresh connection that asks
/// `Peers`, which every coordinator answers, and the result says it is
/// unsigned. **That fallback is a door a hostile coordinator can open at will**
/// -- hanging up is all it takes -- which is why a caller that has seen this
/// coordinator sign passes `false` ([`crate::contact::Due::accept_unsigned`]),
/// and the hang-up is then a failed read. Without the flag the signature check
/// would be advice. If the fresh connection fails too, the network is the
/// problem and is reported as one.
pub(super) fn located(
    client: &mut CoordClient,
    config: &crate::config::Config,
    device: &DeviceKeys,
    user: UserId,
    accept_unsigned: bool,
) -> Result<Located> {
    match client.ask(&Request::ClaimedPeers { user }) {
        Ok(Response::ClaimedPeers(list)) => {
            let (claimed, forged) = verified_claimed(list, user);
            let found = claimed
                .iter()
                .map(|row| {
                    (
                        row.presence.presence.device,
                        row.presence.presence.address.clone(),
                    )
                })
                .collect();
            let presences = claimed.iter().map(|row| row.presence.clone()).collect();
            Ok(Located {
                found,
                forged,
                signed: true,
                presences,
                claimed,
            })
        }
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        // Older than `ClaimedPeers`, or pretending to be: it hung up. What is
        // lost is the claim, so nothing read here is relayable -- the signed
        // list below still checks every address against its device, and the
        // unsigned one still needs `accept_unsigned`. No memory is kept of
        // this coordinator having claimed before, because being refused
        // relayable presences costs gossip and never sends a machine anywhere.
        // The price, accepted: the signed list checks where, not whose, so a
        // coordinator hanging up here on purpose can still pad the book with
        // other accounts' machines -- a connect timeout each, never relayed.
        Err(_) => {
            *client = dial_as(config, device)?;
            signed_located(client, config, device, user, accept_unsigned)
        }
    }
}

/// [`located`] without the claims: `SignedPeers`, falling back to `Peers`.
fn signed_located(
    client: &mut CoordClient,
    config: &crate::config::Config,
    device: &DeviceKeys,
    user: UserId,
    accept_unsigned: bool,
) -> Result<Located> {
    match client.ask(&Request::SignedPeers { user }) {
        Ok(Response::SignedPeers(list)) => {
            let (presences, forged) = verified_presences(list);
            let found = presences
                .iter()
                .map(|signed| (signed.presence.device, signed.presence.address.clone()))
                .collect();
            Ok(Located {
                found,
                forged,
                signed: true,
                presences,
                claimed: Vec::new(),
            })
        }
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        Err(failed) if !accept_unsigned => Err(CliError::Usage(format!(
            "the coordinator signed its list before and now hangs up on the question \
             ({failed}); an unsigned list from it is not read"
        ))),
        Err(_) => {
            *client = dial_as(config, device)?;
            match client.ask(&Request::Peers { user })? {
                Response::Peers(list) => Ok(Located {
                    found: list
                        .into_iter()
                        .filter(|presence| {
                            !presence.address.is_empty()
                                && presence.address.len() <= MAX_ADDRESS_LEN
                        })
                        .map(|presence| (presence.device, presence.address))
                        .collect(),
                    forged: 0,
                    signed: false,
                    presences: Vec::new(),
                    claimed: Vec::new(),
                }),
                Response::Refused(why) => Err(CliError::Usage(why)),
                other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
            }
        }
    }
}

/// The presences their own device signed, as `(device, address)`, and how many
/// were dropped.
///
/// A coordinator relays presences; it does not make them. One that alters an
/// address, or lists a device at an address that device never published, can
/// send every machine of an account to a place of its choosing -- TLS pinning
/// refuses the connection there, but only after a connect timeout spent on it,
/// every round, for every machine. What is dropped is exactly what a
/// coordinator could otherwise have invented.
#[must_use]
pub fn verified(list: Vec<SignedPresence>) -> (Vec<(DeviceId, String)>, usize) {
    let (kept, dropped) = verified_presences(list);
    let kept = kept
        .into_iter()
        .map(|signed| (signed.presence.device, signed.presence.address))
        .collect();
    (kept, dropped)
}

/// As [`verified`], keeping the signed presences themselves.
#[must_use]
pub fn verified_presences(list: Vec<SignedPresence>) -> (Vec<SignedPresence>, usize) {
    let offered = list.len();
    let kept: Vec<SignedPresence> = list
        .into_iter()
        .filter(|signed| signed.verify_origin().is_ok())
        .collect();
    let dropped = offered - kept.len();
    (kept, dropped)
}

/// The rows that are live devices of `owner` at addresses they published,
/// and how many were dropped.
///
/// [`verified_presences`] checks where a device is; this also checks whose it
/// is ([`ClaimedPresence::verify_for`]). A coordinator could otherwise list
/// other accounts' genuine machines under this one -- each a connect timeout,
/// and each, once gossip relays what the book holds, handed on as this
/// account's.
#[must_use]
pub fn verified_claimed(
    list: Vec<ClaimedPresence>,
    owner: UserId,
) -> (Vec<ClaimedPresence>, usize) {
    let offered = list.len();
    let kept: Vec<ClaimedPresence> = list
        .into_iter()
        .filter(|row| row.verify_for(owner).is_ok())
        .collect();
    let dropped = offered - kept.len();
    (kept, dropped)
}

/// Machines of other accounts that offer space, from the coordinator: how a
/// member with no typed address and nobody on their LAN finds a host at all.
///
/// Each row is checked against the account its own claim names, and is dialled
/// with its device pinned, as a stranger found on a LAN is. Nothing private is
/// kept, also inside a `relay:` address: a list from outside must not send this
/// machine to its own network. A coordinator older than this hangs up, which is
/// an empty list.
///
/// # Errors
///
/// If the coordinator cannot be reached.
pub fn hosts(node: &Node) -> Result<Vec<(DeviceId, String)>> {
    let mut client = dial(node)?;
    let Ok(Response::Hosts(list)) = client.ask(&Request::Hosts) else {
        return Ok(Vec::new());
    };
    Ok(list
        .into_iter()
        .take(itsanas_coord::protocol::MAX_HOSTS_RETURNED)
        .filter(|row| {
            let owner = row.claim.claim.owner;
            owner != node.store.owner()
                && row.claim.claim.pledged_bytes > 0
                && row.verify_for(owner).is_ok()
                && dialable_from_anywhere(&row.presence.presence.address)
        })
        .map(|row| (row.presence.presence.device, row.presence.presence.address))
        .collect())
}

/// Whether an address could work from another network, looking through a
/// `relay:` to the address it carries. A relay with none is not.
fn dialable_from_anywhere(address: &str) -> bool {
    match address.strip_prefix(crate::contact::RELAY_PREFIX) {
        Some(relayed) => relayed
            .split_once('@')
            .is_some_and(|(_, at)| !at.is_empty() && !is_private_address(at)),
        None => !is_private_address(address),
    }
}

/// Every device enrolled under this account, reachable or not.
///
/// `Ok(None)` means the coordinator is older than `Request::Devices`: it cannot
/// decode the message and closes the connection. A dropped connection looks
/// identical from here, and this used to read both as "too old" -- so a
/// timeout on an up-to-date coordinator made `device list` quietly show the
/// short list and blame the coordinator's version. Now a failed `Devices` is
/// followed by `Peers`, which every coordinator has always answered: if that
/// works, the coordinator really is older; if it fails too, the connection is
/// the problem and is reported as one.
///
/// # Errors
///
/// If the coordinator cannot be reached, refuses, or answers with something
/// else.
pub fn enrolled(node: &Node) -> Result<Option<Vec<EnrolledDevice>>> {
    enrolled_as(&node.config, &node.device, node.store.owner())
}

/// [`enrolled`] without an open store: for a step that must not wait for the
/// daemon to let the store go (copying the account to a key).
///
/// # Errors
///
/// As [`enrolled`].
pub fn enrolled_as(
    config: &crate::config::Config,
    device: &DeviceKeys,
    user: UserId,
) -> Result<Option<Vec<EnrolledDevice>>> {
    let mut client = dial_as(config, device)?;
    match client.ask(&Request::Devices { user }) {
        Ok(Response::Devices(list)) => Ok(Some(list)),
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        Err(failed) => match devices_as(config, device, user) {
            Ok(_) => Ok(None),
            Err(_) => Err(CliError::Usage(format!(
                "the coordinator stopped answering ({failed}); it is not a version question"
            ))),
        },
    }
}

/// Ask the coordinator what this account's other machines pledge, and remember
/// it for the write bound ([`Node::account_pledge`]).
///
/// Returns the sum, or `None` when the coordinator is too old to list devices,
/// in which case the figure remembered last is left as it was. This machine's
/// own entry is left out: its pledge is read from its configuration, which is
/// current, where the coordinator's copy is whatever it last claimed.
///
/// # Errors
///
/// As [`enrolled`], or if the figure cannot be written.
pub fn refresh_others_pledged(node: &Node) -> Result<Option<u64>> {
    Ok(other_pledges(node)?.map(|pledges| pledges.total))
}

/// What the account's other machines pledge, as [`other_pledges`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtherPledges {
    /// All of them together, which is what earns the account its room.
    pub total: u64,
    /// The largest one machine offers: what a new machine can be told "the
    /// same as your others" about.
    pub largest: u64,
}

/// [`refresh_others_pledged`], with the largest pledge beside the sum.
///
/// # Errors
///
/// As [`refresh_others_pledged`].
pub fn other_pledges(node: &Node) -> Result<Option<OtherPledges>> {
    let Some(devices) = enrolled(node)? else {
        return Ok(None);
    };
    let total = remember_pledges(node, &devices)?;
    let mine = node.device.device_id();
    let largest = devices
        .iter()
        .filter(|enrolled| enrolled.device != mine)
        .map(|enrolled| enrolled.pledged_bytes)
        .max()
        .unwrap_or(0);
    Ok(Some(OtherPledges { total, largest }))
}

pub(super) fn remember_pledges(node: &Node, devices: &[EnrolledDevice]) -> Result<u64> {
    let mine = node.device.device_id();
    let others = devices
        .iter()
        .filter(|enrolled| enrolled.device != mine)
        .fold(0u64, |total, enrolled| {
            total.saturating_add(enrolled.pledged_bytes)
        });
    node.remember_others_pledged(others)?;
    Ok(others)
}

/// Find another member's machines by their name.
///
/// Two requests the coordinator has always answered and nothing ever sent.
/// `Lookup` turns a username into an account, `Peers` turns an account into the
/// addresses its devices published -- and between them they are the difference
/// between "I need my friend's IP address and port" and "I need my friend's
/// name".
///
/// Until this existed, hosting for somebody on another network meant one of
/// them reading an address to the other and both typing `itsanas peer add`. On
/// the same network the discovery beacons do it already; across networks there
/// was nothing, which is what made the fleet a set of arranged pairs rather
/// than a network.
///
/// What this does *not* do is decide who to host with. It answers a question
/// somebody asked by name. Choosing partners automatically is a policy this
/// project has not settled, and guessing at it here would settle it by
/// accident.
///
/// # Errors
///
/// If the coordinator cannot be reached, does not know the name, or answers
/// with something else.
pub fn find_member(node: &Node, username: &str) -> Result<(UserId, Vec<(DeviceId, String)>)> {
    let mut client = dial(node)?;

    let account = match client.ask(&Request::Lookup {
        username: username.to_owned(),
    })? {
        Response::Account(account) => *account,
        Response::Missing => {
            return Err(CliError::Usage(format!(
                "the coordinator has no member called {username:?}"
            )));
        }
        Response::Refused(why) => return Err(CliError::Usage(why)),
        other => return Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    };

    let user = account.user.id;
    if user == node.store.owner() {
        return Err(CliError::Usage(format!(
            "{username:?} is this account. Its own devices are found already."
        )));
    }

    let read = located(&mut client, &node.config, &node.device, user, true)?;
    Ok((user, read.found))
}

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::{NodeClaim, Presence};
    use itsanas_crypto::SecretBytes;

    use super::*;

    /// A coordinator relays presences; it does not make them. Two ways to pass
    /// one off: change the address after the device signed it, or sign with
    /// another key and put the device's id on top. Both are dropped and
    /// counted. The genuine one is dated 1970, as a Pi with no real-time clock
    /// dates it, and is kept: the check is who signed, never when.
    #[test]
    fn red_team_a_presence_its_device_did_not_sign_is_dropped() {
        let pi = DeviceKeys::from_seed(&SecretBytes::new([2; 32]));
        let stranger = DeviceKeys::from_seed(&SecretBytes::new([3; 32]));
        let genuine = Presence {
            device: pi.device_id(),
            address: "192.168.1.20:9797".to_owned(),
            at_unix: 0,
            head: 0,
        }
        .sign(&pi);

        let mut moved = genuine.clone();
        moved.presence.address = "203.0.113.66:9797".to_owned();
        let mut borrowed = Presence {
            device: stranger.device_id(),
            address: "203.0.113.67:9797".to_owned(),
            at_unix: 0,
            head: 0,
        }
        .sign(&stranger);
        borrowed.presence.device = pi.device_id();

        let (kept, dropped) = verified(vec![moved, genuine, borrowed]);
        assert_eq!(
            kept,
            vec![(pi.device_id(), "192.168.1.20:9797".to_owned())],
            "an address the Pi never signed was believed, or the one it did sign was not"
        );
        assert_eq!(dropped, 2, "a forgery was dropped without being counted");
    }

    /// A signed presence says where a machine is, not whose. A coordinator --
    /// or, once gossip exists, a relay -- that lists another account's genuine
    /// machine under this one passes every signature check on the presence;
    /// only the owner's claim tells. Kept, it costs a connect timeout per
    /// round, and gossip would hand it on as this account's.
    #[test]
    fn red_team_a_coordinator_cannot_pass_off_another_accounts_machine_as_yours() {
        use itsanas_crypto::{MasterSecret, UserKeys};
        let row = |owner: &UserKeys, seed: u8, address: &str| {
            let machine = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
            ClaimedPresence {
                presence: Presence {
                    device: machine.device_id(),
                    address: address.to_owned(),
                    at_unix: 0,
                    head: 0,
                }
                .sign(&machine),
                claim: NodeClaim {
                    owner: owner.user_id(),
                    device: machine.device_id(),
                    pledged_bytes: 0,
                    issued_unix: 0,
                    revoked: false,
                }
                .sign(owner),
            }
        };
        let me = UserKeys::derive(&MasterSecret::from_bytes([1; 32]));
        let them = UserKeys::derive(&MasterSecret::from_bytes([2; 32]));
        let mine = row(&me, 2, "192.168.1.20:9797");
        let theirs = row(&them, 3, "203.0.113.67:9797");

        let (kept, dropped) = verified_claimed(vec![theirs, mine.clone()], me.user_id());
        assert_eq!(
            kept,
            vec![mine],
            "another account's machine was listed as this account's, or this account's own was lost"
        );
        assert_eq!(dropped, 1, "the lie was dropped without being counted");
    }

    #[test]
    fn an_address_that_only_its_own_lan_can_dial_is_tried_last() {
        // THE SCENARIO: the laptop is at a friend's house. The coordinator
        // hands it the account's four devices, three of them at home on
        // 192.168.1.x. Dialling those first spends the round's budget and its
        // connection timeouts on addresses that cannot answer, and the one
        // machine that published a name -- the one that can -- is reached last
        // or not at all.
        let ids: Vec<DeviceId> = (0..4).map(|n| DeviceId::from_bytes([n; 32])).collect();
        let mut peers = vec![
            (ids[0], "192.168.1.10:9797".to_owned()),
            (ids[1], "192.168.1.11:9797".to_owned()),
            (ids[2], "nas.example.org:9801".to_owned()),
            (ids[3], "[2001:db8::1]:9797".to_owned()),
        ];
        reachable_first(&mut peers);

        let order: Vec<&str> = peers.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "nas.example.org:9801",
                "[2001:db8::1]:9797",
                "192.168.1.10:9797",
                "192.168.1.11:9797"
            ],
            concat!(
                "addresses reachable from another network must be dialled ",
                "first, and the coordinator's own order kept within each kind"
            )
        );
    }

    #[test]
    fn the_private_ranges_a_home_actually_uses_are_all_recognised() {
        for address in [
            "192.168.1.10:9797",
            "10.0.0.5:9797",
            "172.16.4.4:9797",
            "127.0.0.1:9797",
            "169.254.3.3:9797",
            // Carrier-grade NAT: a mobile network, and what an overlay VPN
            // hands out. Publishing one of those tells a member in another
            // house to dial a machine inside somebody else's overlay.
            "100.90.54.102:9797",
            "[fd00::1]:9797",
            "[fe80::1]:9797",
        ] {
            assert!(
                is_private_address(address),
                "{address} can only be dialled from its own network and must sort last"
            );
        }
        for address in ["nas.example.org:9801", "203.0.113.7:9801", "[2001:db8::1]:9797"] {
            assert!(
                !is_private_address(address),
                "{address} is how somebody in another house reaches this one"
            );
        }
    }
}
