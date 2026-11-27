//! One contact per round -- publish, read, or both, on one connection --
//! and asking whether anybody outside can reach this machine.

use itsanas_coord::claim::{ClaimedPresence, Presence, SignedPresence};
use itsanas_coord::protocol::{Request, Response};
use itsanas_crypto::{DeviceId, DeviceKeys, UserId};

use super::lookup::{located, remember_pledges};
use super::publishing::presence_time;
use super::{devices_as, dial, dial_as, note_heads, published_address, reachable_first};
use crate::contact::Due;
use crate::node::Node;
use crate::{NodeError, NodeError as CliError, Result};

/// What the coordinator found when it tried to reach this machine.
///
/// Three states, and the third is not a polite version of the second. A
/// coordinator that **did not look** -- because this address was asked about
/// within the hour, or because it is private and no coordinator could say
/// anything about it -- has found nothing, and reporting that as "nothing can
/// reach you" sends somebody to rewire a router that works. They were the same
/// value for half a day and using it is what caught the difference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reachability {
    /// It completed a handshake with this device at the published address.
    Reachable(String),
    /// It tried, and could not.
    Unreachable(String),
    /// It did not try, and says why.
    Unknown(String),
}

impl Reachability {
    /// The sentence to show a person.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Reachable(detail) | Self::Unreachable(detail) | Self::Unknown(detail) => detail,
        }
    }
}

/// What one contact with the coordinator brought back.
#[derive(Debug, Default)]
pub struct Contacted {
    /// The address published, if this contact published.
    pub published: Option<String>,
    /// The account's other devices, reachable first, if this contact read
    /// them.
    pub found: Option<Vec<(DeviceId, String)>>,
    /// Why the read failed, when the publication before it went through.
    ///
    /// Kept apart from a failed contact on purpose. Reported as one, the
    /// accepted publication was forgotten with it and repeated every round,
    /// for as long as the coordinator refused the read -- the per-round
    /// heartbeat this module exists to remove, brought back by an error path.
    pub read_failed: Option<NodeError>,
    /// Why the account's pledges could not be remembered, if they could not.
    /// Not a failed contact: the publication and the read went through.
    pub pledges_not_kept: Option<NodeError>,
    /// How many presences the read dropped because their device did not sign
    /// them or their claim was not this account's. Counted because anything
    /// but zero means the coordinator is lying about the account's machines.
    pub forged: usize,
    /// Whether the read was a signed list. `false` with `found` set means the
    /// coordinator answered only `Peers`, and the caller should say so.
    pub signed: bool,
    /// The signed presences behind `found`, for the address book.
    pub presences: Vec<SignedPresence>,
    /// Those of them that came with this account's claim ([`Located::claimed`]).
    ///
    /// [`Located::claimed`]: super::Located::claimed
    pub claimed: Vec<ClaimedPresence>,
}

/// Publish, read, or both, on one connection: whatever [`Due`] asks for.
///
/// A publication also refreshes what the account's other machines pledge
/// ([`Node::account_pledge`]) on the same connection. That was an hourly dial
/// of its own, beside an hourly publication that is already one. A coordinator
/// too old to list devices closes the connection at that request; by then the
/// publication and the read are done, so that costs the refresh and nothing
/// else.
///
/// # Errors
///
/// If the coordinator cannot be reached, refuses, or answers with something
/// else. The halves share one failure, deliberately: they share one
/// connection, and reporting them separately doubled the noise for one cause.
pub fn contact(node: &Node, listen: &str, now: u64, due: &Due) -> Result<Contacted> {
    let mut client = dial(node)?;
    let mut contacted = Contacted::default();

    if due.publish {
        let address = published_address(&node.config, listen, client.local_addr());
        let presence = Presence {
            device: node.store.device_id(),
            address: address.clone(),
            at_unix: presence_time(node, now),
            head: node.store.chain_length().unwrap_or(0),
        }
        .sign(&node.device);
        match client.ask(&Request::Announce(Box::new(presence)))? {
            Response::Done => {}
            Response::Refused(why) => return Err(CliError::Usage(why)),
            other => return Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        }
        contacted.published = Some(address);
    }

    if due.read {
        let read = located(
            &mut client,
            &node.config,
            &node.device,
            node.store.owner(),
            due.accept_unsigned,
        );
        match read {
            Ok(read) => {
                note_heads(node, &read.presences);
                let mut found = read
                    .found
                    .into_iter()
                    .filter(|(device, _)| *device != node.store.device_id())
                    .collect::<Vec<_>>();
                reachable_first(&mut found);
                contacted.found = Some(found);
                contacted.forged = read.forged;
                contacted.signed = read.signed;
                contacted.presences = read.presences;
                contacted.claimed = read.claimed;
            }
            // Nothing was published to keep: the whole contact failed.
            Err(error) if contacted.published.is_none() => return Err(error),
            Err(error) => contacted.read_failed = Some(error),
        }
    }

    if due.publish
        && let Ok(Response::Devices(list)) = client.ask(&Request::Devices {
            user: node.store.owner(),
        })
    {
        contacted.pledges_not_kept = remember_pledges(node, &list).err();
    }

    Ok(contacted)
}

/// Ask the coordinator whether anybody outside can reach this machine.
///
/// The one question a node cannot answer about itself. It knows it can reach
/// the coordinator, because it just did; nothing tells it whether the router in
/// front of it forwards anything, and a member whose forward is wrong is
/// indistinguishable -- to every other member -- from a member who is switched
/// off.
///
/// Asked when the answer can have *changed*: the published address is new, or a
/// person ran `itsanas doctor`. Not on a schedule, because the coordinator
/// spends a real connection on each one.
///
/// `Ok(None)` means the coordinator is too old to answer. That is not an error
/// and must not read as "unreachable": a member told their forward is broken
/// because their coordinator is out of date would go and rewire a router that
/// was working.
///
/// # Errors
///
/// If the coordinator cannot be reached at all, or answers with something else.
pub fn check_me(node: &Node) -> Result<Option<Reachability>> {
    check_me_as(&node.config, &node.device, node.store.owner())
}

/// The same, without an open store. See [`dial_as`].
///
/// # Errors
///
/// As [`check_me`].
pub fn check_me_as(
    config: &crate::config::Config,
    device: &DeviceKeys,
    owner: UserId,
) -> Result<Option<Reachability>> {
    let mut client = dial_as(config, device)?;
    match client.ask(&Request::CheckMe) {
        Ok(Response::Reachable { reachable, detail }) => Ok(Some(if reachable {
            Reachability::Reachable(detail)
        } else {
            Reachability::Unreachable(detail)
        })),
        Ok(Response::Unknown(why)) => Ok(Some(Reachability::Unknown(why))),
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        // A coordinator that does not know this request closes the connection
        // rather than answering, which is indistinguishable from an outage
        // until something that *is* known succeeds. `Devices` learnt this
        // first; the shape is the same.
        Err(failed) => match devices_as(config, device, owner) {
            Ok(_) => Ok(None),
            Err(_) => Err(CliError::Usage(format!(
                "the coordinator stopped answering ({failed}); it is not a version question"
            ))),
        },
    }
}
