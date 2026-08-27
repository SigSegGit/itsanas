//! The bargain, made arithmetic.
//!
//! `docs/ECONOMICS.md` is the reasoning; this is the implementation, and the two
//! are meant to be read together. Anything here that looks arbitrary is
//! explained there.
//!
//! # No floating point, for the same reason as placement
//!
//! Availability is a fraction, and a fraction is where an `f64` wants to go. It
//! does not go there. Members must be able to check their own standing and get
//! the same answer the coordinator got, and two machines disagreeing in the last
//! bit about whether someone is in default is a dispute nobody can settle.
//! Availability is per-mille, everything is integer, and every division is
//! documented where it truncates.
//!
//! Truncation always rounds *against* the member: entitlement is floored. A
//! member is never told they have more room than the arithmetic supports.

use itsanas_crypto::{DeviceId, UserId};
use serde::{Deserialize, Serialize};

/// Bytes a member must pledge for each byte they store.
///
/// Equal to the replication factor, and not by coincidence — see
/// `docs/ECONOMICS.md` §1. With `R` replicas the network must physically hold
/// `R × S` for every `S` a member stores, so a balanced network needs every
/// member to offer `R × S`.
pub const CONTRIBUTION_RATIO: u64 = 3;

/// Availability at or above which a node counts as an *anchor*.
///
/// Anchors are what make the network readable rather than merely durable. See
/// `docs/ECONOMICS.md` §2 for why a swarm of laptops cannot do this alone.
pub const ANCHOR_AVAILABILITY_PER_MILLE: u16 = 900;

/// Lowest availability a node can be credited with.
///
/// Stops a member who went on holiday from having their entitlement collapse to
/// zero and being declared in default the moment they come back.
pub const AVAILABILITY_FLOOR_PER_MILLE: u16 = 50;

/// How long a member may be over their entitlement before writes stop being
/// replicated.
pub const GRACE_SECONDS: u64 = 14 * 24 * 3600;

/// How long a member may be over their entitlement before hosts may reclaim.
pub const DEFAULT_AFTER_SECONDS: u64 = 60 * 24 * 3600;

/// Entitlement granted to a new member regardless of what they have pledged.
pub const JOINING_ALLOWANCE: u64 = 10 * 1024 * 1024 * 1024;

/// How long the joining allowance lasts.
pub const JOINING_PERIOD_SECONDS: u64 = 30 * 24 * 3600;

/// What one device contributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceContribution {
    pub device: DeviceId,
    /// What its owner pledged for it.
    pub pledged_bytes: u64,
    /// Smoothed fraction of time it has been reachable, in per mille.
    pub availability_per_mille: u16,
}

impl DeviceContribution {
    /// Bytes this device is credited with.
    ///
    /// Pledged space on a machine that is usually off is not worth what the
    /// number says. Counting it at face value would let the network promise
    /// durability it cannot deliver, and the people who find out are the ones
    /// who lose files.
    #[must_use]
    pub fn effective_bytes(&self) -> u64 {
        let availability = u64::from(
            self.availability_per_mille
                .clamp(AVAILABILITY_FLOOR_PER_MILLE, 1000),
        );
        // Multiply before dividing: the other order throws away everything
        // below a kilobyte for a node at 1‰.
        self.pledged_bytes.saturating_mul(availability) / 1000
    }

    /// Whether this device can serve as an availability anchor.
    #[must_use]
    pub fn is_anchor(&self) -> bool {
        self.availability_per_mille >= ANCHOR_AVAILABILITY_PER_MILLE
    }
}

/// Where a member stands with the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberState {
    /// Inside the joining period, relying on the allowance.
    Joining,
    /// Using no more than they are entitled to.
    Good,
    /// Over, recently. Nothing has changed yet.
    Over,
    /// Over for longer than the grace period. New writes stay local.
    Grace,
    /// Over for a long time. Hosts may reclaim space.
    Default,
}

impl MemberState {
    /// Whether new writes should still be replicated to peers.
    #[must_use]
    pub const fn may_replicate(self) -> bool {
        matches!(self, Self::Joining | Self::Good | Self::Over)
    }

    /// Whether hosts may reclaim space from this member.
    #[must_use]
    pub const fn may_be_reclaimed(self) -> bool {
        matches!(self, Self::Default)
    }

    /// Whether the member should be told something.
    #[must_use]
    pub const fn needs_attention(self) -> bool {
        matches!(self, Self::Over | Self::Grace | Self::Default)
    }
}

