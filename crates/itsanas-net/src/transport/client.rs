//! The dialling side: connecting to a peer directly or through a relay, and
//! what an offer to store came back as.

use std::{
    net::{SocketAddr, ToSocketAddrs},
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceId, DeviceKeys, UserId};
use itsanas_tls::{Authenticated, Identity, reach::connect_to_one_of};
use itsanas_wire::Connection;

use crate::{
    error::{NetError, Result},
    protocol::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, Request, Response},
};

use super::{Budgeted, IO_TIMEOUT};

/// What a peer did with something offered for storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// It stored it.
    Taken,
    /// It already had it, which is ordinary: re-offering a log tip is.
    AlreadyHeld,
    /// It said no.
    ///
    /// Kept apart from `AlreadyHeld`, which it used to share a `false` with, so
    /// a host refusing everything read exactly like a round with nothing to do.
    Refused(Refusal),
}

/// Why a peer refused an offer.
///
/// Two cases rather than the peer's sentence, so a report stays `Copy` and a
/// hostile peer's text never reaches a log line through this path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It has no room left under its pledge, or pledged nothing.
    PledgeFull,
    /// Any other refusal: a segment that does not verify or does not chain.
    Rejected,
    /// It has room, but not for this account: the account has stored there
    /// what its pledge earns (§8 1c).
    ///
    /// Kept apart from [`Self::Rejected`], which it used to read as, because
    /// the two call for opposite answers. A rejection says something was
    /// wrong with what was sent. This says the account's share on that host
    /// is spent: the owner can pledge more, or let the host evict copies it
    /// has to spare ([`PeerClient::may_evict`]), and the host is no worse a
    /// place for the account's data than before.
    AccountFull,
    /// The host has not confirmed this device of another account with its
    /// coordinator lately (`UNCONFIRMED`): nothing was wrong with what was
    /// sent. Read as [`Self::Rejected`], it sent a session after segments and
    /// chains when the answer was a coordinator (`tests/route.rs`, 2026-10-30).
    Unconfirmed,
    /// The coordinator says this device was withdrawn by its account.
    Withdrawn,
    /// This account's devices failed the host's storage audits.
    Contradicted,
}

impl Offer {
    pub(super) fn from_response(answer: Response) -> Result<Self> {
        match answer {
            Response::Stored { accepted: true } => Ok(Self::Taken),
            Response::Stored { accepted: false } => Ok(Self::AlreadyHeld),
            Response::Refused(reason) if reason == crate::service::PLEDGE_EXHAUSTED => {
                Ok(Self::Refused(Refusal::PledgeFull))
            }
            // A prefix, not an equality: the host appends what the account
            // holds and what it earned, for the operator's log.
            Response::Refused(reason) if reason.starts_with(crate::service::ACCOUNT_FULL) => {
                Ok(Self::Refused(Refusal::AccountFull))
            }
            Response::Refused(reason) if reason == crate::service::UNCONFIRMED => {
                Ok(Self::Refused(Refusal::Unconfirmed))
            }
            Response::Refused(reason) if reason == crate::service::WITHDRAWN => {
                Ok(Self::Refused(Refusal::Withdrawn))
            }
            Response::Refused(reason) if reason == crate::service::CONTRADICTED => {
                Ok(Self::Refused(Refusal::Contradicted))
            }
            Response::Refused(_) => Ok(Self::Refused(Refusal::Rejected)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }
}

/// A connection to a peer, from the asking side.
pub struct PeerClient {
    pub(super) connection: Connection<itsanas_tls::session::ClientStream<Budgeted>>,
    peer_device: DeviceId,
    /// The version both sides agreed to speak.
    ///
    /// Not a formality: a verb added after that version does not exist for this
    /// peer, and sending it would fail the connection rather than the feature.
    /// Every caller of a newer request asks this first.
    pub(super) spoken: u16,
    /// The address the socket reached, for a direct session only: over a
    /// splice it would be the relay's, which says nothing about the peer.
    direct: Option<std::net::IpAddr>,
    /// The relay this session went through, when it did.
    via: Option<std::net::IpAddr>,
}

impl std::fmt::Debug for PeerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerClient")
            .field("peer_device", &self.peer_device)
            .finish_non_exhaustive()
    }
}

