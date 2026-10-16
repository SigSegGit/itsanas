//! Which remote copy a full host may drop, when one may be dropped at all.
//!
//! Placement only adds copies. The one exception is a host that refused new
//! chunks of this account because the account's share of it is full: then it
//! may evict *this account's* chunks there to make room for this account's
//! other chunks. This module picks which.
//!
//! What this module refuses to do:
//!
//! - **Release below margin.** What stays must clear the goal with room to
//!   spare: one account more than [`crate::target::Goal::accounts`] and a
//!   quarter of the miss budget. Two own devices may release two copies of
//!   one chunk on two hosts in the same hour; the margin is what keeps that
//!   race above the goal instead of under it.
//! - **Let own devices disagree on the choice.** It is always the
//!   lowest-ranked qualifying holder in rendezvous order, which every own
//!   device computes the same from the same evidence.
//! - **Judge on soft evidence.** The caller passes only strictly fresh
//!   holdings (proven within `LIVE_FOR`, recently confirmed, no extended
//!   liveness window); this module trusts that and adds nothing looser.

use crate::target::{Goal, Holding, assess};
use itsanas_crypto::DeviceId;

/// The holder to release for this chunk, if any: among off-account holders,
/// the **lowest** in `order` whose removal leaves at least `goal.accounts + 1`
/// distinct accounts and a miss of at most `goal.miss_ppm / 4`.
#[must_use]
pub fn release_holder(
    strict: &[Holding<'_>],
    order: &[DeviceId],
    goal: &Goal,
    now_hour: u64,
) -> Option<DeviceId> {
    order.iter().rev().copied().find(|candidate| {
        if !strict
            .iter()
            .any(|holding| !holding.own && holding.device == *candidate)
        {
            return false;
        }
        let remaining: Vec<Holding<'_>> = strict
            .iter()
            .filter(|holding| holding.device != *candidate)
            .copied()
            .collect();
        let verdict = assess(&remaining, goal, now_hour);
        verdict.accounts > goal.accounts && verdict.miss_ppm <= goal.miss_ppm / 4
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{device, steady, user};
    use crate::target::{Account, GOAL};
    use crate::uptime::Uptime;

    const NOW: u64 = 10_000;

    fn holding(index: u16, account: u8, uptime: &Uptime) -> Holding<'_> {
        Holding {
            device: device(index),
            account: Account::Known(user(account)),
            own: false,
            uptime: Some(uptime),
            proven: true,
            recent: true,
        }
    }

    #[test]
    fn release_picks_the_lowest_ranked_holder_and_only_with_margin() {
        let rings: Vec<Uptime> = (0..4).map(|phase| steady(NOW, 990, phase * 25)).collect();
        let held: Vec<Holding<'_>> = (0..4)
            .map(|index| {
                holding(
                    index + 1,
                    u8::try_from(index + 1).unwrap(),
                    &rings[usize::from(index)],
                )
            })
            .collect();
        let order = [device(3), device(1), device(4), device(2)];

        // Four accounts: dropping any one leaves three, well inside 2 500 ppm.
        assert_eq!(release_holder(&held, &order, &GOAL, NOW), Some(device(2)));

        // Three accounts: dropping one leaves two, which meets the goal but
        // not the margin. Nothing is released.
        assert_eq!(release_holder(&held[..3], &order, &GOAL, NOW), None);
    }

    #[test]
    fn red_team_a_holder_whose_loss_breaks_the_margin_is_skipped_for_the_next() {
        // The lowest-ranked holder is the only copy of its account: taking it
        // would leave two accounts, below margin. The next one up merely
        // duplicates an account and is the one released.
        let good: Vec<Uptime> = (0..4).map(|phase| steady(NOW, 990, phase * 25)).collect();
        let held = [
            holding(1, 1, &good[0]),
            holding(2, 2, &good[1]),
            holding(3, 3, &good[2]),
        ];
        // An extra copy on account 1 that adds availability but no account.
        let extra = steady(NOW, 990, 75);
        let mut with_extra = held.to_vec();
        with_extra.push(holding(4, 1, &extra));

        let order = [device(1), device(2), device(4), device(3)];
        assert_eq!(
            release_holder(&with_extra, &order, &GOAL, NOW),
            Some(device(4))
        );
    }

    #[test]
    fn red_team_own_devices_and_unlisted_holders_are_never_released() {
        let rings: Vec<Uptime> = (0..5).map(|phase| steady(NOW, 990, phase * 20)).collect();
        let mut held: Vec<Holding<'_>> = (0..4)
            .map(|index| {
                holding(
                    index + 1,
                    u8::try_from(index + 1).unwrap(),
                    &rings[usize::from(index)],
                )
            })
            .collect();
        held.push(Holding {
            own: true,
            ..holding(5, 9, &rings[4])
        });
        // The own device is ranked lowest; it must be passed over.
        let order = [device(1), device(2), device(3), device(4), device(5)];
        assert_eq!(release_holder(&held, &order, &GOAL, NOW), Some(device(4)));
        // A device not in the order is not a candidate.
        assert_eq!(release_holder(&held, &[device(7)], &GOAL, NOW), None);
    }
}