/// A member's full position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub user: UserId,
    /// Raw bytes pledged across every live device.
    pub pledged_bytes: u64,
    /// Bytes credited after weighting by availability.
    pub effective_bytes: u64,
    /// Bytes the member may store.
    pub entitlement_bytes: u64,
    /// Bytes the member is storing.
    pub usage_bytes: u64,
    /// Whether any of their devices is an anchor.
    pub has_anchor: bool,
    pub state: MemberState,
}

impl Standing {
    /// How much room is left, or zero if over.
    #[must_use]
    pub const fn headroom_bytes(&self) -> u64 {
        self.entitlement_bytes.saturating_sub(self.usage_bytes)
    }

    /// How far over, or zero if within.
    #[must_use]
    pub const fn excess_bytes(&self) -> u64 {
        self.usage_bytes.saturating_sub(self.entitlement_bytes)
    }
}

/// What the coordinator knows about a member when working out their standing.
#[derive(Clone, Debug)]
pub struct Assessment<'a> {
    pub user: UserId,
    pub devices: &'a [DeviceContribution],
    pub usage_bytes: u64,
    /// When the account was registered.
    pub registered_unix: u64,
    /// When the member first went over their entitlement, if they are over.
    ///
    /// Smoothed by the caller over `USAGE_SMOOTHING`: a member who copies a
    /// large folder in and deletes it an hour later has not defaulted on
    /// anything, and a system that reacted within the hour would say they had.
    pub over_since_unix: Option<u64>,
    pub now_unix: u64,
}

