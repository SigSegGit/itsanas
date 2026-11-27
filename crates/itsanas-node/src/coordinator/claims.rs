//! Device claims after enrolment: withdrawing a device, and asking a pinned
//! coordinator about the standing of other accounts' devices.

use itsanas_coord::claim::{NodeClaim, SignedClaim};
use itsanas_coord::protocol::{Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceId, DeviceKeys};

use super::{dial, dial_as};
use crate::node::Node;
use crate::{NodeError as CliError, Result};

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
    match ask_to_forget(node, device, now)? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Send the withdrawal of `device` and hand back the coordinator's answer: an
/// error here means nobody answered, which is the difference a marker left for
/// a later login needs and a refusal in words cannot show.
fn ask_to_forget(node: &Node, device: DeviceId, now: u64) -> Result<Response> {
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
    // Kept here first, whatever the coordinator says or whether it answers:
    // the account's machines learn it from each other. Not for this machine
    // itself, whose node is erased after a logout.
    if device != node.store.device_id()
        && let Ok(bytes) = postcard::to_stdvec(&claim)
    {
        node.withdrawn_here(device, &bytes)?;
    }

    let mut client = dial(node)?;
    Ok(client.ask(&Request::Claim(Box::new(claim)))?)
}

/// Withdraw `device`, an id this node's shell kept from an earlier logout. `Ok`
/// when the coordinator answered, whichever way: an id of another account is
/// refused there, harmlessly, and asking again would be refused the same way.
///
/// # Errors
///
/// When the coordinator could not be reached: the id is worth keeping.
pub fn withdraw_old_device(node: &Node, device: DeviceId, now: u64) -> Result<()> {
    ask_to_forget(node, device, now).map(|_| ())
}

/// Withdraw this device from its own account, for a logout and nothing else.
///
/// [`withdraw_device`] refuses this because the machine would go on running,
/// unlisted. A logout removes that premise: the node is erased straight after,
/// and a later login on this device mints a new id. Without it every logout
/// and login would keep one of the account's device slots for ever.
///
/// # Errors
///
/// As [`forget_device`].
pub fn withdraw_self(node: &Node, now: u64) -> Result<()> {
    forget_device(node, node.store.device_id(), now)
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
}
