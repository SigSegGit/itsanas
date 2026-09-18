//! Talking to a coordinator: registering, publishing an address, escrow.
//!
//! # What this buys, and what it costs
//!
//! Two things machines on one network do not need: reaching a peer somewhere
//! else, and recovering an account from a passphrase alone. Everything else the
//! coordinator once did has been removed — see `docs/DESIGN.md` §8 — so a node
//! with no coordinator configured is a fully working node with a smaller reach.
//!
//! The cost is the honest one: a coordinator sees who is online and who asks
//! after whom, and it holds an escrow blob that can be attacked offline by
//! anyone who steals its database. Escrow is therefore opt-in, and withdrawing
//! it is one command.

use std::net::SocketAddr;

use itsanas_coord::claim::{NodeClaim, Presence};
use itsanas_coord::directory::Registration;
use itsanas_coord::invitation::{Invitation, SECRET_LEN, Secret};
use itsanas_coord::protocol::{EnrolledDevice, Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, KdfParams, Keystore, UserId};

use crate::node::{ESCROW_LABEL, Node};
use crate::{NodeError as CliError, Result};

/// Open a connection to the node's configured coordinator.
///
/// The device id is pinned when the configuration names one, so an address
/// resolving to a different machine is refused rather than trusted. A
/// coordinator address is configuration; configuration is not a promise about
/// who lives there.
pub fn dial(node: &Node) -> Result<CoordClient> {
    let Some(address) = node.config.coordinator.as_deref() else {
        return Err(CliError::Usage(
            "no coordinator configured; run `itsanas coordinator <host:port>`".to_owned(),
        ));
    };

    let expect = node
        .config
        .coordinator_device
        .as_deref()
        .map(parse_device)
        .transpose()?;

    CoordClient::connect(address, &node.device, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))
}

/// Read a device id from its hexadecimal form.
pub fn parse_device(hex: &str) -> Result<DeviceId> {
    let mut bytes = [0u8; 32];
    if hex.len() != 64 {
        return Err(CliError::Usage(format!(
            "a device id is 64 hexadecimal characters; got {}",
            hex.len()
        )));
    }
    // The length check above makes the split exact.
    let (pairs, _) = hex.as_bytes().as_chunks::<2>();
    for (index, pair) in pairs.iter().enumerate() {
        let text = std::str::from_utf8(pair)
            .map_err(|_| CliError::Usage("a device id must be hexadecimal".to_owned()))?;
        bytes[index] = u8::from_str_radix(text, 16)
            .map_err(|_| CliError::Usage("a device id must be hexadecimal".to_owned()))?;
    }
    Ok(DeviceId::from_bytes(bytes))
}

