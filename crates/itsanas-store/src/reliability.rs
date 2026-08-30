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
//! It also keeps being audited, which is the only way back: one passing
//! challenge clears the record. A host that lost a disk and genuinely recovered
//! should not be exiled for it.
//!
//! # Consecutive, not cumulative
//!
//! A ratio would punish a peer that had a bad month a year ago and has been
//! flawless since, and would forgive one that fails everything today because it
//! used to be good. What matters is whether it is failing *now*, so the counter
//! resets on any pass.

use itsanas_crypto::DeviceId;
use serde::{Deserialize, Serialize};

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
}

impl Reliability {
    /// Whether this peer should still be offered new file content.
    ///
    /// Log segments are sent regardless — they are kilobytes, and cutting a
    /// peer out of the log would stop it relaying for devices that have done
    /// nothing wrong.
    #[must_use]
    pub const fn worth_sending_to(&self) -> bool {
        self.consecutive_failures < FAILURES_BEFORE_PAUSE
    }

    /// Record one answered challenge.
    ///
    /// Clears the consecutive count: one pass is the way back, because a host
    /// that lost a disk and genuinely recovered should not be exiled for it.
    pub const fn passed_one(&mut self) {
        self.consecutive_failures = 0;
        self.passed = self.passed.saturating_add(1);
    }

    /// Record one failed challenge.
    pub const fn failed_one(&mut self, now: u64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
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
            "{} has failed {} storage challenges in a row and is no longer being \
             sent new data. It still receives the log, and one passing challenge \
             clears this.",
            device.short(),
            self.consecutive_failures
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
    fn one_pass_clears_the_record() {
        // The way back. A host that lost a disk and genuinely recovered should
        // not be exiled for it, and a sanction with no exit is a ban.
        let mut record = Reliability::default();
        for round in 1..=FAILURES_BEFORE_PAUSE {
            record.failed_one(u64::from(round));
        }
        assert!(!record.worth_sending_to());

        record.passed_one();
        assert!(record.worth_sending_to());
        assert_eq!(record.consecutive_failures, 0);
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
        assert!(complaint.contains("clears this"), "no way back was offered");
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
        };
        record.failed_one(1);
        assert_eq!(record.consecutive_failures, u32::MAX);
        assert_eq!(record.failed, u64::MAX);
        assert!(!record.worth_sending_to());

        record.passed_one();
        assert_eq!(record.passed, u64::MAX);
    }
}
