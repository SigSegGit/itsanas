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

use itsanas_coord::claim::{NodeClaim, Presence};
use itsanas_coord::directory::Registration;
use itsanas_coord::protocol::{Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, KdfParams, Keystore, UserId};

use crate::error::{CliError, Result};
use crate::node::{ESCROW_LABEL, Node};

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

    CoordClient::connect(address, &node.device, node.store.owner(), expect)
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
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair)
            .map_err(|_| CliError::Usage("a device id must be hexadecimal".to_owned()))?;
        bytes[index] = u8::from_str_radix(text, 16)
            .map_err(|_| CliError::Usage("a device id must be hexadecimal".to_owned()))?;
    }
    Ok(DeviceId::from_bytes(bytes))
}

/// Register the account name and enrol this device.
///
/// Both are signed by keys the coordinator does not hold, so the worst it can
/// do is refuse — which is denial of service and already in the threat model.
pub fn register(node: &Node, now: u64) -> Result<()> {
    let mut client = dial(node)?;

    let registration = Registration {
        username: node.config.username.clone(),
        user: node.user.public(),
        issued_unix: now,
    }
    .sign(&node.user);

    match client.ask(&Request::Register(Box::new(registration)))? {
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

/// Publish where this device can be reached.
///
/// Signed by the device, not by the owner: a laptop moving between networks
/// announces constantly and must never need the key that can revoke everything.
pub fn announce(node: &Node, address: &str, now: u64) -> Result<()> {
    let mut client = dial(node)?;
    let presence = Presence {
        device: node.store.device_id(),
        address: address.to_owned(),
        at_unix: now,
    }
    .sign(&node.device);

    match client.ask(&Request::Announce(Box::new(presence)))? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Where the other devices of `user` say they are.
pub fn peers(node: &Node, user: UserId) -> Result<Vec<(DeviceId, String)>> {
    let mut client = dial(node)?;
    match client.ask(&Request::Peers { user })? {
        Response::Peers(list) => Ok(list
            .into_iter()
            .filter(|presence| presence.device != node.store.device_id())
            .map(|presence| (presence.device, presence.address))
            .collect()),
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
    let mut client = CoordClient::connect(address, &device, UserId::from_bytes([0; 32]), expect)
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
    fn the_escrow_label_differs_from_the_local_keystore_label() {
        // The two containers hold the same secrets and live in different places
        // under different threat models. Sharing a label would mean a copy of
        // the coordinator's blob could be dropped in as the local keystore, and
        // the domain separation that makes each one specific would be gone.
        assert_ne!(ESCROW_LABEL, crate::node::KEYSTORE_LABEL);
    }
}
