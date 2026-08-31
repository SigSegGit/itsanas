//! Whether a peer has been worth sending data to.
//!
//! # The attack this closes
//!
//! Auditing catches a host that discarded what it accepted, withdraws the
//! record, and the next round re-uploads. Good — until the host discards it
//! again. Then the cycle repeats forever: every round costs the owner the full
//! upload and buys nothing, and the host pays nothing at all. A free, indefinite
//! bandwidth drain on whoever trusted it, and the more data you have the more it
//! costs you.
//!
//! Detection without memory is not a defence. Something has to notice that this
//! is the fourth time.
//!
//! # The sanction is to stop spending, never to destroy
//!
//! A peer that keeps failing stops being offered **new content**. It is not
//! blocked, nothing of its own is touched, and it keeps receiving log segments —
//! which are kilobytes and keep it able to relay. `ECONOMICS.md` §5 sets the
//! rule for every sanction in this project: restrict new commitments, never
//! destroy existing ones, and this is that rule applied to bandwidth.
//!
//! # Coming back costs as many rounds as falling did
//!
//! It keeps being audited, and that is the way back. Not in one round, though,
//! and the first version of this got that wrong in a way that undid the
//! sanction entirely.
//!
//! A pass used to zero the counter. A host could therefore throw away a
//! terabyte, take three failures, keep the single probe chunk it was handed for
//! one round, and be fully trusted again — then receive the whole store,
//! discard it, and repeat. The sanction cost it one chunk held for one round,
//! for ever.
//!
//! So a pass now decrements by one rather than zeroing, and the pause lifts
//! only when the count reaches zero. Three failures cost three passing rounds;
//! thirty cost thirty. A host that lost a disk and genuinely recovered is back
//! within minutes, and one that is gaming the mechanism pays in proportion to
//! how much it has gamed it.
//!
//! # Recent, not cumulative
//!
//! A ratio would punish a peer that had a bad month a year ago and has been
//! flawless since, and would forgive one that fails everything today because it
//! used to be good. What matters is whether it is failing *now*, so the counter
//! walks back down as well as up and a peer that has been answering for long
//! enough carries nothing.
//!
//! The lifetime totals are kept beside it and never decremented, because
//! somebody deciding whether to keep a peer at all wants to know it has been
//! paused nine times, and the sanction itself deliberately does not.

use itsanas_crypto::DeviceId;
use serde::{Deserialize, Serialize};

/// Longest probation a peer can dig itself into.
///
/// Recovery is one passing round per outstanding failure, so without a ceiling
/// a peer that failed for a week would need a week to come back — which is a
/// ban again, arrived at by arithmetic instead of by decision. Thirty-five is
/// about three hours at the service beat: long enough to be a real cost,
/// short enough that a machine which was genuinely broken and is now fixed
/// rejoins the same afternoon.
pub const PROBATION_CEILING: u32 = FAILURES_BEFORE_PAUSE + 32;

/// Consecutive audit failures before a peer stops being offered new content.
///
/// Three, and the exact number matters less than it being above one. A single
/// failure is ordinary: a host that was mid-restart, a disk that was swapped, a
/// chunk collected on one side of a race. Reacting to one would make a
/// household stop syncing every time a machine rebooted at the wrong moment.
///
/// Three consecutive failures, each after this node re-sent the data, is not an
/// accident.
pub const FAILURES_BEFORE_PAUSE: u32 = 3;

/// What auditing this peer has shown, over time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reliability {
    /// Audit failures since the last pass.
    pub consecutive_failures: u32,
    /// Challenges this peer has answered, ever. Diagnostics only.
    pub passed: u64,
    /// Challenges it has failed, ever. Diagnostics only.
    pub failed: u64,
    /// This node's clock at the most recent failure, or zero.
    pub last_failure_unix: u64,
    /// Whether the sanction is currently in force.
    ///
    /// Separate from the counter because the thresholds differ in each
    /// direction: the pause starts at [`FAILURES_BEFORE_PAUSE`] and lifts only
    /// at zero. Without the hysteresis, a peer paused at three failures would
    /// be released by the single pass that took it back to two, and the
    /// sanction would cost it one chunk held for one round.
    pub paused: bool,
}

