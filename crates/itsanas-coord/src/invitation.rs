//! Who is allowed to join, and who vouched for them.
//!
//! # The gap this fills
//!
//! Everything else the coordinator does assumes the members already exist.
//! `Register` claims a username for a keypair, and a keypair costs nothing, so
//! until now anyone who could reach the coordinator was a member. Every defence
//! elsewhere in this project — audits, the reliability pause, the probation
//! ladder, the keyed audit order — is aimed at a *hostile host*, and a hostile
//! host is somebody who joined. The front door had not been built.
//!
//! `ECONOMICS.md` §6 already named the answer and left it as a sentence: "when
//! it becomes a real problem, the answer is invitation, not a bigger number."
//! This is that sentence.
//!
//! # The coordinator must not be the authority
//!
//! It is a notice board (`ECONOMICS.md` §7) and it is not trusted. So an
//! invitation is signed by an **existing member**, and the coordinator only
//! checks arithmetic it cannot fake: that the signature verifies against a user
//! it already knows, that the code matches, that it has not expired, and that
//! its uses are not spent.
//!
//! The coordinator can still **refuse** a valid invitation. That is denial of
//! service, it is already in the threat model, and it is the one thing a notice
//! board is inherently able to do.
//!
//! # Why a hashed code rather than naming the invitee
//!
//! The obvious design is "Alice signs Bob's user id". It cannot work: Bob has no
//! user id until he has joined, and asking a newcomer to generate keys, send
//! them to Alice, wait, and then register is four steps where one will do.
//!
//! So Alice draws a secret, publishes `BLAKE3(secret)` inside something she has
//! signed, and hands the secret over by whatever channel she was going to use
//! anyway. Knowing the secret is the proof of invitation. The coordinator learns
//! only the hash until somebody redeems it, so its database does not hand an
//! attacker a list of live codes.
//!
//! # What it buys, stated exactly
//!
//! Minting an identity now costs an existing member's endorsement, and the
//! endorsement is **attributable**: every invitation names its inviter, and the
//! directory keeps that link after redemption. A member who invites a hundred
//! accounts that all fail their audits is visible as the source.
//!
//! What it does not buy: it is not proof of good behaviour, and a member who
//! wants to farm identities can invite themselves as many times as their own
//! invitation budget allows. That budget is the lever, and it is deliberately
//! not set here — see `Directory::invitations_left`.

use itsanas_crypto::{Signature, UserId, UserKeys, verify};
use serde::{Deserialize, Serialize};

use crate::error::{CoordError, Result};

/// Signature domain for an invitation.
pub const INVITATION_DOMAIN: &str = "itsanas v1 membership invitation";

/// Bytes in an invitation secret.
///
/// Thirty-two, because it is the thing that stands between a stranger and
/// membership and it is guessed offline: the coordinator will answer "is this
/// code valid?" as fast as anyone can ask. Sixteen would be enough against that
/// and thirty-two costs nothing to carry.
pub const SECRET_LEN: usize = 32;

/// How long an invitation is good for by default.
///
/// A week. Long enough to reach somebody who checks their messages on Sundays,
/// short enough that a code posted in a group chat two years ago is not still a
/// way in. The inviter can choose otherwise; this is what the CLI uses when
/// nobody said.
pub const DEFAULT_VALIDITY: u64 = 7 * 24 * 60 * 60;

/// A secret handed to somebody out of band.
///
/// Its hash travels in the signed invitation; the secret itself never reaches
/// the coordinator until it is redeemed, so a stolen directory is a list of
/// hashes rather than a list of ways in.
pub type Secret = [u8; SECRET_LEN];

/// The identifier a coordinator files an invitation under.
pub type CodeId = [u8; 32];

/// What the code identifying `secret` is.
#[must_use]
pub fn code_id(secret: &Secret) -> CodeId {
    itsanas_crypto::message_digest("itsanas v1 invitation code id", secret)
}

/// An existing member's statement that whoever holds a secret may join.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    /// Who is vouching. Kept after redemption, so a member who invites a
    /// hundred accounts that all misbehave is visible as the source.
    pub inviter: UserId,
    /// `BLAKE3` of the secret. The secret itself is not here.
    pub code: CodeId,
    /// When the inviter issued it, seconds since the Unix epoch.
    pub issued_unix: u64,
    /// When it stops working.
    pub expires_unix: u64,
    /// How many accounts it may admit.
    ///
    /// One, normally. More is for a household enrolling several machines from
    /// one code, and every use is recorded against the same inviter.
    pub uses: u32,
}

/// An [`Invitation`] signed by the member issuing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedInvitation {
    pub invitation: Invitation,
    pub signature: Signature,
}

impl Invitation {
    fn payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 32 + 8 + 8 + 4);
        out.extend_from_slice(self.inviter.as_bytes());
        out.extend_from_slice(&self.code);
        out.extend_from_slice(&self.issued_unix.to_le_bytes());
        out.extend_from_slice(&self.expires_unix.to_le_bytes());
        out.extend_from_slice(&self.uses.to_le_bytes());
        out
    }

    /// Sign with the master key of the member vouching.
    #[must_use]
    pub fn sign(self, inviter: &UserKeys) -> SignedInvitation {
        let signature = inviter.sign(INVITATION_DOMAIN, &self.payload());
        SignedInvitation {
            invitation: self,
            signature,
        }
    }
}

