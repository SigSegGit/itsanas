//! What a peer may ask a coordinator, and what it may get back.
//!
//! # The surface is small on purpose
//!
//! This is the only component of ITSaNAS that sits on a public address, so
//! every message here is an attack surface and a compatibility commitment. The
//! test applied to each one was: *could a peer obtain this another way?* Six
//! survived.
//!
//! It is also the surface a DHT would eventually replace. Keeping it to an
//! address book plus an escrow locker is what makes that swap a contained
//! change rather than a rewrite — see `docs/DESIGN.md` §8.
//!
//! # What a coordinator can do with these messages
//!
//! Refuse to answer, and lie about who is online. That is the whole list, and
//! it is the same list a compromised coordinator has. Nothing here carries a
//! key, a chunk, or a plaintext; nothing here lets it delete or forge anything.
//! Registrations, claims and presences are all signed by keys it does not hold,
//! and the escrow blob is sealed under a passphrase it never sees.

use itsanas_crypto::UserId;
use serde::{Deserialize, Serialize};

use crate::claim::{Presence, SignedClaim, SignedPresence};
use crate::directory::{Account, SignedRegistration};
use crate::invitation::{Secret, SignedInvitation};

/// The protocol version this build speaks.
pub const COORD_VERSION: u16 = 1;

/// Longest username accepted on the wire, before the directory sees it.
///
/// Duplicated from `directory::MAX_USERNAME_LEN` rather than imported, because
/// the two answer different questions: what may be stored, and what may be
/// *parsed* from a stranger. If they ever diverge, the smaller one wins here.
pub const MAX_WIRE_USERNAME: usize = 64;

/// Most peers returned for one lookup.
///
/// Bounds the reply a stranger can provoke. A user with more devices than this
/// gets the first few by device id, which is stable, so a client asking twice
/// sees the same answer rather than a shuffling set.
pub const MAX_PEERS_RETURNED: usize = 32;

/// Most requests one connection may make before it is closed.
///
/// A connection is cheap to open and cheaper to leave open. Capping the work
/// per connection means a client that wants more has to pay for another
/// handshake, which is the expensive part.
pub const MAX_REQUESTS_PER_CONNECTION: usize = 16;

// A stranger must not be able to provoke an unbounded reply: that is how a
// lookup service becomes an amplifier pointed at somebody else. Checked at
// compile time, because a runtime test of two constants proves nothing that the
// compiler cannot prove first.
const _: () = assert!(MAX_PEERS_RETURNED <= 64);
const _: () = assert!(MAX_WIRE_USERNAME <= 64);

/// What a peer asks for.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Request {
    /// Agree on a protocol version before anything else.
    Hello {
        /// The version the caller speaks.
        version: u16,
    },

    /// Claim a username for a user id. Signed by the user's own key.
    ///
    /// The coordinator verifies the signature and refuses a name already held
    /// by a different key, so it cannot hand somebody else's name away — but it
    /// can refuse, which is denial of service and is in the threat model.
    Register(Box<SignedRegistration>),

    /// Claim a username, presenting the invitation secret that admits you.
    ///
    /// Separate from [`Request::Register`] rather than an `Option` on it,
    /// because the two are different acts: one is a member refreshing their own
    /// entry, the other is a stranger asking to be let in. A coordinator that
    /// admits openly accepts both; one that admits by invitation accepts the
    /// first only from members it already knows.
    RegisterInvited {
        /// The registration, signed by the key being registered.
        registration: Box<SignedRegistration>,
        /// The secret the inviter handed over, out of band.
        secret: Secret,
    },

    /// Lodge an invitation this member has signed.
    ///
    /// The coordinator files it under the hash of the secret, so its database
    /// never holds a working code.
    Invite(Box<SignedInvitation>),

    /// Look a username up.
    ///
    /// Answers with a user id, which is a public key. A coordinator that lies
    /// here produces a name mapping to a key nobody can use.
    Lookup {
        /// The name, lowercase ASCII.
        username: String,
    },

    /// Enrol a device under its owner. Signed by the owner.
    Claim(Box<SignedClaim>),

    /// Say where a device can be reached. Signed by the device.
    ///
    /// Separate from [`Request::Claim`] because they change at different rates:
    /// a laptop moving between networks announces constantly and must never
    /// need the key that can revoke everything.
    Announce(Box<SignedPresence>),

    /// Where are this user's devices?
    Peers {
        /// The user whose devices to list.
        user: UserId,
    },

    /// Store, or withdraw, this user's passphrase-sealed escrow blob.
    ///
    /// `Some` stores it and turns passphrase recovery on; `None` withdraws it
    /// and turns recovery off. One message rather than two because the second
    /// direction has to exist and is easy to forget: a member who decides that
    /// recovery-by-passphrase is too much risk for their threat model must be
    /// able to take it back, and escrow is off until asked for.
    ///
    /// Authorised by the connection: the calling device must already hold a
    /// live claim from the user it is storing for. There is no signature on the
    /// blob itself because there is nothing useful to sign — the contents are
    /// opaque and self-authenticating under Argon2id.
    PutEscrow {
        /// The sealed container, or nothing to withdraw it.
        blob: Option<Vec<u8>>,
    },

    /// Fetch a user's escrow blob by name.
    ///
    /// **Deliberately unauthenticated**, because a machine recovering from
    /// nothing has no device, no claim and no key — that is the situation the
    /// escrow exists for. The protection is a rate limit, which is the one
    /// thing a central component offers that a DHT cannot.
    GetEscrow {
        /// The account name.
        username: String,
    },
}

