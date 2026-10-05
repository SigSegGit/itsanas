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

use std::fmt::Write as _;
use std::net::{SocketAddr, ToSocketAddrs};

use itsanas_coord::claim::{
    ClaimedPresence, MAX_ADDRESS_LEN, NodeClaim, Presence, SignedClaim, SignedPresence,
};
// Re-exported for a shell that does not depend on the coordinator crate (the
// Android app) and has to say the limit and carry an invitation code.
pub use itsanas_coord::claim::MAX_DEVICES_PER_ACCOUNT;
use itsanas_coord::directory::Registration;
pub use itsanas_coord::invitation::Secret;
use itsanas_coord::invitation::{Invitation, SECRET_LEN};
use itsanas_coord::protocol::{EnrolledDevice, Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, DeviceKeys, KdfParams, Keystore, UserId};
// One classifier, in the crate both callers already depend on: a node uses
// it to order what it dials, a coordinator to refuse probing what it must
// not. Two copies would drift the day one of them learned a range.
pub use itsanas_tls::reach::is_private_address;

use crate::contact::Due;
use crate::node::{ESCROW_LABEL, Node};
use crate::{NodeError, NodeError as CliError, Result};

/// Open a connection to the node's configured coordinator.
///
/// The device id is pinned when the configuration names one, so an address
/// resolving to a different machine is refused rather than trusted. A
/// coordinator address is configuration; configuration is not a promise about
/// who lives there.
pub fn dial(node: &Node) -> Result<CoordClient> {
    dial_as(&node.config, &node.device)
}

