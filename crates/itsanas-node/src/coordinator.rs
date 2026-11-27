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

// Re-exported for a shell that does not depend on the coordinator crate (the
// Android app) and has to say the limit and carry an invitation code.
pub use itsanas_coord::claim::MAX_DEVICES_PER_ACCOUNT;
// The same reason: the setup page and the phone say what a name may be before
// asking whether it is free, since a name refused later reads as free now.
pub use itsanas_coord::directory::{MAX_USERNAME_LEN, validate_username};
pub use itsanas_coord::invitation::Secret;
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, DeviceKeys};
// One classifier, in the crate both callers already depend on: a node uses
// it to order what it dials, a coordinator to refuse probing what it must
// not. Two copies would drift the day one of them learned a range.
pub use itsanas_tls::reach::is_private_address;

use crate::node::Node;
use crate::{NodeError as CliError, Result};

mod account;
mod claims;
mod escrow;
mod ipv6;
mod lookup;
mod publishing;
mod round;

pub use account::{
    accepted_deletions, account_pledge, cap_named, decode_secret, delete_account, deleted_among,
    depart, encode_secret, ensure_enrolled, invitation_needed, invite, name_owner, name_taken,
    register_with, room_for, username_refusal,
};
pub use claims::{
    STANDING_PER_ROUND, StandingReport, asker, forget_device, pinned, standing, withdraw_device,
    withdraw_old_device, withdraw_self,
};
pub use escrow::{NOTHING_OPENS, fetch_escrow, recovery_container, set_escrow};
pub use ipv6::{prefer_global_ipv6, with_global_ipv6};
pub use lookup::{
    Located, OtherPledges, devices, devices_as, enrolled, enrolled_as, find_member, hosts,
    note_heads, other_pledges, peers, reachable_first, reachable_first_addresses,
    refresh_others_pledged, verified, verified_claimed, verified_presences,
};
pub use publishing::{
    ANNOUNCE_LISTS, ANNOUNCE_RELAY, address_now, announce, direct_address, found_address,
    on_a_real_adapter, published_address, reachable_address, set_found_address, set_parked_on,
};
pub use round::{Contacted, Reachability, check_me, check_me_as, contact};

/// The official coordinator: what a desktop setup page proposes when the
/// person names none, and the only one the Android app joins. One
/// name for the whole network, repointable by DNS (HANDOVER §8 0w (6b), 4).
/// Not a secret -- the code is public and the address shows in `netstat` --
/// and not a single point of failure: members that know each other keep
/// syncing without it.
pub const DEFAULT_COORDINATOR: &str = "itsanas.ngas.fr:9898";

/// The device id of the coordinator at [`DEFAULT_COORDINATOR`], printed by
/// `install/coordinator.sh` on the Freebox VM on 2026-10-07. Pinned, so a
/// machine answering at that name that is not this coordinator is refused
/// rather than trusted (DNS, a hijacked forward, a hostile network).
pub const DEFAULT_COORDINATOR_DEVICE: &str =
    "2cfb515fc90749d7248f4af404ecb34b5417e0c1339565c8036a9dde70cf72ce";

/// The device to pin for `address`, when it is the built-in coordinator.
#[must_use]
pub fn pinned_for(address: &str) -> Option<&'static str> {
    (address == DEFAULT_COORDINATOR).then_some(DEFAULT_COORDINATOR_DEVICE)
}

/// The file that says which coordinator this node last registered with: the
/// only way `setup` can tell "registered" from "configured" without asking
/// the coordinator, and asking again is what a re-run must not need.
pub const REGISTERED: &str = "registered";

/// The coordinator this node last registered with, if any.
#[must_use]
pub fn registered_with(home: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(home.join(REGISTERED))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Write the [`REGISTERED`] marker for `node`'s coordinator. Best effort:
/// without it the next start registers again, which the coordinator takes as a
/// refresh. The coordinator's address, so that a node pointed at another one
/// enrols there.
pub fn mark_registered(node: &Node) {
    if let Some(address) = &node.config.coordinator {
        let _ = std::fs::write(node.home.join(REGISTERED), address);
    }
}

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
}