impl PeerClient {
    /// Connect, authenticate, and complete the opening exchange.
    ///
    /// `expect` pins which device must answer. Pass it whenever the identity is
    /// known — addresses come from the coordinator, and the coordinator is not
    /// trusted to say who lives at one.
    pub fn connect(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
    ) -> Result<Self> {
        Self::connect_within(address, device, owner, expect, None)
    }

    /// [`Self::connect`], with the whole conversation bounded by `budget`,
    /// handshake included: once it is spent every read and write fails, so a
    /// peer that stalls cannot hold the caller past it. The daemon passes
    /// [`PEER_SESSION_BUDGET`](super::PEER_SESSION_BUDGET).
    pub fn connect_within(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
        budget: Option<Duration>,
    ) -> Result<Self> {
        Self::dial(address, device, owner, expect, budget, PROTOCOL_VERSION)
    }

    /// [`Self::connect`] as a node of an older release would: offering
    /// `offering` and speaking no verb added after it. For the tests that keep
    /// every gate working against the oldest version in the window (HANDOVER
    /// §8 18); a real node always offers [`PROTOCOL_VERSION`].
    #[doc(hidden)]
    pub fn connect_offering(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
        offering: u16,
    ) -> Result<Self> {
        Self::dial(address, device, owner, expect, None, offering)
    }

    fn dial(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
        budget: Option<Duration>,
        offering: u16,
    ) -> Result<Self> {
        let addresses: Vec<SocketAddr> = address.to_socket_addrs()?.collect();

        let stream = connect_to_one_of(&addresses)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let direct = stream.peer_addr().ok().map(|address| address.ip());
        let until = budget.map(|budget| Instant::now() + budget);
        Self::establish(
            Budgeted {
                stream: Box::new(stream),
                until,
            },
            device,
            owner,
            expect,
            direct,
            offering,
        )
    }

    /// Reach `target` through `relay`, a peer it has parked on (§8 0v).
    ///
    /// The relay is dialled and proven like any peer, asked to splice, and
    /// then a second, end-to-end session is run over the splice, with
    /// `target` pinned: the relay passes ciphertext it cannot read, and a
    /// relay answering in `target`'s place fails that session's proof.
    ///
    /// # Errors
    ///
    /// The relay's refusal, a relay older than
    /// [`crate::protocol::PROTOCOL_WITH_RELAY`], or anything `connect` fails on.
    pub fn connect_relayed(
        relay: impl ToSocketAddrs,
        relay_device: DeviceId,
        target: DeviceId,
        device: &DeviceKeys,
        owner: UserId,
        budget: Option<Duration>,
    ) -> Result<Self> {
        let mut outer = Self::connect_within(relay, device, owner, Some(relay_device), budget)?;
        if outer.spoken < crate::protocol::PROTOCOL_WITH_RELAY {
            return Err(NetError::Refused(crate::relay::RELAY_TOO_OLD.to_owned()));
        }
        match outer.request(&Request::Splice { to: target })? {
            Response::Spliced => {}
            Response::Refused(reason) => return Err(NetError::Refused(reason)),
            _ => {
                return Err(NetError::UnexpectedResponse {
                    expected: "spliced",
                });
            }
        }
        let via = outer.direct;
        let (stream, rest) = outer.connection.into_parts();
        let until = budget.map(|budget| Instant::now() + budget);
        let mut spliced = Self::establish(
            Budgeted {
                stream: Box::new(crate::relay::Prefixed::new(rest, stream)),
                until,
            },
            device,
            owner,
            Some(target),
            None,
            PROTOCOL_VERSION,
        )?;
        spliced.via = via;
        Ok(spliced)
    }

    /// The stream back, and what was read past the last message: a park
    /// becomes a splice (§8 0v).
    pub(crate) fn into_parts(self) -> (itsanas_tls::session::ClientStream<Budgeted>, Vec<u8>) {
        self.connection.into_parts()
    }

