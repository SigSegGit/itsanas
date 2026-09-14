//! Device certificates: who owns which machine, and who says so.
//!
//! This is the piece `docs/ROADMAP.md` listed as missing from M1 — "the X25519
//! agreement primitive exists, the certificate format does not". Without it a
//! coordinator has no way to tell a member's real device from a stranger
//! claiming to be one, and no way to be told that a laptop was stolen.
//!
//! # Two signatures, because they change at different rates
//!
//! A **claim** says "this device is mine and it pledges this much". It is signed
//! by the user's master key and changes rarely: enrolling a machine, altering a
//! pledge, revoking a stolen laptop.
//!
//! A **presence** says "this device is reachable here, now". It is signed by the
//! *device* key and changes constantly — DHCP, roaming, a reboot.
//!
//! Splitting them keeps the master key out of routine operation. If one message
//! carried both, a laptop that moved between two networks would need the master
//! key every few minutes, and the key that can revoke every device would be the
//! key most often in use.
//!
//! # Revocation
//!
//! Revoking is issuing a claim with `revoked` set, and **a withdrawal is final
//! for that device id**: nothing supersedes it, and a withdrawal supersedes any
//! live claim whatever the two timestamps say. A machine that is used again
//! logs in afresh and gets a new device id.
//!
//! Both halves were once decided by timestamp, and both were wrong:
//!
//! - **A later enrolment un-revoked a device.** This comment used to say that
//!   somebody holding a stolen laptop cannot sign a claim because claims need
//!   the user's key. Every node's keystore holds the master secret, which *is*
//!   the user's key, so a thief who has the passphrase — or the file a daemon
//!   reads it from — ran `itsanas register` and the device was back.
//! - **A withdrawal signed on a slow clock was ignored.** Supersession by the
//!   signer's clock let an enrolment made on a laptop running forty minutes
//!   fast outrank a withdrawal issued ten minutes later from a Pi with the right
//!   time, and the coordinator answered `Done` to both.
//!
//! **What finality costs, which is new.** Anybody holding the master secret —
//! the same thief — can now withdraw the owner's *legitimate* devices for good.
//! Under the timestamp rule the owner answered with a newer claim; now the first
//! withdrawal wins, and every machine it hit has to remove its node directory
//! and log in again, which downloads its share of the account afresh. That was
//! judged the better failure: a withdrawn device that stays withdrawn is a loud,
//! recoverable nuisance; a stolen device that re-enrols is a quiet one. It is a
//! choice, not a free improvement.
//!
//! Also: "whatever the timestamps say" holds within `MAX_CLOCK_SKEW`. A
//! withdrawal signed on a clock more than an hour *ahead* of the coordinator is
//! refused as from the future — out loud, and it can be sent again.
//!
//! What this does **not** protect against, stated because the old wording
//! claimed it did: a thief with the passphrase holds the master secret. They
//! can read everything the account stores and enrol a *new* device under it.
//! The only answer to that is a new account, and nothing rotates an identity
//! today. Withdrawal also does not reach the peer protocol: a withdrawn machine
//! on the same network is still found by discovery and synced with, because a
//! node does not consult the coordinator's claims before serving its own
//! account (`docs/HANDOVER.md` §8.1(c) is where that attribution is planned).

use itsanas_crypto::{DeviceId, DeviceKeys, Signature, UserId, UserKeys, verify};
use serde::{Deserialize, Serialize};

use crate::error::{CoordError, Result};

/// Signature domain for a device claim.
pub const CLAIM_DOMAIN: &str = "itsanas v1 node claim";

/// Signature domain for a presence announcement.
pub const PRESENCE_DOMAIN: &str = "itsanas v1 node presence";

/// How far into the future a timestamp may be before it is refused.
///
/// Live claims supersede each other by timestamp, so a device whose clock is
/// wrong could otherwise issue a claim dated next year that no pledge change
/// could ever replace. A withdrawal no longer depends on it — it wins whatever
/// the dates — but a pledge update still does. One hour is generous for honest
/// clock drift and bounds the damage from a dishonest one to an hour of
/// confusion.
pub const MAX_CLOCK_SKEW: u64 = 3600;