/// Work out where a member stands.
#[must_use]
pub fn assess(input: &Assessment<'_>) -> Standing {
    let pledged_bytes = input.devices.iter().fold(0u64, |total, device| {
        total.saturating_add(device.pledged_bytes)
    });

    let effective_bytes = input.devices.iter().fold(0u64, |total, device| {
        total.saturating_add(device.effective_bytes())
    });

    let earned = effective_bytes / CONTRIBUTION_RATIO;

    let joining = input.now_unix.saturating_sub(input.registered_unix) < JOINING_PERIOD_SECONDS;

    // The allowance is a floor, not a bonus: a member who has already pledged
    // enough to earn more than the allowance keeps what they earned.
    let entitlement_bytes = if joining {
        earned.max(JOINING_ALLOWANCE)
    } else {
        earned
    };

    let state = if input.usage_bytes <= entitlement_bytes {
        if joining && earned < JOINING_ALLOWANCE {
            MemberState::Joining
        } else {
            MemberState::Good
        }
    } else {
        match input.over_since_unix {
            // Over, but nobody has recorded since when. Treat it as having
            // started now: sanctions must never be applied retroactively to a
            // period the member had no way to see.
            None => MemberState::Over,
            Some(since) => {
                let over_for = input.now_unix.saturating_sub(since);
                if over_for < GRACE_SECONDS {
                    MemberState::Over
                } else if over_for < DEFAULT_AFTER_SECONDS {
                    MemberState::Grace
                } else {
                    MemberState::Default
                }
            }
        }
    };

    Standing {
        user: input.user,
        pledged_bytes,
        effective_bytes,
        entitlement_bytes,
        usage_bytes: input.usage_bytes,
        has_anchor: input.devices.iter().any(DeviceContribution::is_anchor),
        state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;
    const NOW: u64 = 1_800_000_000;
    /// Registered long enough ago that the joining allowance has expired.
    const ESTABLISHED: u64 = NOW - JOINING_PERIOD_SECONDS - 1;

    fn device(byte: u8, pledged: u64, availability: u16) -> DeviceContribution {
        DeviceContribution {
            device: DeviceId::from_bytes([byte; 32]),
            pledged_bytes: pledged,
            availability_per_mille: availability,
        }
    }

    fn assess_with(devices: &[DeviceContribution], usage: u64) -> Standing {
        assess(&Assessment {
            user: UserId::from_bytes([1; 32]),
            devices,
            usage_bytes: usage,
            registered_unix: ESTABLISHED,
            over_since_unix: None,
            now_unix: NOW,
        })
    }

    #[test]
    fn an_always_on_node_earns_a_third_of_what_it_pledges() {
        // The core of the bargain: pledge three times what you store.
        let standing = assess_with(&[device(1, 300 * GB, 1000)], 0);

        assert_eq!(standing.effective_bytes, 300 * GB);
        assert_eq!(standing.entitlement_bytes, 100 * GB);
    }

    #[test]
    fn a_laptop_earns_a_quarter_of_what_an_always_on_machine_would() {
        // Nicolas's observation, made arithmetic. Bytes on a machine that is
        // usually off are not worth what the number says, and pretending
        // otherwise means the network promises durability it cannot deliver.
        let pledge = 1000 * GB;
        let server = assess_with(&[device(1, pledge, 1000)], 0);
        let laptop = assess_with(&[device(2, pledge, 250)], 0);

        // Same disk offered, a quarter of the uptime, a quarter of the credit.
        assert_eq!(server.effective_bytes, pledge);
        assert_eq!(laptop.effective_bytes, pledge / 4);

        // Entitlement is the credit over the contribution ratio, floored. The
        // two divisions can each lose a byte, so compare within that.
        assert_eq!(server.entitlement_bytes, pledge / CONTRIBUTION_RATIO);
        assert!(
            laptop
                .entitlement_bytes
                .abs_diff(server.entitlement_bytes / 4)
                <= 1,
            "a quarter-uptime laptop earned {} where a quarter of the server's \
             {} was expected",
            laptop.entitlement_bytes,
            server.entitlement_bytes
        );
    }

    #[test]
    fn contributions_from_several_devices_add_up() {
        let standing = assess_with(
            &[
                device(1, 100 * GB, 1000), // an always-on Pi
                device(2, 400 * GB, 250),  // a laptop
            ],
            0,
        );

        assert_eq!(standing.pledged_bytes, 500 * GB);
        assert_eq!(standing.effective_bytes, 100 * GB + 100 * GB);
        assert_eq!(standing.entitlement_bytes, 200 * GB / 3);
    }

    #[test]
    fn availability_has_a_floor_so_a_holiday_is_not_a_default() {
        // A member offline for a month must not come back to find their
        // entitlement collapsed to nothing and themselves in default.
        let absent = assess_with(&[device(1, 1000 * GB, 0)], 0);

        assert!(absent.effective_bytes > 0, "a holiday zeroed the account");
        assert_eq!(
            absent.effective_bytes,
            1000 * GB * u64::from(AVAILABILITY_FLOOR_PER_MILLE) / 1000
        );
    }

    #[test]
    fn availability_above_one_hundred_percent_is_clamped() {
        // A dishonest coordinator publishing 5000‰ must not be able to mint
        // entitlement out of nothing.
        let honest = assess_with(&[device(1, 100 * GB, 1000)], 0);
        let inflated = assess_with(&[device(1, 100 * GB, 60_000)], 0);

        assert_eq!(inflated.entitlement_bytes, honest.entitlement_bytes);
    }

    #[test]
    fn only_a_high_availability_device_counts_as_an_anchor() {
        // Anchors are what make the network readable rather than merely
        // durable. A laptop is not one.
        assert!(assess_with(&[device(1, GB, 1000)], 0).has_anchor);
        assert!(assess_with(&[device(1, GB, ANCHOR_AVAILABILITY_PER_MILLE)], 0).has_anchor);
        assert!(!assess_with(&[device(1, GB, ANCHOR_AVAILABILITY_PER_MILLE - 1)], 0).has_anchor);
        assert!(!assess_with(&[device(1, GB, 250)], 0).has_anchor);
    }

    #[test]
    fn a_new_member_can_store_before_they_have_contributed() {
        // Requiring contribution before storage means nobody can ever start.
        let standing = assess(&Assessment {
            user: UserId::from_bytes([1; 32]),
            devices: &[],
            usage_bytes: 5 * GB,
            registered_unix: NOW - 3600,
            over_since_unix: None,
            now_unix: NOW,
        });

        assert_eq!(standing.entitlement_bytes, JOINING_ALLOWANCE);
        assert_eq!(standing.state, MemberState::Joining);
        assert!(standing.state.may_replicate());
    }

    #[test]
    fn the_joining_allowance_expires() {
        let standing = assess(&Assessment {
            user: UserId::from_bytes([1; 32]),
            devices: &[],
            usage_bytes: 5 * GB,
            registered_unix: NOW - JOINING_PERIOD_SECONDS - 1,
            over_since_unix: Some(NOW - 1),
            now_unix: NOW,
        });

        assert_eq!(standing.entitlement_bytes, 0);
        assert_eq!(standing.state, MemberState::Over);
    }

    #[test]
    fn the_joining_allowance_is_a_floor_not_a_bonus() {
        // A new member who has already pledged generously keeps what they
        // earned rather than being capped at the allowance.
        let standing = assess(&Assessment {
            user: UserId::from_bytes([1; 32]),
            devices: &[device(1, 3000 * GB, 1000)],
            usage_bytes: 0,
            registered_unix: NOW - 3600,
            over_since_unix: None,
            now_unix: NOW,
        });

        assert_eq!(standing.entitlement_bytes, 1000 * GB);
        assert_eq!(standing.state, MemberState::Good);
    }

    #[test]
    fn going_over_escalates_on_a_schedule_and_not_before() {
        let devices = [device(1, 300 * GB, 1000)]; // 100 GB entitlement
        let over = 150 * GB;

        let at = |elapsed: u64| {
            assess(&Assessment {
                user: UserId::from_bytes([1; 32]),
                devices: &devices,
                usage_bytes: over,
                registered_unix: ESTABLISHED,
                over_since_unix: Some(NOW - elapsed),
                now_unix: NOW,
            })
            .state
        };

        assert_eq!(at(0), MemberState::Over);
        assert_eq!(at(GRACE_SECONDS - 1), MemberState::Over);
        assert_eq!(at(GRACE_SECONDS), MemberState::Grace);
        assert_eq!(at(DEFAULT_AFTER_SECONDS - 1), MemberState::Grace);
        assert_eq!(at(DEFAULT_AFTER_SECONDS), MemberState::Default);
    }

    #[test]
    fn sanctions_are_never_applied_retroactively() {
        // A member found to be over, with no record of since when, must start
        // at the mildest state. Anything else punishes them for a period they
        // had no way to observe.
        let standing = assess(&Assessment {
            user: UserId::from_bytes([1; 32]),
            devices: &[],
            usage_bytes: 500 * GB,
            registered_unix: ESTABLISHED,
            over_since_unix: None,
            now_unix: NOW,
        });

        assert_eq!(standing.state, MemberState::Over);
        assert!(standing.state.may_replicate());
        assert!(!standing.state.may_be_reclaimed());
    }

    #[test]
    fn only_default_permits_reclaiming_and_it_still_is_not_deletion_of_everything() {
        // The first principle of §5: the network never deletes a member's data
        // as a punishment. Only the harshest state permits reclaiming at all.
        for state in [
            MemberState::Joining,
            MemberState::Good,
            MemberState::Over,
            MemberState::Grace,
        ] {
            assert!(
                !state.may_be_reclaimed(),
                "{state:?} allowed a host to reclaim a member's data"
            );
        }
        assert!(MemberState::Default.may_be_reclaimed());
    }

    #[test]
    fn grace_stops_new_replication_but_default_is_the_only_destructive_state() {
        assert!(MemberState::Over.may_replicate());
        assert!(!MemberState::Grace.may_replicate());
        assert!(!MemberState::Default.may_replicate());
    }

    #[test]
    fn headroom_and_excess_never_underflow() {
        let under = assess_with(&[device(1, 300 * GB, 1000)], 10 * GB);
        assert_eq!(under.headroom_bytes(), 90 * GB);
        assert_eq!(under.excess_bytes(), 0);

        let over = assess_with(&[device(1, 300 * GB, 1000)], 150 * GB);
        assert_eq!(over.headroom_bytes(), 0);
        assert_eq!(over.excess_bytes(), 50 * GB);
    }

    #[test]
    fn an_enormous_pledge_does_not_overflow_the_arithmetic() {
        // Nobody has 16 exabytes, but a hostile or broken client can claim to.
        let standing = assess_with(&[device(1, u64::MAX, 1000), device(2, u64::MAX, 1000)], 0);

        assert!(standing.pledged_bytes > 0);
        assert!(standing.entitlement_bytes > 0);
    }

    #[test]
    fn entitlement_is_floored_so_a_member_is_never_told_they_have_more_room() {
        // 100 bytes effective over a ratio of 3 is 33, not 34.
        let standing = assess_with(&[device(1, 100, 1000)], 0);
        assert_eq!(standing.entitlement_bytes, 33);
    }

    #[test]
    fn a_member_with_no_devices_earns_nothing_once_established() {
        let standing = assess_with(&[], 0);
        assert_eq!(standing.entitlement_bytes, 0);
        assert_eq!(standing.state, MemberState::Good);
        assert!(!standing.has_anchor);
    }
}
