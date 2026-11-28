//! The recovery container lodged with the coordinator, and opening it by
//! name and passphrase.

use itsanas_coord::protocol::{Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, KdfParams, Keystore};

use super::dial;
use crate::node::{ESCROW_LABEL, Node};
use crate::{NodeError as CliError, Result};

/// Seal this node's identity under `passphrase` and lodge it with the
/// coordinator, or withdraw what is lodged.
///
/// The container is the same shape as the local keystore but sealed under a
/// **different label**, so a copy of one cannot be substituted for the other.
/// The coordinator holds opaque bytes and never sees a passphrase.
pub fn set_escrow(node: &Node, passphrase: Option<&str>, secrets: &[u8]) -> Result<()> {
    let blob = match passphrase {
        Some(passphrase) => Some(recovery_container(passphrase, secrets)?),
        None => None,
    };

    let mut client = dial(node)?;
    match client.ask(&Request::PutEscrow { blob })? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// The recovery container `set_escrow` lodges: `secrets` sealed under
/// `passphrase`. One function so that the coordinator's decoys can be held to
/// its shape (`the_coordinators_decoy_has_the_shape_of_a_real_recovery_container`).
///
/// # Errors
///
/// If sealing fails.
pub fn recovery_container(passphrase: &str, secrets: &[u8]) -> Result<Vec<u8>> {
    debug_assert!(KdfParams::RECOMMENDED.meets_production_floor());
    Ok(Keystore::lock(passphrase, ESCROW_LABEL, secrets, KdfParams::RECOMMENDED)?.to_bytes())
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
    fetch_escrow_with_code(address, expect, username, passphrase, "")
}

/// [`fetch_escrow`] with the code the coordinator mailed to the account's
/// verified address (`ask_recovery_code`); empty for an account with none.
/// A coordinator older than email is asked as before, without it.
///
/// # Errors
///
/// [`NOTHING_OPENS`] whatever was wrong: name, passphrase or code.
pub fn fetch_escrow_with_code(
    address: &str,
    expect: Option<DeviceId>,
    username: &str,
    passphrase: &str,
    code: &str,
) -> Result<Vec<u8>> {
    // A throwaway device key: this machine has no identity yet, and the
    // coordinator does not need it to have one.
    let device = itsanas_crypto::DeviceKeys::generate()?;
    let mut client = CoordClient::connect(address, &device, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))?;

    let request = if client.version() >= itsanas_coord::protocol::EMAIL_VERSION {
        Request::RecoverEscrow {
            username: username.to_owned(),
            code: code.trim().to_owned(),
        }
    } else {
        Request::GetEscrow {
            username: username.to_owned(),
        }
    };
    let answer = client.ask(&request)?;
    open_recovery_answer(answer, username, passphrase)
}

/// What a recovery makes of the coordinator's answer: the secrets, or
/// [`NOTHING_OPENS`] after one Argon2id run whatever was wrong.
fn open_recovery_answer(answer: Response, username: &str, passphrase: &str) -> Result<Vec<u8>> {
    let blob = match answer {
        Response::Escrow(blob) => blob,
        // An older coordinator says so for a name with nothing; today's hands
        // out a decoy. Against the older one the client opens a decoy of its
        // own, so a wrong name costs the Argon2id run a wrong passphrase
        // does and reads the same: neither the message nor the time confirms
        // a name.
        Response::Missing => {
            let key = itsanas_crypto::SymmetricKey::random()?;
            let _ = Keystore::decoy(
                &key,
                username.as_bytes(),
                itsanas_coord::directory::RECOVERY_PAYLOAD_LEN,
                KdfParams::RECOMMENDED,
            )
            .unlock(passphrase, ESCROW_LABEL);
            return Err(CliError::Usage(NOTHING_OPENS.to_owned()));
        }
        Response::Refused(why) => return Err(CliError::Usage(why)),
        other => return Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    };

    Keystore::from_bytes(&blob)?
        .unlock(passphrase, ESCROW_LABEL)
        .map_err(|_| CliError::Usage(NOTHING_OPENS.to_owned()))
}

/// What a recovery by name says when it fails, whichever part was wrong.
///
/// A wrong username, an account that lodged no container, a wrong passphrase
/// and a tampered container are one message, on purpose: telling them apart
/// would tell whoever types names at it which accounts exist.
pub const NOTHING_OPENS: &str = concat!(
    "nothing opens with this username and passphrase: either is wrong, or the account ",
    "keeps no recovery copy on this coordinator. Recover with the 24 words instead."
);

#[cfg(test)]
mod tests {
    use super::*;

    /// THE ATTACK, against a coordinator older than its decoys: it answers
    /// `Missing` for a name with nothing, at once. A client that then stopped
    /// there said nothing different, but said it an Argon2id run sooner than
    /// for a wrong passphrase, and the time named the accounts.
    #[test]
    fn red_team_an_older_coordinators_missing_costs_the_client_what_a_wrong_passphrase_does() {
        let real = recovery_container("the passphrase", &[7; 64]).expect("seal");
        let timed = |answer: Response| {
            let started = std::time::Instant::now();
            let outcome = open_recovery_answer(answer, "alice", "a guess");
            (
                outcome.map_err(|error| error.to_string()),
                started.elapsed(),
            )
        };
        let quickest = |answer: &Response| {
            let tries: Vec<_> = (0..3).map(|_| timed(answer.clone())).collect();
            let fastest = tries.iter().map(|(_, took)| *took).min().expect("three");
            (tries[0].0.clone(), fastest)
        };
        let (wrong_passphrase, slow) = quickest(&Response::Escrow(real));
        let (no_container, fast) = quickest(&Response::Missing);
        assert_eq!(
            no_container, wrong_passphrase,
            "an older coordinator's Missing read differently from a wrong passphrase"
        );
        assert!(
            fast * 2 >= slow,
            "Missing failed in {fast:?} and a wrong passphrase in {slow:?}: the time names the account"
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
