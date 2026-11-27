//! Joining and leaving: who holds a name, registering and enrolling this
//! device, invitations, the device cap, departing and deleting the account.

use std::fmt::Write as _;

use itsanas_coord::claim::NodeClaim;
use itsanas_coord::directory::Registration;
use itsanas_coord::invitation::{Invitation, SECRET_LEN};
use itsanas_coord::protocol::{Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, DeviceKeys, UserId};

use super::lookup::located;
use super::{
    MAX_DEVICES_PER_ACCOUNT, Secret, devices, dial, mark_registered, parse_device, pinned_for,
    registered_with, validate_username, verified_claimed,
};
use crate::node::Node;
use crate::{NodeError, NodeError as CliError, Result};

/// Who holds `username` at the coordinator at `address`, asked before anything
/// of this machine exists. A lookup is unauthenticated, so a throwaway device
/// key serves: this machine has no key of its own yet, which is the point of
/// asking first.
///
/// # Errors
///
/// If the coordinator cannot be reached, or answers with something else.
pub fn name_owner(address: &str, username: &str) -> Result<Option<UserId>> {
    let keys = DeviceKeys::generate().map_err(|error| CliError::Usage(error.to_string()))?;
    let expect = pinned_for(address).map(parse_device).transpose()?;
    let mut client = CoordClient::connect(address, &keys, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))?;
    match client.ask(&Request::Lookup {
        username: username.to_owned(),
    }) {
        Ok(Response::Account(account)) => Ok(Some(account.user.id)),
        Ok(Response::Missing) => Ok(None),
        Ok(other) => Err(CliError::Usage(format!("{address} answered {other:?}"))),
        Err(error) => Err(CliError::Usage(format!("{address}: {error}"))),
    }
}

/// The largest pledge among the machines of the account that holds `username`,
/// as their signed claims say, asked before anything of this machine exists: what
/// a person joining that account is told their other machines offer. `None` when
/// no account has the name or none of its machines has announced itself -- a
/// figure nobody could check is not offered.
///
/// # Errors
///
/// As [`name_owner`].
pub fn account_pledge(address: &str, username: &str) -> Result<Option<u64>> {
    let Some(user) = name_owner(address, username)? else {
        return Ok(None);
    };
    let keys = DeviceKeys::generate().map_err(|error| CliError::Usage(error.to_string()))?;
    let expect = pinned_for(address).map(parse_device).transpose()?;
    let mut client = CoordClient::connect(address, &keys, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))?;
    match client.ask(&Request::ClaimedPeers { user }) {
        Ok(Response::ClaimedPeers(list)) => {
            let (claimed, _) = verified_claimed(list, user);
            Ok(claimed
                .iter()
                .map(|row| row.claim.claim.pledged_bytes)
                .max())
        }
        Ok(Response::Refused(_)) => Ok(None),
        Ok(other) => Err(CliError::Usage(format!("{address} answered {other:?}"))),
        Err(error) => Err(CliError::Usage(format!("{address}: {error}"))),
    }
}

/// Why no coordinator would ever register `username`, in a sentence for the
/// person typing it; `None` when one could. Asked first: a name the coordinator
/// would refuse reads as free on a lookup, so a person is told so and goes on,
/// and only learns otherwise at registration.
#[must_use]
pub fn username_refusal(username: &str) -> Option<String> {
    validate_username(username)
        .err()
        .map(|refused| match refused {
            itsanas_coord::CoordError::Rejected(why) => why.to_owned(),
            other => other.to_string(),
        })
}