/// A user's statement that a device is theirs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeClaim {
    pub owner: UserId,
    pub device: DeviceId,
    /// How much space this device offers other members.
    pub pledged_bytes: u64,
    /// When the owner issued this, seconds since the Unix epoch.
    ///
    /// Used only to order claims against each other. Nothing that risks data
    /// depends on it.
    pub issued_unix: u64,
    /// Whether this claim withdraws the device.
    pub revoked: bool,
}

/// A [`NodeClaim`] with the owner's signature over it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedClaim {
    pub claim: NodeClaim,
    pub signature: Signature,
}

impl NodeClaim {
    /// Canonical bytes covered by the signature.
    fn payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 32 + 8 + 8 + 1);
        out.extend_from_slice(self.owner.as_bytes());
        out.extend_from_slice(self.device.as_bytes());
        out.extend_from_slice(&self.pledged_bytes.to_le_bytes());
        out.extend_from_slice(&self.issued_unix.to_le_bytes());
        out.push(u8::from(self.revoked));
        out
    }

    /// Sign this claim with the owner's master key.
    #[must_use]
    pub fn sign(self, owner: &UserKeys) -> SignedClaim {
        let signature = owner.sign(CLAIM_DOMAIN, &self.payload());
        SignedClaim {
            claim: self,
            signature,
        }
    }
}

impl SignedClaim {
    /// Check the signature against the owner the claim names.
    ///
    /// `now` is the verifier's clock, used only to refuse absurd future dates.
    pub fn verify(&self, now: u64) -> Result<()> {
        if self.claim.issued_unix > now.saturating_add(MAX_CLOCK_SKEW) {
            return Err(CoordError::FromTheFuture {
                issued: self.claim.issued_unix,
                now,
            });
        }

        verify(
            self.claim.owner.as_bytes(),
            CLAIM_DOMAIN,
            &self.claim.payload(),
            self.signature,
        )
        .map_err(|_| CoordError::BadSignature("node claim"))
    }

    /// Whether this claim replaces `existing`.
    ///
    /// A withdrawal is final: nothing replaces it, and it replaces any live
    /// claim regardless of the dates, because both dates are the signing
    /// machines' opinions of the time. Between two live claims the later wins
    /// and a tie keeps what is held rather than churning. See the module
    /// documentation for the two attacks the old all-timestamp rule allowed.
    #[must_use]
    pub fn supersedes(&self, existing: &Self) -> bool {
        if self.claim.device != existing.claim.device {
            return false;
        }
        match (existing.claim.revoked, self.claim.revoked) {
            (true, _) => false,
            (false, true) => true,
            (false, false) => self.claim.issued_unix > existing.claim.issued_unix,
        }
    }
}

/// A device saying where it can be reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub device: DeviceId,
    /// `host:port`, as the device believes it is reachable.
    pub address: String,
    pub at_unix: u64,
}

/// A [`Presence`] with the device's signature over it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPresence {
    pub presence: Presence,
    pub signature: Signature,
}

/// Longest address string accepted.
///
/// A hostname and port cannot need more, and an unbounded string is an
/// unbounded row in somebody's database.
pub const MAX_ADDRESS_LEN: usize = 255;