/// What a coordinator answers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Response {
    /// Version agreed.
    Welcome {
        /// The version the server speaks.
        version: u16,
    },
    /// The request was carried out and there is nothing to return.
    Done,
    /// An account.
    Account(Box<Account>),
    /// Reachable devices, most recently seen first.
    Peers(Vec<Presence>),
    /// A sealed escrow container.
    Escrow(Vec<u8>),
    /// No such account, name, or blob.
    Missing,
    /// Refused, with a reason a person can act on.
    ///
    /// Refusals are text because the situations are open-ended and an operator
    /// reading a log needs the sentence, not a code to look up. Nothing branches
    /// on the contents.
    Refused(String),
}

impl Request {
    /// Whether this request may be made before the caller has proved anything.
    ///
    /// Only two: agreeing a version, and fetching an escrow blob. Everything
    /// else either carries its own signature or needs the calling device to
    /// hold a claim. This is the list the hostile-internet argument rests on,
    /// so it is a function rather than a comment.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Hello { .. } | Self::GetEscrow { .. })
    }

    /// A short name for logs, with no caller-controlled text in it.
    ///
    /// A log line that quoted a stranger's username would let them write into
    /// the operator's journal.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Register(_) => "register",
            Self::RegisterInvited { .. } => "register-invited",
            Self::Invite(_) => "invite",
            Self::Lookup { .. } => "lookup",
            Self::Claim(_) => "claim",
            Self::Announce(_) => "announce",
            Self::Peers { .. } => "peers",
            Self::PutEscrow { .. } => "put-escrow",
            Self::GetEscrow { .. } => "get-escrow",
        }
    }
}

#[cfg(test)]
mod tests {
    use itsanas_crypto::ID_LEN;

    use super::*;

    #[test]
    fn only_hello_and_escrow_retrieval_are_reachable_without_proving_anything() {
        // The hostile-internet argument rests on this list being short. If a
        // message is added and lands here by accident, a stranger on a public
        // address can reach it — so the list is asserted rather than described.
        assert!(Request::Hello { version: 1 }.is_open());
        assert!(
            Request::GetEscrow {
                username: "a".to_owned()
            }
            .is_open()
        );

        assert!(
            !Request::Lookup {
                username: "a".to_owned()
            }
            .is_open()
        );
        assert!(
            !Request::Peers {
                user: UserId::from_bytes([0; ID_LEN])
            }
            .is_open()
        );
        assert!(!Request::PutEscrow { blob: None }.is_open());
    }

    #[test]
    fn a_log_line_never_contains_anything_a_caller_wrote() {
        // Otherwise a stranger picks their own username and writes into the
        // operator's journal — newlines, escape sequences, whatever they like.
        let hostile = Request::Lookup {
            username: "\n\u{1b}[2Jroot: everything is fine".to_owned(),
        };
        assert_eq!(hostile.kind(), "lookup");
    }

    #[test]
    fn a_request_round_trips_through_postcard() {
        let request = Request::Peers {
            user: UserId::from_bytes([3; ID_LEN]),
        };
        let bytes = postcard::to_stdvec(&request).unwrap();
        assert_eq!(postcard::from_bytes::<Request>(&bytes).unwrap(), request);
    }
}