impl Reliability {
    /// Whether this peer should still be offered new file content.
    ///
    /// Log segments are sent regardless — they are kilobytes, and cutting a
    /// peer out of the log would stop it relaying for devices that have done
    /// nothing wrong.
    #[must_use]
    pub const fn worth_sending_to(&self) -> bool {
        !self.paused
    }

    /// How many more passing rounds this peer owes before the pause lifts.
    #[must_use]
    pub const fn rounds_to_reinstatement(&self) -> u32 {
        if self.paused {
            self.consecutive_failures
        } else {
            0
        }
    }

    /// Record one answered round.
    ///
    /// Pays off **one** failure, not all of them. Zeroing the count was the
    /// original design and it made the sanction free: keep one probe chunk for
    /// one round and every previous failure was forgiven.
    pub const fn passed_one(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_sub(1);
        self.passed = self.passed.saturating_add(1);
        if self.consecutive_failures == 0 {
            self.paused = false;
        }
    }

    /// Record one failed round.
    pub const fn failed_one(&mut self, now: u64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures > PROBATION_CEILING {
            self.consecutive_failures = PROBATION_CEILING;
        }
        if self.consecutive_failures >= FAILURES_BEFORE_PAUSE {
            self.paused = true;
        }
        self.failed = self.failed.saturating_add(1);
        self.last_failure_unix = now;
    }