    fn establish(
        stream: Budgeted,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
        direct: Option<std::net::IpAddr>,
        offering: u16,
    ) -> Result<Self> {
        let identity = Identity::generate()?;
        let Authenticated { peer, connection } =
            itsanas_tls::connect(&identity.client_config()?, device, stream, expect)?;

        let mut client = Self {
            connection,
            peer_device: peer,
            spoken: MIN_PROTOCOL_VERSION,
            direct,
            via: None,
        };

        // Version negotiation after authentication, so an incompatible peer
        // fails legibly rather than on some later message.
        match client.request(&Request::Hello {
            protocol: offering,
            device: device.device_id(),
            owner,
        })? {
            Response::Hello { protocol, .. } => {
                if protocol < MIN_PROTOCOL_VERSION {
                    return Err(NetError::UnsupportedProtocolVersion {
                        found: protocol,
                        supported: PROTOCOL_VERSION,
                    });
                }
                // Capped at what this node knows how to speak. A peer answering
                // with something higher is newer than us and has agreed to come
                // down; believing its number would have us using verbs we do
                // not have.
                client.spoken = protocol.min(offering);
            }
            Response::Refused(reason) => return Err(NetError::Refused(reason)),
            _ => {
                return Err(NetError::UnexpectedResponse { expected: "hello" });
            }
        }

        Ok(client)
    }

    /// The device on the other end, as it *proved* itself — not as it claimed.
    #[must_use]
    pub const fn peer_device(&self) -> DeviceId {
        self.peer_device
    }

    /// The address this session reached the peer at, if it reached it
    /// directly; `None` through a relay.
    #[must_use]
    pub const fn direct_address(&self) -> Option<std::net::IpAddr> {
        self.direct
    }

    /// The address that says which network this peer counts under: its own
    /// when dialled directly, its relay's otherwise. Parked machines in one
    /// house are one household, and ten emulators behind one relay must not
    /// read as ten networks.
    #[must_use]
    pub const fn network_address(&self) -> Option<std::net::IpAddr> {
        match self.direct {
            Some(address) => Some(address),
            None => self.via,
        }
    }

    /// The protocol version both sides settled on.
    #[must_use]
    pub const fn spoken(&self) -> u16 {
        self.spoken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_that_has_no_room_for_this_account_is_not_reported_as_rejecting_it() {
        // The two refusals call for opposite answers: a rejection means what
        // was sent is wrong, a full account means its share there is spent,
        // and only the second is answered by letting the host evict spares.
        // Sabotage: drop the `ACCOUNT_FULL` arm in `Offer::from_response`.
        let full = format!(
            "{}: 300 of 300 bytes (pledged 700, proved 0)",
            crate::service::ACCOUNT_FULL
        );
        assert_eq!(
            Offer::from_response(Response::Refused(full)).unwrap(),
            Offer::Refused(Refusal::AccountFull),
            "a full account read as a host that rejects what it is sent"
        );
        assert_eq!(
            Offer::from_response(Response::Refused(
                crate::service::PLEDGE_EXHAUSTED.to_owned()
            ))
            .unwrap(),
            Offer::Refused(Refusal::PledgeFull)
        );
        assert_eq!(
            Offer::from_response(Response::Refused(
                crate::service::UPDATE_REQUIRED.to_owned()
            ))
            .unwrap(),
            Offer::Refused(Refusal::Rejected),
            "a refusal that is not about room read as one that is"
        );
    }

    #[test]
    fn red_team_a_host_waiting_on_its_coordinator_is_not_reported_as_rejecting_the_segments() {
        // tests/route.rs, 2026-10-30: a host of another account refused every
        // offer because no coordinator had confirmed the owner's device, and
        // the owner's log said "a segment that does not verify, or does not
        // follow its chain" -- an investigation went after segments and chains
        // when the answer was `itsanas coordinator --device`. Sabotage: drop
        // any one of the three arms in `Offer::from_response`.
        for (text, want) in [
            (crate::service::UNCONFIRMED, Refusal::Unconfirmed),
            (crate::service::WITHDRAWN, Refusal::Withdrawn),
            (crate::service::CONTRADICTED, Refusal::Contradicted),
        ] {
            assert_eq!(
                Offer::from_response(Response::Refused(text.to_owned())).unwrap(),
                Offer::Refused(want),
                "the owner was told its segments were bad when the host said: {text}"
            );
        }
    }
}
