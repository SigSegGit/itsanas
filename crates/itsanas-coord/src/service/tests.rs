//! The service's unit tests, one module per concern, and the helpers they
//! share.

use super::*;
use crate::claim::{NodeClaim, Presence};

mod peers;
mod presence;
mod probe;
mod standing;
mod strangers;

/// A fixed wall clock, so nothing in these tests depends on today's date.
const NOW: u64 = 1_700_000_000;

fn directory() -> Directory {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("directory.redb");
    // The directory owns its file; the handle keeps the directory alive for
    // the length of the test by leaking it, which is what a test process
    // exiting does anyway.
    std::mem::forget(dir);
    Directory::open(path).expect("directory")
}

fn user(seed: u8) -> itsanas_crypto::UserKeys {
    itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([seed; 32]))
}

fn device(seed: u8) -> itsanas_crypto::DeviceKeys {
    itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]))
}

fn enrol(
    directory: &Directory,
    owner: &itsanas_crypto::UserKeys,
    device: &itsanas_crypto::DeviceKeys,
) {
    directory
        .register(
            &crate::directory::Registration {
                username: format!("member{}", device.device_id().short()),
                user: owner.public(),
                issued_unix: NOW,
            }
            .sign(owner),
            NOW,
        )
        .expect("register");
    directory
        .claim(
            &NodeClaim {
                owner: owner.user_id(),
                device: device.device_id(),
                pledged_bytes: 1 << 30,
                issued_unix: NOW,
                revoked: false,
            }
            .sign(owner),
            NOW,
        )
        .expect("claim");
}

fn announce_address(
    directory: &Directory,
    _owner: &itsanas_crypto::UserKeys,
    device: &itsanas_crypto::DeviceKeys,
    address: &str,
) {
    directory
        .announce(
            &Presence {
                device: device.device_id(),
                address: address.to_owned(),
                at_unix: NOW,
                head: 0,
            }
            .sign(device),
            NOW,
        )
        .expect("announce");
}

fn ask(service: &CoordService<'_>, request: &Request, caller: DeviceId) -> Response {
    ask_at(service, request, caller, NOW)
}

fn ask_at(service: &CoordService<'_>, request: &Request, caller: DeviceId, now: u64) -> Response {
    let mut limiter = EscrowLimiter::new();
    service
        .handle(request, caller, now, &mut limiter, Instant::now())
        .unwrap()
}

fn claim_of(
    owner: &itsanas_crypto::UserKeys,
    device: &itsanas_crypto::DeviceKeys,
    revoked: bool,
    at: u64,
) -> crate::claim::SignedClaim {
    NodeClaim {
        owner: owner.user_id(),
        device: device.device_id(),
        pledged_bytes: 1 << 30,
        issued_unix: at,
        revoked,
    }
    .sign(owner)
}