    /// A sentence for a status line, or `None` when there is nothing to say.
    #[must_use]
    pub fn complaint(&self, device: &DeviceId) -> Option<String> {
        if self.worth_sending_to() {
            return None;
        }
        Some(format!(
            concat!(
                "{} has failed {} storage challenges in a row and is no longer ",
                "being sent new data. It still receives the log and one chunk a ",
                "round to answer for; {} more answered rounds clear this."
            ),
            device.short(),
            self.consecutive_failures,
            self.rounds_to_reinstatement()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_peer_is_worth_sending_to() {
        assert!(Reliability::default().worth_sending_to());
    }

    #[test]
    fn one_failure_is_not_enough_to_stop_sending() {
        // A host mid-restart, a swapped disk, a chunk collected on one side of
        // a race. Reacting to a single failure would make a household stop
        // syncing every time a machine rebooted at the wrong moment.
        let mut record = Reliability::default();
        record.failed_one(100);
        assert!(record.worth_sending_to());
    }

    #[test]
    fn red_team_a_host_that_keeps_discarding_stops_costing_bandwidth() {
        // THE ATTACK: accept everything, delete it, repeat. Auditing catches
        // each round and the owner re-uploads each round — so the host pays
        // nothing and the owner pays the full upload forever. Detection without
        // memory is not a defence.
        //
        // If this test fails, anyone can drain an owner's uplink indefinitely
        // by agreeing to store their data and then not.
        let mut record = Reliability::default();
        for round in 1..=FAILURES_BEFORE_PAUSE {
            record.failed_one(u64::from(round));
        }
        assert!(
            !record.worth_sending_to(),
            "a host that failed {FAILURES_BEFORE_PAUSE} audits in a row is still \
             being sent data"
        );
    }

    #[test]
    fn the_way_back_is_one_answered_round_per_failure() {
        // A sanction with no exit is a ban. A host that lost a disk and
        // genuinely recovered is back in three rounds, which at the service
        // beat is a quarter of an hour.
        let mut record = Reliability::default();
        for round in 1..=FAILURES_BEFORE_PAUSE {
            record.failed_one(u64::from(round));
        }
        assert!(!record.worth_sending_to());
        assert_eq!(record.rounds_to_reinstatement(), FAILURES_BEFORE_PAUSE);

        for remaining in (1..FAILURES_BEFORE_PAUSE).rev() {
            record.passed_one();
            assert!(
                !record.worth_sending_to(),
                "the pause lifted with {remaining} failures still outstanding"
            );
        }
        record.passed_one();
        assert!(record.worth_sending_to());
        assert_eq!(record.consecutive_failures, 0);
    }

    #[test]
    fn red_team_one_kept_chunk_does_not_buy_back_a_discarded_terabyte() {
        // THE ATTACK the first version of the way back allowed. A pass zeroed
        // the counter, so a host could throw away everything, take its three
        // failures, keep the single probe chunk it was handed for one round,
        // and be fully trusted again — then receive the whole store, discard
        // it, and repeat for ever. The sanction cost one chunk for one round.
        //
        // If this test fails, the pause is decorative: a host can hold nothing
        // at all and still be sent everything, half the time.
        let mut record = Reliability::default();
        for round in 1..=12u32 {
            record.failed_one(u64::from(round));
        }
        assert!(!record.worth_sending_to());

        record.passed_one();
        assert!(
            !record.worth_sending_to(),
            "one answered round wiped out twelve failures, so the sanction is \
             free to a host that keeps a single chunk for a single round"
        );
        assert_eq!(record.rounds_to_reinstatement(), 11);
    }

    #[test]
    fn probation_is_bounded_so_it_never_becomes_a_ban_by_arithmetic() {
        // Recovery is one round per failure, so without a ceiling a peer that
        // was broken for a week would owe a week of answered rounds. That is a
        // ban reached by arithmetic rather than by decision, and this project
        // does not ban.
        let mut record = Reliability::default();
        for round in 0..10_000u32 {
            record.failed_one(u64::from(round));
        }
        assert_eq!(record.consecutive_failures, PROBATION_CEILING);
        assert_eq!(record.failed, 10_000);

        for _ in 0..PROBATION_CEILING {
            record.passed_one();
        }
        assert!(
            record.worth_sending_to(),
            "a peer that answered {PROBATION_CEILING} rounds in a row is still \
             paused, so the sanction has no exit"
        );
    }

    #[test]
    fn the_lifetime_totals_survive_a_reset() {
        // Consecutive failures decide the sanction; the totals are for somebody
        // deciding whether to keep a peer at all, and clearing them on every
        // pass would hide a host that fails half the time.
        let mut record = Reliability::default();
        record.failed_one(1);
        record.passed_one();
        record.failed_one(2);
        record.passed_one();

        assert_eq!(record.failed, 2);
        assert_eq!(record.passed, 2);
        assert_eq!(record.consecutive_failures, 0);
        assert!(record.worth_sending_to());
    }

    #[test]
    fn a_paused_peer_explains_itself_and_a_healthy_one_says_nothing() {
        let device = DeviceId::from_bytes([7; 32]);
        assert!(Reliability::default().complaint(&device).is_none());

        let mut record = Reliability::default();
        for round in 1..=FAILURES_BEFORE_PAUSE {
            record.failed_one(u64::from(round));
        }
        let complaint = record.complaint(&device).expect("a paused peer says why");
        assert!(complaint.contains("clear this"), "no way back was offered");
        assert!(
            complaint.contains(&FAILURES_BEFORE_PAUSE.to_string()),
            "the complaint does not say how many rounds it owes: {complaint}"
        );
    }

    #[test]
    fn counters_saturate_rather_than_wrapping() {
        // A wrap would turn a peer that failed four billion challenges into a
        // trusted one.
        let mut record = Reliability {
            consecutive_failures: u32::MAX,
            passed: u64::MAX,
            failed: u64::MAX,
            last_failure_unix: 0,
            paused: true,
        };
        record.failed_one(1);
        assert_eq!(record.consecutive_failures, PROBATION_CEILING);
        assert_eq!(record.failed, u64::MAX);
        assert!(!record.worth_sending_to());

        record.passed_one();
        assert_eq!(record.passed, u64::MAX);
    }
}