/// Draw a secret, sign an invitation for it, and lodge it.
///
/// The secret is returned so the caller can print it once. It never goes to
/// disk and the coordinator only ever sees its hash, so this is the single
/// moment it exists anywhere it can be read.
pub fn invite(node: &Node, uses: u32, validity: u64, now: u64) -> Result<Secret> {
    if uses == 0 {
        return Err(CliError::Usage(
            "an invitation good for no uses is not an invitation".to_owned(),
        ));
    }

    let mut secret = [0u8; SECRET_LEN];
    getrandom::fill(&mut secret)
        .map_err(|error| CliError::Usage(format!("could not draw a secret: {error}")))?;

    let signed = Invitation {
        inviter: node.store.owner(),
        code: itsanas_coord::code_id(&secret),
        issued_unix: now,
        expires_unix: now.saturating_add(validity),
        uses,
    }
    .sign(&node.user);

    let mut client = dial(node)?;
    match client.ask(&Request::Invite(Box::new(signed)))? {
        Response::Done => Ok(secret),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Turn a secret into something a person can send in a message, and back.
///
/// Hex, because it survives every chat client, quoting style and font this will
/// be pasted through, and because a code that a person mistypes must fail rather
/// than silently mean something else.
#[must_use]
pub fn encode_secret(secret: &Secret) -> String {
    secret
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Read a code somebody was sent.
pub fn decode_secret(text: &str) -> Result<Secret> {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if cleaned.len() != SECRET_LEN * 2 {
        return Err(CliError::Usage(format!(
            "an invitation code is {} hexadecimal characters; this one is {}",
            SECRET_LEN * 2,
            cleaned.len()
        )));
    }
    let mut secret = [0u8; SECRET_LEN];
    // The length check above makes the split exact.
    let (pairs, _) = cleaned.as_bytes().as_chunks::<2>();
    for (index, pair) in pairs.iter().enumerate() {
        let text = std::str::from_utf8(pair)
            .map_err(|_| CliError::Usage("an invitation code is hexadecimal".to_owned()))?;
        secret[index] = u8::from_str_radix(text, 16)
            .map_err(|_| CliError::Usage("an invitation code is hexadecimal".to_owned()))?;
    }
    Ok(secret)
}

/// Register the account name and enrol this device.
///
/// Both are signed by keys the coordinator does not hold, so the worst it can
/// do is refuse — which is denial of service and already in the threat model.
/// Register, presenting an invitation code if one was given.
///
/// A coordinator that admits openly ignores the code; one that admits by
/// invitation refuses without it, unless this account is already a member.
pub fn register_with(node: &Node, invite: Option<&Secret>, now: u64) -> Result<()> {
    let mut client = dial(node)?;

    let registration = Registration {
        username: node.config.username.clone(),
        user: node.user.public(),
        issued_unix: now,
    }
    .sign(&node.user);

    let ask = match invite {
        Some(secret) => Request::RegisterInvited {
            registration: Box::new(registration),
            secret: *secret,
        },
        None => Request::Register(Box::new(registration)),
    };

    match client.ask(&ask)? {
        Response::Account(_) => {}
        Response::Refused(why) => return Err(CliError::Usage(why)),
        other => return Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }

    let claim = NodeClaim {
        owner: node.store.owner(),
        device: node.store.device_id(),
        pledged_bytes: node.config.pledge_bytes,
        issued_unix: now,
        revoked: false,
    }
    .sign(&node.user);

    match client.ask(&Request::Claim(Box::new(claim)))? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// The address to publish for this device.
///
/// A node listening on `0.0.0.0:9797` — the default, and the right default —
/// must not tell the coordinator that this *is* its address. `0.0.0.0` is where
/// to accept connections from; it is not somewhere to dial. A peer that looked
/// it up got an address it could not use, and `itsanas register` printed
/// `announced 0.0.0.0:9797` as though that had worked. The comment above that
/// call says a device nobody can reach has not really joined anything, which is
/// exactly what it had just arranged.
///
/// So when the configured address is unspecified, publish the local end of the
/// connection that just reached the coordinator: among this machine's
/// addresses, that is the one demonstrably able to talk to it. The port stays
/// the configured one — the listening port, not the ephemeral port this
/// particular connection went out from.
///
/// **This is still wrong behind NAT**, where the address a peer needs is the
/// router's and only the coordinator can observe it. The fix for that is the
/// coordinator recording the source address it saw, which is a protocol change
/// and is safe for the same reason this is: members pin the device id, so an
/// address leading to the wrong machine is refused rather than trusted.
#[must_use]
pub fn reachable_address(configured: &str, local: SocketAddr) -> String {
    let Ok(parsed) = configured.parse::<SocketAddr>() else {
        // Not an address literal, so it is a hostname somebody chose on
        // purpose. Substituting an IP for it would be overruling them.
        return configured.to_owned();
    };
    if parsed.ip().is_unspecified() {
        SocketAddr::new(local.ip(), parsed.port()).to_string()
    } else {
        configured.to_owned()
    }
}

/// What this node tells the coordinator, which is not always where it listens.
///
/// `announce` in the configuration wins, verbatim and including its port: it is
/// the address that reaches this machine from *another* network, and the whole
/// reason it exists is that it differs from the local one. A forwarded port
/// maps an outside port to a different inside port, so replacing the port with
/// the listening one would publish an address that is wrong in exactly the case
/// the setting is for.
///
/// Without it, the old behaviour: the local end of the connection that reached
/// the coordinator. That is right for a house where everyone is on one LAN and
/// right for a machine that moves -- a laptop at a friend's house has no
/// address anybody elsewhere can dial, and saying so honestly is better than
/// inventing one. Such a machine takes part by dialling out; one reachable side
/// per pair is enough.
#[must_use]
pub fn published_address(
    config: &crate::config::Config,
    listen: &str,
    local: SocketAddr,
) -> String {
    match config.announce.as_deref() {
        Some(announce) => announce.to_owned(),
        None => reachable_address(listen, local),
    }
}

/// Publish where this device can be reached, and return what was published.
///
/// Signed by the device, not by the owner: a laptop moving between networks
/// announces constantly and must never need the key that can revoke everything.
///
/// Returns the address rather than the unit, so that a caller reporting what
/// happened reports what was sent instead of what it asked for. `register`
/// printed the configured value and was wrong whenever the two differed.
pub fn announce(node: &Node, address: &str, now: u64) -> Result<String> {
    let mut client = dial(node)?;
    let address = published_address(&node.config, address, client.local_addr());
    let presence = Presence {
        device: node.store.device_id(),
        address: address.clone(),
        at_unix: now,
    }
    .sign(&node.device);

    match client.ask(&Request::Announce(Box::new(presence)))? {
        Response::Done => Ok(address),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Where the other devices of `user` say they are.
pub fn peers(node: &Node, user: UserId) -> Result<Vec<(DeviceId, String)>> {
    let mut found: Vec<(DeviceId, String)> = devices(node, user)?
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

/// Whether an address is one that only its own network can dial.
///
/// A name is never private: it is the thing somebody set up precisely so that
/// others could reach them, and what it resolves to is a question for the
/// resolver, not for this.
///
/// **Where this is deliberately wrong:** an overlay network -- Tailscale,
/// Nebula, any `WireGuard` mesh -- hands out addresses in `100.64.0.0/10`, and
/// inside such a network they are reachable from anywhere. Counted here as
/// private, so they sort last. That is the right default, because the same
/// range is what a mobile carrier hands a phone behind CGNAT and that address
/// is reachable from nothing; and it costs only an ordering. Anybody running
/// this over an overlay should give the node a **name** for that address, which
/// is what names are for and what this function trusts.
#[must_use]
pub fn is_private_address(address: &str) -> bool {
    let Ok(parsed) = address.parse::<SocketAddr>() else {
        return false;
    };
    match parsed.ip() {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                // 100.64.0.0/10, carrier-grade NAT: what a mobile network and
                // an overlay such as Tailscale hand out. `Ipv4Addr::is_shared`
                // says this and is still unstable.
                || (a == 100 && (64..128).contains(&b))
        }
        std::net::IpAddr::V6(v6) => {
            let [a, b, ..] = v6.octets();
            v6.is_loopback()
                // fc00::/7 unique local, and fe80::/10 link local. Both are
                // `Ipv6Addr` methods that are still unstable.
                || (a & 0xfe) == 0xfc
                || (a == 0xfe && (b & 0xc0) == 0x80)
        }
    }
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
    let mut client = dial(node)?;
    match client.ask(&Request::Peers { user })? {
        Response::Peers(list) => Ok(list
            .into_iter()
            .map(|presence| (presence.device, presence.address))
            .collect()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
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
    let mut client = dial(node)?;
    match client.ask(&Request::Devices {
        user: node.store.owner(),
    }) {
        Ok(Response::Devices(list)) => Ok(Some(list)),
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        Err(failed) => match devices(node, node.store.owner()) {
            Ok(_) => Ok(None),
            Err(_) => Err(CliError::Usage(format!(
                "the coordinator stopped answering ({failed}); it is not a version question"
            ))),
        },
    }
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

    match client.ask(&Request::Peers { user })? {
        Response::Peers(list) => Ok((
            user,
            list.into_iter()
                .map(|presence| (presence.device, presence.address))
                .collect(),
        )),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Withdraw a device from this account.
///
/// A `NodeClaim` carries a `revoked` flag and the directory has honoured it
/// since it was written -- `a_revoked_device_leaves_the_live_set` covers it --
/// but nothing could send one. So a device that was lost, reinstalled or sold
/// stayed in the directory for ever, and every other machine on the account
/// kept dialling it every round and being correctly refused by the pinning:
///
/// ```text
/// 192.168.1.142:9797: unreachable
///   (tls: expected to reach device 393f7d4acf72 but d5af6664ae53 answered)
/// ```
///
/// The claim is signed by the *user* key, not the device's, which is what makes
/// this possible at all: a machine that has been lost cannot sign its own
/// withdrawal.
///
/// # Errors
///
/// If the coordinator cannot be reached, or refuses the claim.
pub fn forget_device(node: &Node, device: DeviceId, now: u64) -> Result<()> {
    let claim = NodeClaim {
        owner: node.store.owner(),
        device,
        // A withdrawn device offers nothing. The field is not read for a
        // revoked claim, and setting it to anything else would be a number
        // that means nothing sitting in a signed record.
        pledged_bytes: 0,
        issued_unix: now,
        revoked: true,
    }
    .sign(&node.user);

    let mut client = dial(node)?;
    match client.ask(&Request::Claim(Box::new(claim)))? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Seal this node's identity under `passphrase` and lodge it with the
/// coordinator, or withdraw what is lodged.
///
/// The container is the same shape as the local keystore but sealed under a
/// **different label**, so a copy of one cannot be substituted for the other.
/// The coordinator holds opaque bytes and never sees a passphrase.
pub fn set_escrow(node: &Node, passphrase: Option<&str>, secrets: &[u8]) -> Result<()> {
    let blob = match passphrase {
        Some(passphrase) => {
            debug_assert!(KdfParams::RECOMMENDED.meets_production_floor());
            Some(
                Keystore::lock(passphrase, ESCROW_LABEL, secrets, KdfParams::RECOMMENDED)?
                    .to_bytes(),
            )
        }
        None => None,
    };

    let mut client = dial(node)?;
    match client.ask(&Request::PutEscrow { blob })? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Fetch and open the escrow container for `username`.
///
/// Used by a machine that has nothing: no device key, no account, no store. The
/// coordinator answers this without authentication because there is nothing
/// left to authenticate with — the rate limit and the Argon2id cost are what
/// stand between a stolen database and an account.
pub fn fetch_escrow(
    address: &str,
    expect: Option<DeviceId>,
    username: &str,
    passphrase: &str,
) -> Result<Vec<u8>> {
    // A throwaway device key: this machine has no identity yet, and the
    // coordinator does not need it to have one.
    let device = itsanas_crypto::DeviceKeys::generate()?;
    let mut client = CoordClient::connect(address, &device, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))?;

    let blob = match client.ask(&Request::GetEscrow {
        username: username.to_owned(),
    })? {
        Response::Escrow(blob) => blob,
        Response::Missing => {
            return Err(CliError::Usage(format!(
                "{address} has no recovery container for {username:?}. Either the \
                 account never lodged one, or it was withdrawn — recover with the \
                 24-word phrase instead."
            )));
        }
        Response::Refused(why) => return Err(CliError::Usage(why)),
        other => return Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    };

    Keystore::from_bytes(&blob)?
        .unlock(passphrase, ESCROW_LABEL)
        .map_err(|_| {
            CliError::Usage(
                "wrong passphrase, or the container has been tampered with. The two \
                 are indistinguishable on purpose."
                    .to_owned(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_announce_is_published_instead_of_the_local_address() {
        // The whole point of the setting. Without it a node behind a router
        // publishes its address on the LAN it is on, which is what every
        // machine in another house cannot use.
        let config = crate::config::Config {
            announce: Some("ngas.fr:9801".to_owned()),
            ..crate::config::Config::default()
        };
        let local = "192.168.1.11:41999".parse().unwrap();
        assert_eq!(
            published_address(&config, "0.0.0.0:9797", local),
            "ngas.fr:9801"
        );
    }

    #[test]
    fn the_announced_port_is_not_replaced_by_the_listening_one() {
        // THE BUG THIS PREVENTS: a port forward exists to map an outside port
        // to a different inside one. `ngas.fr:9801 -> 192.168.1.11:9797` is the
        // normal shape of one. Substituting the listening port here would
        // publish ngas.fr:9797, where the router forwards nothing, and the
        // failure would look like the peer being offline.
        let config = crate::config::Config {
            announce: Some("ngas.fr:9801".to_owned()),
            ..crate::config::Config::default()
        };
        let local = "192.168.1.11:41999".parse().unwrap();
        assert!(published_address(&config, "0.0.0.0:9797", local).ends_with(":9801"));
    }

    #[test]
    fn a_machine_that_moves_still_publishes_where_it_is() {
        // No announce is the right configuration for a laptop: it has no
        // address another network can dial. It must still publish something --
        // announcing is also the heartbeat the coordinator counts availability
        // from, and a node that stopped announcing would be counted as gone.
        let config = crate::config::Config::default();
        let friends_house = "10.42.0.7:41999".parse().unwrap();
        assert_eq!(
            published_address(&config, "0.0.0.0:9797", friends_house),
            "10.42.0.7:9797"
        );
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
            (ids[2], "ngas.fr:9801".to_owned()),
            (ids[3], "[2001:db8::1]:9797".to_owned()),
        ];
        reachable_first(&mut peers);

        let order: Vec<&str> = peers.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "ngas.fr:9801",
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
        for address in ["ngas.fr:9801", "82.67.35.234:9801", "[2001:db8::1]:9797"] {
            assert!(
                !is_private_address(address),
                "{address} is how somebody in another house reaches this one"
            );
        }
    }

    #[test]
    fn a_device_id_round_trips_through_its_hexadecimal_form() {
        let device = DeviceId::from_bytes([0xAB; 32]);
        assert_eq!(parse_device(&device.to_hex()).unwrap(), device);
    }

    #[test]
    fn a_malformed_device_id_is_refused_rather_than_padded() {
        // Somebody pins a coordinator by pasting an id. A short paste that was
        // silently zero-padded would pin the wrong machine and the error would
        // arrive later, somewhere else.
        assert!(parse_device("").is_err());
        assert!(parse_device("abcd").is_err());
        assert!(parse_device(&"z".repeat(64)).is_err());
        assert!(parse_device(&"a".repeat(63)).is_err());
        assert!(parse_device(&"a".repeat(65)).is_err());
    }

    #[test]
    fn an_unspecified_listen_address_is_not_what_gets_published() {
        // The default is `0.0.0.0:9797`, and it is the right default: a node
        // should accept from every interface. Publishing it is a different
        // statement, and a false one — nobody can dial 0.0.0.0. A peer that
        // looked this device up got that string, and `register` printed
        // "announced 0.0.0.0:9797" as though something had been achieved.
        let local = "192.168.1.81:54321".parse().unwrap();
        assert_eq!(
            reachable_address("0.0.0.0:9797", local),
            "192.168.1.81:9797"
        );
    }

    #[test]
    fn the_published_port_is_the_listening_one_not_the_one_dialled_from() {
        // The local end of the connection to the coordinator carries an
        // ephemeral source port. Taking the address from it and the port with
        // it would publish somewhere nothing is listening — which fails later,
        // elsewhere, and looks like a network problem.
        let local = "10.0.0.5:41999".parse().unwrap();
        assert_eq!(reachable_address("0.0.0.0:9797", local), "10.0.0.5:9797");
        assert_eq!(reachable_address("[::]:9797", local), "10.0.0.5:9797");
    }

    #[test]
    fn an_address_somebody_chose_is_left_alone() {
        // Substitution is for the case where the configuration says "anywhere".
        // A specific address, or a hostname, is a decision, and overruling it
        // with whatever interface happened to reach the coordinator would break
        // exactly the setups that were configured on purpose.
        let local = "192.168.1.81:54321".parse().unwrap();
        assert_eq!(
            reachable_address("203.0.113.7:9797", local),
            "203.0.113.7:9797"
        );
        assert_eq!(
            reachable_address("nas.example.org:9797", local),
            "nas.example.org:9797"
        );
    }

    #[test]
    fn the_escrow_label_differs_from_the_local_keystore_label() {
        // The two containers hold the same secrets and live in different places
        // under different threat models. Sharing a label would mean a copy of
        // the coordinator's blob could be dropped in as the local keystore, and
        // the domain separation that makes each one specific would be gone.
        assert_ne!(ESCROW_LABEL, crate::node::KEYSTORE_LABEL);
    }
}