impl Presence {
    fn payload(&self) -> Vec<u8> {
        let address = self.address.as_bytes();
        let mut out = Vec::with_capacity(32 + 4 + address.len() + 8);
        out.extend_from_slice(self.device.as_bytes());
        // Length-prefixed, so an address cannot be confused with the timestamp
        // that follows it.
        out.extend_from_slice(
            &u32::try_from(address.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(address);
        out.extend_from_slice(&self.at_unix.to_le_bytes());
        out
    }

    /// Sign with the device's own key.
    #[must_use]
    pub fn sign(self, device: &DeviceKeys) -> SignedPresence {
        let signature = device.sign(PRESENCE_DOMAIN, &self.payload());
        SignedPresence {
            presence: self,
            signature,
        }
    }
}

impl SignedPresence {
    /// Check the signature against the device that claims to have sent it.
    pub fn verify(&self, now: u64) -> Result<()> {
        if self.presence.address.len() > MAX_ADDRESS_LEN {
            return Err(CoordError::Rejected("address is too long"));
        }
        if self.presence.address.is_empty() {
            return Err(CoordError::Rejected("address is empty"));
        }
        if self.presence.at_unix > now.saturating_add(MAX_CLOCK_SKEW) {
            return Err(CoordError::FromTheFuture {
                issued: self.presence.at_unix,
                now,
            });
        }

        verify(
            self.presence.device.as_bytes(),
            PRESENCE_DOMAIN,
            &self.presence.payload(),
            self.signature,
        )
        .map_err(|_| CoordError::BadSignature("presence"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{MasterSecret, SecretBytes};

    const NOW: u64 = 1_700_000_000;

    fn user(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    fn device(byte: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
    }

    fn claim(owner: &UserKeys, dev: &DeviceKeys, issued: u64, revoked: bool) -> SignedClaim {
        NodeClaim {
            owner: owner.user_id(),
            device: dev.device_id(),
            pledged_bytes: 10 * 1024 * 1024 * 1024,
            issued_unix: issued,
            revoked,
        }
        .sign(owner)
    }

    #[test]
    fn an_honest_claim_verifies() {
        let owner = user(1);
        let dev = device(1);
        claim(&owner, &dev, NOW, false).verify(NOW).unwrap();
    }

    #[test]
    fn a_claim_signed_by_someone_else_is_refused() {
        // Otherwise anyone could enrol a device under anyone's account and be
        // paid for it out of that account's entitlement.
        let owner = user(2);
        let impostor = user(3);
        let dev = device(2);

        let mut forged = claim(&impostor, &dev, NOW, false);
        forged.claim.owner = owner.user_id();

        assert!(matches!(
            forged.verify(NOW),
            Err(CoordError::BadSignature(_))
        ));
    }

    #[test]
    fn tampering_with_any_field_invalidates_the_claim() {
        let owner = user(4);
        let dev = device(4);
        let original = claim(&owner, &dev, NOW, false);

        let mut richer = original.clone();
        richer.claim.pledged_bytes *= 1000;
        assert!(richer.verify(NOW).is_err(), "the pledge is not signed");

        let mut other_device = original.clone();
        other_device.claim.device = device(99).device_id();
        assert!(other_device.verify(NOW).is_err());

        let mut unrevoked = claim(&owner, &dev, NOW, true);
        unrevoked.claim.revoked = false;
        assert!(
            unrevoked.verify(NOW).is_err(),
            "a revocation could be stripped, so a stolen laptop could \
             un-revoke itself"
        );

        let mut redated = original.clone();
        redated.claim.issued_unix += 1;
        assert!(redated.verify(NOW).is_err());
    }

    #[test]
    fn a_claim_dated_far_in_the_future_is_refused() {
        // Supersession is by timestamp. A claim dated next year could never be
        // replaced — not even by its owner trying to revoke it.
        let owner = user(5);
        let dev = device(5);

        let future = claim(&owner, &dev, NOW + MAX_CLOCK_SKEW + 60, false);
        assert!(matches!(
            future.verify(NOW),
            Err(CoordError::FromTheFuture { .. })
        ));

        // Ordinary drift is tolerated.
        claim(&owner, &dev, NOW + 60, false).verify(NOW).unwrap();
    }

    #[test]
    fn a_later_claim_supersedes_an_earlier_one() {
        let owner = user(6);
        let dev = device(6);

        let old = claim(&owner, &dev, NOW, false);
        let new = claim(&owner, &dev, NOW + 10, false);

        assert!(new.supersedes(&old));
        assert!(!old.supersedes(&new), "an older claim replaced a newer one");
    }

    #[test]
    fn a_revocation_cannot_be_undone_by_replaying_an_older_claim() {
        // The stolen-laptop case. Whoever holds the device must not be able to
        // resurrect it by re-sending a claim they captured earlier.
        let owner = user(7);
        let dev = device(7);

        let enrolled = claim(&owner, &dev, NOW, false);
        let revoked = claim(&owner, &dev, NOW + 100, true);

        assert!(revoked.supersedes(&enrolled));
        assert!(
            !enrolled.supersedes(&revoked),
            "replaying an old enrolment un-revoked a withdrawn device"
        );
    }

    #[test]
    fn a_revocation_wins_a_tie() {
        let owner = user(8);
        let dev = device(8);

        let alive = claim(&owner, &dev, NOW, false);
        let dead = claim(&owner, &dev, NOW, true);

        assert!(dead.supersedes(&alive));
        assert!(!alive.supersedes(&dead));
    }

    #[test]
    fn red_team_a_withdrawal_signed_on_a_slow_clock_still_withdraws() {
        // THE ATTACK, or the accident: the laptop enrolled itself with its
        // clock forty minutes fast -- inside MAX_CLOCK_SKEW, so accepted -- and
        // was stolen. The owner withdraws it ten minutes later from the Pi,
        // whose clock is right. By timestamp the enrolment is newer, the
        // coordinator keeps it, answers `Done`, and `device forget` prints
        // "withdrew" over a device that is still enrolled.
        let owner = user(16);
        let dev = device(16);

        let enrolled_on_a_fast_clock = claim(&owner, &dev, NOW + 40 * 60, false);
        let withdrawn_later_on_a_right_clock = claim(&owner, &dev, NOW + 10 * 60, true);

        assert!(
            withdrawn_later_on_a_right_clock.supersedes(&enrolled_on_a_fast_clock),
            "a withdrawal lost to an enrolment dated by a faster clock; the \
             device stays enrolled while its owner is told it was withdrawn"
        );
    }

    #[test]
    fn a_later_enrolment_does_not_supersede_a_withdrawal() {
        // Every node's keystore holds the master secret, so whoever has a
        // stolen laptop and its passphrase can sign a claim dated after the
        // withdrawal. Supersession must not let that claim through, or
        // `itsanas register` on the stolen machine undoes `device forget`.
        let owner = user(17);
        let dev = device(17);

        let withdrawn = claim(&owner, &dev, NOW, true);
        let re_enrolled = claim(&owner, &dev, NOW + 3000, false);

        assert!(
            !re_enrolled.supersedes(&withdrawn),
            "a later enrolment replaced a withdrawal: a stolen machine with \
             its passphrase re-enrols itself"
        );
    }

    #[test]
    fn a_claim_never_supersedes_one_for_a_different_device() {
        let owner = user(9);
        let first = claim(&owner, &device(9), NOW + 100, false);
        let second = claim(&owner, &device(10), NOW, false);

        assert!(!first.supersedes(&second));
    }

    #[test]
    fn presence_is_signed_by_the_device_not_the_owner() {
        // The point of the split: a laptop moving between networks must not
        // need the key that can revoke every device the user owns.
        let dev = device(11);
        let announced = Presence {
            device: dev.device_id(),
            address: "192.168.1.20:9797".to_owned(),
            at_unix: NOW,
        }
        .sign(&dev);

        announced.verify(NOW).unwrap();

        let mut moved = announced.clone();
        moved.presence.address = "10.0.0.5:9797".to_owned();
        assert!(
            moved.verify(NOW).is_err(),
            "the address is not covered by the signature, so anyone could \
             redirect a peer's traffic"
        );
    }

    #[test]
    fn one_device_cannot_announce_an_address_for_another() {
        let honest = device(12);
        let attacker = device(13);

        let mut forged = Presence {
            device: honest.device_id(),
            address: "attacker.example:9797".to_owned(),
            at_unix: NOW,
        }
        .sign(&attacker);
        forged.presence.device = honest.device_id();

        assert!(forged.verify(NOW).is_err());
    }

    #[test]
    fn an_absurd_address_is_refused_before_the_signature_is_checked() {
        let dev = device(14);

        for address in [String::new(), "a".repeat(MAX_ADDRESS_LEN + 1)] {
            let announced = Presence {
                device: dev.device_id(),
                address,
                at_unix: NOW,
            }
            .sign(&dev);

            assert!(matches!(
                announced.verify(NOW),
                Err(CoordError::Rejected(_))
            ));
        }
    }

    #[test]
    fn claims_and_presences_round_trip_through_the_wire() {
        let owner = user(15);
        let dev = device(15);

        let signed = claim(&owner, &dev, NOW, false);
        let frame = itsanas_wire::encode(&signed).unwrap();
        assert_eq!(itsanas_wire::decode::<SignedClaim>(&frame).unwrap(), signed);

        let announced = Presence {
            device: dev.device_id(),
            address: "pi.local:9797".to_owned(),
            at_unix: NOW,
        }
        .sign(&dev);
        let frame = itsanas_wire::encode(&announced).unwrap();
        assert_eq!(
            itsanas_wire::decode::<SignedPresence>(&frame).unwrap(),
            announced
        );
    }
}
