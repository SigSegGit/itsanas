//! The directory's unit tests, one module per concern, and the helpers they
//! share.

use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes};

use super::*;
use crate::{
    claim::{MAX_DEVICES_PER_ACCOUNT, NodeClaim, Presence},
    invitation::{self, SECRET_LEN, Secret},
};

const NOW: u64 = 1_800_000_000;

mod accounts;
mod availability;
mod claims;
mod escrow;
mod expiry;
mod invitations;
mod owner_index;
mod presence;

fn open_registration(who: &UserKeys, name: &str, now: u64) -> SignedRegistration {
    Registration {
        username: name.to_owned(),
        user: who.public(),
        issued_unix: now,
    }
    .sign(who)
}

fn invite(inviter: &UserKeys, secret: &Secret, uses: u32, now: u64) -> SignedInvitation {
    crate::invitation::Invitation {
        inviter: inviter.user_id(),
        code: invitation::code_id(secret),
        issued_unix: now,
        expires_unix: now + crate::invitation::DEFAULT_VALIDITY,
        uses,
    }
    .sign(inviter)
}

fn found(directory: &Directory, who: &UserKeys, name: &str, now: u64) -> Result<Account> {
    let signed = Registration {
        username: name.to_owned(),
        user: who.public(),
        issued_unix: now,
    }
    .sign(who);
    directory.register_admitted(&signed, None, Admission::Founding, now)
}

fn join(
    directory: &Directory,
    who: &UserKeys,
    name: &str,
    secret: Option<&Secret>,
    now: u64,
) -> Result<Account> {
    let signed = Registration {
        username: name.to_owned(),
        user: who.public(),
        issued_unix: now,
    }
    .sign(who);
    directory.register_admitted(&signed, secret, Admission::ByInvitation, now)
}

fn directory() -> (tempfile::TempDir, Directory) {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("coord.redb")).expect("open");
    (dir, directory)
}

fn user(byte: u8) -> UserKeys {
    UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
}

fn device(byte: u8) -> DeviceKeys {
    DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
}

fn register(directory: &Directory, name: &str, owner: &UserKeys) -> Account {
    let signed = Registration {
        username: name.to_owned(),
        user: owner.public(),
        issued_unix: NOW,
    }
    .sign(owner);
    directory.register(&signed, NOW).expect("register")
}

fn enrol(directory: &Directory, owner: &UserKeys, dev: &DeviceKeys, pledged: u64) {
    let claim = NodeClaim {
        owner: owner.user_id(),
        device: dev.device_id(),
        pledged_bytes: pledged,
        issued_unix: NOW,
        revoked: false,
    }
    .sign(owner);
    directory.claim(&claim, NOW).expect("claim");
}

/// A claim of `owner` on device `seed`, live or withdrawn, at `at`.
fn signed(owner: &UserKeys, seed: u8, at: u64, revoked: bool) -> SignedClaim {
    NodeClaim {
        owner: owner.user_id(),
        device: device(seed).device_id(),
        pledged_bytes: if revoked { 0 } else { 1024 },
        issued_unix: at,
        revoked,
    }
    .sign(owner)
}

fn deletion(owner: &UserKeys, name: &str, at: u64) -> crate::claim::SignedDeletion {
    crate::claim::Deletion {
        user: owner.user_id(),
        username: name.to_owned(),
        at_unix: at,
    }
    .sign(owner)
}

fn try_register(directory: &Directory, name: &str, owner: &UserKeys, now: u64) -> Result<Account> {
    let signed = Registration {
        username: name.to_owned(),
        user: owner.public(),
        issued_unix: now,
    }
    .sign(owner);
    directory.register(&signed, now)
}

const KEPT: u64 = crate::service::PRESENCE_KEPT_FOR;

/// `dev` announcing itself, saying the time is `said`, heard by a
/// coordinator whose clock reads `heard`.
fn announce_at(directory: &Directory, dev: &DeviceKeys, said: u64, heard: u64) {
    directory
        .announce(
            &Presence {
                device: dev.device_id(),
                address: "203.0.113.7:9797".to_owned(),
                at_unix: said,
                head: 0,
            }
            .sign(dev),
            heard,
        )
        .expect("announce");
}