/// The same, for a caller that has the keys but not an open store.
///
/// Talking to a coordinator needs the config and this device's key. It does
/// **not** need the store, and only one process at a time may hold that -- so
/// tying the two together is what made `doctor` refuse to run on a machine
/// whose daemon was running, which is every machine somebody would ask about.
///
/// # Errors
///
/// If no coordinator is configured, or it cannot be reached, or the device that
/// answers is not the one pinned.
pub fn dial_as(config: &crate::config::Config, device: &DeviceKeys) -> Result<CoordClient> {
    let Some(address) = config.coordinator.as_deref() else {
        return Err(CliError::Usage(
            "no coordinator configured; run `itsanas coordinator <host:port>`".to_owned(),
        ));
    };

    let expect = config
        .coordinator_device
        .as_deref()
        .map(parse_device)
        .transpose()?;

    CoordClient::connect(address, device, expect)
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
    // Before anything is signed: an account with no room is told which of its
    // devices to choose from here, on the machine being enrolled, rather than
    // by whatever a coordinator of unknown version says. The coordinator
    // refuses too (`Directory::claim`), and that is the bound that holds.
    if let Some(live) = live_devices_seen(node) {
        room_for(node.store.device_id(), &live)?;
    }

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

/// Refuse to enrol `this` on an account whose live devices are `live`, when
/// `live` already holds [`MAX_DEVICES_PER_ACCOUNT`] others.
///
/// `this` among them is a re-signing -- `itsanas register` run again, a new
/// pledge -- and takes no slot. An account already above the bound (enrolled
/// before it existed) is refused the same way and told its real count; its
/// devices are not touched.
///
/// # Errors
///
/// When there is no room, naming the devices and the command that frees one.
pub fn room_for(this: DeviceId, live: &[(DeviceId, String)]) -> Result<()> {
    if live.iter().any(|(device, _)| *device == this) || live.len() < MAX_DEVICES_PER_ACCOUNT {
        return Ok(());
    }
    let mut out = format!(
        "this account already has {} live devices and the limit is {MAX_DEVICES_PER_ACCOUNT}:\n",
        live.len()
    );
    for (device, address) in live {
        let address = if address.is_empty() {
            "no address"
        } else {
            address
        };
        // Writing to a `String` cannot fail.
        // The full id: this machine is not enrolled, so a short one could not
        // be resolved against a listing it is refused (see `named` in
        // `itsanas-coord`'s directory).
        let _ = writeln!(out, "  {device}  {address}");
    }
    out.push_str(concat!(
        "This machine was not enrolled and nothing was sent. Those devices keep working.\n",
        "To add this one, withdraw one of them -- from this machine too, with the full id:\n",
        "  itsanas device forget <id>\n",
        "which frees its slot, then run `itsanas register` here again.",
    ));
    Err(CliError::Usage(out))
}

/// The account's live devices as far as this machine can tell before it is
/// enrolled, or `None` when there is nothing to check.
///
/// `Devices` is answered only to a device whose own claim is live, so an
/// answer means this is a re-signing and takes no slot: `None`, whatever the
/// list holds. Not "is this device in the list": the list is truncated at
/// `MAX_PEERS_RETURNED`, silent devices last, so a long-silent device of an
/// account enrolled above the bound can be missing from its own list
/// (found by `itsanas-redteam`). A machine not enrolled yet is refused
/// `Devices` and falls back to `ClaimedPeers` -- every claim owner-signed and
/// checked here, but only the devices heard from within the presence window.
/// So this count can be short and never long (short of a coordinator
/// replaying claims, which is denial of service it can do anyway); the
/// coordinator's own count is the one that binds.
fn live_devices_seen(node: &Node) -> Option<Vec<(DeviceId, String)>> {
    let owner = node.store.owner();
    if let Ok(mut client) = dial(node)
        && let Ok(Response::Devices(_)) = client.ask(&Request::Devices { user: owner })
    {
        return None;
    }
    let mut client = dial(node).ok()?;
    let read = located(&mut client, &node.config, &node.device, owner, false).ok()?;
    Some(
        read.claimed
            .into_iter()
            .map(|row| (row.presence.presence.device, row.presence.presence.address))
            .collect(),
    )
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

/// Tell the coordinator this device is leaving on purpose.
///
/// `Ok(false)` when the coordinator is too old to know the request: it closes
/// the connection, and the departure simply goes unrecorded, which is what a
/// crash looks like and costs nothing today -- nothing reads the record yet.
///
/// # Errors
///
/// If the coordinator cannot be reached, or refuses.
pub fn depart(node: &Node, now: u64) -> Result<bool> {
    let mut client = dial(node)?;
    let notice = itsanas_coord::Departure {
        device: node.store.device_id(),
        at_unix: now,
    }
    .sign(&node.device);
    match client.ask(&Request::Depart(Box::new(notice))) {
        Ok(Response::Done) => Ok(true),
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        // An old coordinator closes the connection on a request it does not
        // know, which looks exactly like an outage. Something it has always
        // answered tells the two apart, as for `check_me`.
        Err(failed) => match devices(node, node.store.owner()) {
            Ok(_) => Ok(false),
            Err(_) => Err(CliError::Usage(format!(
                "the coordinator stopped answering ({failed}); it is not a version question"
            ))),
        },
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
fn located(
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
    let Some(devices) = enrolled(node)? else {
        return Ok(None);
    };
    remember_pledges(node, &devices).map(Some)
}

fn remember_pledges(node: &Node, devices: &[EnrolledDevice]) -> Result<u64> {
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
            at_unix: now,
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

/// Where this machine would publish itself now, found without dialling.
///
/// `announce`, and a listening address that names an interface, are what they
/// are. Otherwise the answer is the local end of a route to the coordinator,
/// which a UDP socket finds by `connect` while sending nothing: a laptop that
/// changed network sees its address change without costing the coordinator a
/// connection. It is not always the address a TCP connection would publish --
/// the two can pick different families of a dual-stack name -- which is why
/// [`Contact`](crate::contact::Contact) compares it only with itself.
///
/// `None` when there is no route: no network, or a name that does not resolve.
#[must_use]
pub fn address_now(config: &crate::config::Config, listen: &str) -> Option<String> {
    if let Some(announce) = config.announce.as_deref() {
        return Some(announce.to_owned());
    }
    match listen.parse::<SocketAddr>() {
        Ok(parsed) if parsed.ip().is_unspecified() => {}
        // An interface, or a name somebody chose: published as it stands.
        _ => return Some(listen.to_owned()),
    }
    let coordinator = config.coordinator.as_deref()?;
    coordinator
        .to_socket_addrs()
        .ok()?
        .find_map(|target| {
            let any: SocketAddr = if target.is_ipv4() {
                (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
            } else {
                (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
            };
            let socket = std::net::UdpSocket::bind(any).ok()?;
            socket.connect(target).ok()?;
            socket.local_addr().ok()
        })
        .map(|local| reachable_address(listen, local))
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

/// Withdraw `wanted` from this account, from this machine, refusing this
/// machine itself.
///
/// The one rule both shells apply before [`forget_device`]: the command line's
/// `itsanas device forget` and the Android app's withdraw button call this, so
/// a phone cannot withdraw itself where a laptop could not.
///
/// # Errors
///
/// When `wanted` is this machine -- withdrawing it from itself would leave a
/// node running and unlisted, dialled by nobody -- and as [`forget_device`].
pub fn withdraw_device(node: &Node, wanted: DeviceId, now: u64) -> Result<()> {
    if wanted == node.store.device_id() {
        return Err(CliError::Usage(
            "that is this device. Withdrawing it from itself would leave it running and \
             unlisted; withdraw it from another device of the account."
                .to_owned(),
        ));
    }
    forget_device(node, wanted, now)
}

/// The coordinator's answer about `presented`, read; `None` when there was
/// none.
fn asked(client: &mut CoordClient, presented: &SignedClaim) -> Option<crate::owners::Verdict> {
    answer_about(
        presented,
        client.ask(&Request::Standing(Box::new(presented.clone()))),
    )
}

/// What one reply to [`Request::Standing`] says about `presented`.
///
/// Only a [`Response::Standing`] is about the device. A refusal is about the
/// question -- a rate limit, a caller the coordinator will not answer -- and
/// reading it as "not enrolled" ended every fresh confirmation at once on a
/// host the coordinator was merely slowing down (redteam), so it is no
/// answer, like a hang-up.
fn answer_about<E>(
    presented: &SignedClaim,
    reply: std::result::Result<Response, E>,
) -> Option<crate::owners::Verdict> {
    match reply {
        Ok(Response::Standing(answer)) => Some(crate::owners::ClaimBook::verdict(
            presented,
            answer.as_deref(),
        )),
        Ok(_) | Err(_) => None,
    }
}

/// What [`crate::owners::ClaimBook::asking`] is handed: a question to this
/// node's coordinator about one claim, on its own connection, owning what it
/// needs so that a process-wide book can keep it.
///
/// No coordinator configured, one not pinned ([`pinned`]), or none
/// answering, is [`Verdict::NoAnswer`](crate::owners::Verdict::NoAnswer).
#[must_use]
pub fn asker(node: &Node) -> crate::owners::Asker {
    let config = node.config.clone();
    let device = DeviceKeys::from_seed(&node.device.seed());
    Box::new(move |presented| {
        if !pinned(&config) {
            return crate::owners::Verdict::NoAnswer;
        }
        dial_as(&config, &device)
            .ok()
            .and_then(|mut client| asked(&mut client, presented))
            .unwrap_or(crate::owners::Verdict::NoAnswer)
    })
}

/// Whether this node's coordinator is pinned by its device id
/// (`coordinator_device`), so that what answers is the coordinator.
///
/// An answer about another account's device is taken only from a pinned one:
/// unpinned, anybody on the path -- the withdrawn device itself, on a café
/// network -- can answer "live" by echoing the claim it presented, which
/// checks out (redteam). The docs have always said to pin it
/// (`itsanas coordinator <host:port> --device <id>`); since 2026-10-05 a host
/// that did not stores for its own account only.
#[must_use]
pub fn pinned(config: &crate::config::Config) -> bool {
    config.coordinator.is_some() && config.coordinator_device.is_some()
}

/// Most devices [`standing`] asks about in one round.
///
/// A device is asked about again after an hour and refused after two, so a
/// round every five minutes must reach every device of other accounts within
/// twelve rounds: 64 a round is 768 an hour. The first version asked 8 a
/// round, which let a host with more than about a hundred of them lapse
/// while its coordinator answered every question (redteam).
pub const STANDING_PER_ROUND: usize = 64;

/// Questions asked over one connection before opening another, well inside
/// the coordinator's `MAX_REQUESTS_PER_CONNECTION`.
const STANDING_PER_CONNECTION: usize = 12;

/// What one round of [`standing`] found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StandingReport {
    /// Devices asked about.
    pub asked: usize,
    /// Answered with a live claim.
    pub live: usize,
    /// Answered with a withdrawal.
    pub withdrawn: usize,
    /// Answered with nothing this host could take as live.
    pub unenrolled: usize,
    /// Not answered.
    pub unanswered: usize,
    /// Devices were due, and nothing was asked because the coordinator is not
    /// pinned ([`pinned`]).
    pub unpinned: bool,
}

/// Ask the coordinator about the devices of other accounts `book` holds
/// claims for and has not confirmed lately (HANDOVER §8 1c (ii)), and note
/// each answer in `book`.
///
/// Up to [`STANDING_PER_ROUND`] a round, a fresh connection every twelve, and
/// only of a pinned coordinator ([`pinned`]). A device the
/// coordinator does not answer for keeps what it had until that lapses --
/// and a device of another account stores nothing on this host until it is
/// confirmed (`owners::UNCONFIRMED`): no answer means no storing (Nicolas,
/// 2026-10-05). With no coordinator configured nothing is asked, so only this
/// node's own account stores here.
///
/// # Errors
///
/// When the coordinator cannot be dialled; the devices due stay due.
pub fn standing(node: &Node, book: &crate::owners::ClaimBook) -> Result<StandingReport> {
    use crate::owners::Verdict;

    let mut report = StandingReport::default();
    if node.config.coordinator.is_none() {
        return Ok(report);
    }
    let due = book.due(node.store.owner(), STANDING_PER_ROUND);
    if due.is_empty() {
        return Ok(report);
    }
    if !pinned(&node.config) {
        report.unpinned = true;
        return Ok(report);
    }
    let mut client = dial(node)?;
    for (index, (device, bytes)) in due.into_iter().enumerate() {
        if index > 0 && index % STANDING_PER_CONNECTION == 0 {
            client = dial(node)?;
        }
        report.asked += 1;
        // Checked when it was taken; a claim that no longer decodes cannot be
        // asked about, and stays unconfirmed.
        let Ok(presented) = postcard::from_bytes::<SignedClaim>(&bytes) else {
            report.unenrolled += 1;
            continue;
        };
        let Some(verdict) = asked(&mut client, &presented) else {
            // The rest of this round's go unasked: the connection is gone.
            report.unanswered += 1;
            break;
        };
        match verdict {
            Verdict::Live => report.live += 1,
            Verdict::Withdrawn => report.withdrawn += 1,
            Verdict::Unenrolled => report.unenrolled += 1,
            Verdict::NoAnswer => report.unanswered += 1,
        }
        book.note(device, verdict);
    }
    Ok(report)
}

/// The devices a refusal for want of a slot names, or `None` when `error` is
/// not that refusal.
///
/// Both refusals -- [`room_for`]'s, on this machine, and the coordinator's
/// `TooManyDevices`, which reaches a client as text -- name every live device
/// by its full id, so a machine that is not enrolled can withdraw one without
/// a listing it would be refused. A shell with no command line (the Android
/// app) reads them from here to put a withdraw button beside each, instead of
/// showing a command it does not have. Only full ids are returned: a short
/// form is never something to withdraw by.
#[must_use]
pub fn cap_named(error: &NodeError) -> Option<Vec<DeviceId>> {
    let text = error.to_string();
    if !text.contains("live devices and the limit is") {
        return None;
    }
    let mut named: Vec<DeviceId> = Vec::new();
    for word in text.split(|c: char| !c.is_ascii_hexdigit()) {
        if word.len() == 2 * itsanas_crypto::ID_LEN
            && let Ok(device) = word.parse::<DeviceId>()
            && !named.contains(&device)
        {
            named.push(device);
        }
    }
    Some(named)
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
    use itsanas_crypto::SecretBytes;

    use super::*;

    fn a_claim(revoked: bool) -> SignedClaim {
        let owner =
            itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([7; 32]));
        NodeClaim {
            owner: owner.user_id(),
            device: DeviceKeys::from_seed(&SecretBytes::new([8; 32])).device_id(),
            pledged_bytes: 1,
            issued_unix: 1,
            revoked,
        }
        .sign(&owner)
    }

    #[test]
    fn red_team_a_refusal_is_no_answer_rather_than_not_enrolled() {
        // Found by the redteam agent: reading a refusal -- a rate limit, a
        // caller the coordinator will not serve -- as "not enrolled" ended
        // every fresh confirmation at once on a host it was only slowing
        // down. Only a `Standing` reply is about the device. Sabotage: map
        // `Refused` to `Unenrolled` again.
        use crate::owners::Verdict;
        let presented = a_claim(false);
        assert_eq!(
            answer_about::<()>(&presented, Ok(Response::Refused("slow down".to_owned()))),
            None,
            "a refusal of the question was taken as an answer about the device"
        );
        assert_eq!(answer_about(&presented, Err(())), None);
        assert_eq!(
            answer_about::<()>(&presented, Ok(Response::Standing(None))),
            Some(Verdict::Unenrolled)
        );
        assert_eq!(
            answer_about::<()>(
                &presented,
                Ok(Response::Standing(Some(Box::new(a_claim(true)))))
            ),
            Some(Verdict::Withdrawn)
        );
    }

    #[test]
    fn red_team_an_unpinned_coordinator_is_not_asked_about_other_accounts() {
        // Unpinned, whoever is on the path answers, and echoing the presented
        // claim checks out as "live". Sabotage: ask without a pin.
        let mut config = crate::config::Config {
            coordinator: Some("127.0.0.1:1".to_owned()),
            ..crate::config::Config::default()
        };
        assert!(
            !pinned(&config),
            "an unpinned coordinator was trusted with withdrawals"
        );
        config.coordinator_device = Some("00".repeat(32));
        assert!(pinned(&config));
    }

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
        }
        .sign(&pi);

        let mut moved = genuine.clone();
        moved.presence.address = "203.0.113.66:9797".to_owned();
        let mut borrowed = Presence {
            device: stranger.device_id(),
            address: "203.0.113.67:9797".to_owned(),
            at_unix: 0,
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