/// Whether the coordinator at `address` already gives `username` to a key.
///
/// # Errors
///
/// As [`name_owner`].
pub fn name_taken(address: &str, username: &str) -> Result<bool> {
    Ok(name_owner(address, username)?.is_some())
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
        pledged_bytes: node.config.offered_bytes(),
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

/// Enrol this device unless its [`REGISTERED`] marker says it already is, with
/// no invitation: what a shell with no step for it (the phone, at each round)
/// calls so a person never has to find a Join button. `Ok(true)` when it
/// enrolled now, `Ok(false)` when there was nothing to do. The marker is how a
/// later call stays off the network; it names the coordinator, so a node
/// pointed at another one enrols there.
///
/// # Errors
///
/// As [`register_with`]: the caller decides whether that matters, and on a
/// phone at each round it does not, because the next round asks again.
///
/// [`REGISTERED`]: super::REGISTERED
pub fn ensure_enrolled(node: &Node, now: u64) -> Result<bool> {
    let Some(address) = node.config.coordinator.as_deref() else {
        return Ok(false);
    };
    if registered_with(&node.home).as_deref() == Some(address) {
        return Ok(false);
    }
    register_with(node, None, now)?;
    mark_registered(node);
    Ok(true)
}

/// Whether `error` is a coordinator refusing a newcomer for want of a good
/// invitation: no code, or one it will not take. Read from the sentence, as
/// [`cap_named`] reads its own, because it reaches a client as text; a shell
/// with a code field then shows the field instead of a failure.
#[must_use]
pub fn invitation_needed(error: &NodeError) -> bool {
    let text = error.to_string();
    text.contains("by invitation only") || text.contains("that invitation cannot be used")
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

/// Ask the coordinator to delete this account (HANDOVER §8 0w (8)), signed
/// with the account's key. The username then stays retired for a year and
/// this key never registers again.
///
/// # Errors
///
/// No coordinator, a refusal (said as the coordinator worded it), an outage.
pub fn delete_account(node: &Node, now: u64) -> Result<()> {
    let mut client = dial(node)?;
    let request = itsanas_coord::claim::Deletion {
        user: node.store.owner(),
        username: node.config.username.clone(),
        at_unix: now,
    }
    .sign(&node.user);
    match client.ask(&Request::DeleteAccount(Box::new(request))) {
        Ok(Response::Done) => Ok(()),
        Ok(Response::Refused(why)) => Err(CliError::Usage(why)),
        Ok(other) => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
        Err(failed) => Err(CliError::Usage(format!(
            "the coordinator did not answer ({failed}); nothing was deleted there"
        ))),
    }
}

/// Which of `users` the coordinator says were deleted, asked in batches of
/// [`itsanas_coord::protocol::MAX_DELETED_ASKED`]. An old coordinator that
/// does not know the question answers nothing, which deletes nothing.
///
/// # Errors
///
/// No coordinator, an outage.
pub fn deleted_among(node: &Node, users: &[UserId]) -> Result<Vec<UserId>> {
    let mut client = dial(node)?;
    let mut out = Vec::new();
    for batch in users.chunks(itsanas_coord::protocol::MAX_DELETED_ASKED) {
        match client.ask(&Request::Deleted {
            users: batch.to_vec(),
        }) {
            // Only what was asked, and only what the account itself signed: a
            // coordinator, pinned or not, must not be able to make a host drop
            // an account that never asked to be deleted.
            Ok(Response::Deleted(found)) => out.extend(accepted_deletions(batch, found)),
            Ok(_) | Err(_) => break,
        }
    }
    Ok(out)
}

/// The accounts a host may free: asked about, and signed by the account.
#[must_use]
pub fn accepted_deletions(
    asked: &[UserId],
    told: Vec<itsanas_coord::claim::SignedDeletion>,
) -> Vec<UserId> {
    told.into_iter()
        .filter(|told| told.verify_origin().is_ok())
        .map(|told| told.deletion.user)
        .filter(|user| asked.contains(user))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_a_coordinator_cannot_make_a_host_drop_an_account_that_never_asked() {
        use itsanas_coord::claim::Deletion;
        use itsanas_crypto::{MasterSecret, UserKeys};
        let victim = UserKeys::derive(&MasterSecret::from_bytes([21; 32]));
        let liar = UserKeys::derive(&MasterSecret::from_bytes([22; 32]));
        let leaving = UserKeys::derive(&MasterSecret::from_bytes([23; 32]));
        let mut forged = Deletion {
            user: liar.user_id(),
            username: "victim".to_owned(),
            at_unix: 0,
        }
        .sign(&liar);
        forged.deletion.user = victim.user_id();
        let genuine = Deletion {
            user: leaving.user_id(),
            username: "leaving".to_owned(),
            at_unix: 0,
        }
        .sign(&leaving);
        let unasked = Deletion {
            user: liar.user_id(),
            username: "liar".to_owned(),
            at_unix: 0,
        }
        .sign(&liar);
        assert_eq!(
            accepted_deletions(
                &[victim.user_id(), leaving.user_id()],
                vec![forged, genuine, unasked],
            ),
            vec![leaving.user_id()],
            "a host would free an account on its coordinator's word alone, or one it never asked \
             about: a hostile coordinator could erase anybody's replicas"
        );
    }
}