impl SignedInvitation {
    /// Check the signature and that the invitation is internally sensible.
    ///
    /// Says nothing about whether the inviter is a member, whether the code has
    /// been spent, or what time it is: those are the directory's questions, and
    /// keeping them apart is what lets this be tested without one.
    pub fn verify(&self) -> Result<()> {
        if self.invitation.uses == 0 {
            return Err(CoordError::Rejected(
                "an invitation good for no uses is not an invitation",
            ));
        }
        if self.invitation.expires_unix <= self.invitation.issued_unix {
            return Err(CoordError::Rejected(
                "an invitation that expires before it is issued",
            ));
        }

        verify(
            self.invitation.inviter.as_bytes(),
            INVITATION_DOMAIN,
            &self.invitation.payload(),
            self.signature,
        )
        .map_err(|_| CoordError::BadSignature("invitation"))
    }

    /// Whether `secret` is the one this invitation was issued for.
    ///
    /// Constant time is not needed and would be theatre: the comparison is
    /// between two hashes, and an attacker who can grind the hash has already
    /// won without measuring anything.
    #[must_use]
    pub fn opens_with(&self, secret: &Secret) -> bool {
        code_id(secret) == self.invitation.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::MasterSecret;

    fn member(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    fn secret(byte: u8) -> Secret {
        [byte; SECRET_LEN]
    }

    fn invitation(inviter: &UserKeys, code: &Secret) -> SignedInvitation {
        Invitation {
            inviter: inviter.user_id(),
            code: code_id(code),
            issued_unix: 1_000,
            expires_unix: 1_000 + DEFAULT_VALIDITY,
            uses: 1,
        }
        .sign(inviter)
    }

    #[test]
    fn an_invitation_signed_by_a_member_verifies() {
        let alice = member(1);
        assert!(invitation(&alice, &secret(9)).verify().is_ok());
    }

    #[test]
    fn the_secret_opens_its_own_invitation_and_no_other() {
        let alice = member(1);
        let signed = invitation(&alice, &secret(9));
        assert!(signed.opens_with(&secret(9)));
        assert!(!signed.opens_with(&secret(10)));
    }

    #[test]
    fn red_team_the_coordinator_cannot_write_itself_an_invitation() {
        // THE ATTACK this whole file exists to stop. The coordinator holds every
        // invitation and every account, so if it could mint one it would be the
        // admission authority rather than a notice board — and `ECONOMICS.md` §7
        // is a promise that it is not. It can refuse, which is denial of service
        // and is in the threat model; it must not be able to admit.
        //
        // If this test fails, "an invitation is a member's endorsement" is a
        // sentence about a value anybody holding the database can forge.
        let alice = member(1);
        let coordinator = member(0xC0);
        let code = secret(9);

        // The coordinator has the whole invitation and simply re-signs it.
        let genuine = invitation(&alice, &code);
        let forged = SignedInvitation {
            invitation: genuine.invitation.clone(),
            signature: coordinator.sign(INVITATION_DOMAIN, b"whatever it likes"),
        };
        assert!(
            forged.verify().is_err(),
            "a signature that is not the inviter's was accepted"
        );

        // And it cannot promote itself by naming itself the inviter, because
        // the directory checks the inviter is an existing member and the
        // coordinator is not one.
        let self_issued = Invitation {
            inviter: coordinator.user_id(),
            ..genuine.invitation.clone()
        }
        .sign(&coordinator);
        assert!(
            self_issued.verify().is_ok(),
            "this much is well-formed; membership is the directory's question"
        );
        assert_ne!(
            self_issued.invitation.inviter,
            alice.user_id(),
            "the forgery is attributable to whoever signed it, which is the point"
        );
    }

    #[test]
    fn red_team_an_invitation_cannot_be_edited_after_signing() {
        // Every field is signed, so a redeemer cannot extend the expiry, raise
        // the number of uses, or point the endorsement at somebody else. If any
        // of them were outside the payload, an invitation for one machine on
        // one afternoon would become an open door.
        let alice = member(1);
        let signed = invitation(&alice, &secret(9));

        let edits: Vec<(&str, Invitation)> = vec![
            (
                "expiry extended",
                Invitation {
                    expires_unix: signed.invitation.expires_unix + 10_000_000,
                    ..signed.invitation.clone()
                },
            ),
            (
                "uses raised",
                Invitation {
                    uses: 1_000,
                    ..signed.invitation.clone()
                },
            ),
            (
                "inviter swapped",
                Invitation {
                    inviter: member(2).user_id(),
                    ..signed.invitation.clone()
                },
            ),
            (
                "code swapped",
                Invitation {
                    code: code_id(&secret(11)),
                    ..signed.invitation.clone()
                },
            ),
        ];

        for (what, tampered) in edits {
            let forged = SignedInvitation {
                invitation: tampered,
                signature: signed.signature,
            };
            assert!(
                forged.verify().is_err(),
                "{what}: the field is outside the signed payload"
            );
        }
    }

    #[test]
    fn an_invitation_good_for_nothing_is_refused_rather_than_stored() {
        // Zero uses, or an expiry before the issue date. Neither can admit
        // anybody, so accepting them fills the directory with rows that exist
        // only to be checked and rejected for ever.
        let alice = member(1);
        let mut none = Invitation {
            inviter: alice.user_id(),
            code: code_id(&secret(9)),
            issued_unix: 1_000,
            expires_unix: 2_000,
            uses: 0,
        };
        assert!(none.clone().sign(&alice).verify().is_err());

        none.uses = 1;
        none.expires_unix = 500;
        assert!(none.sign(&alice).verify().is_err());
    }

    #[test]
    fn the_code_id_reveals_nothing_about_the_secret() {
        // The coordinator stores the id, not the secret, so a stolen directory
        // is a list of hashes rather than a list of ways in. Two secrets that
        // differ in one bit must not produce related ids.
        let mut near = secret(9);
        near[SECRET_LEN - 1] ^= 1;
        let a = code_id(&secret(9));
        let b = code_id(&near);
        assert_ne!(a, b);
        let shared = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
        assert!(shared < 8, "{shared} of 32 bytes match between neighbours");
    }
}
