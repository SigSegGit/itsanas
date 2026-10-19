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
//!   spare: a quarter of the miss budget, and one account more than
//!   [`crate::target::Goal::accounts`] **per own device that may release**.
//!   Own devices decide independently, each from its own roster and its own
//!   measurements, so they do *not* reliably pick the same holder: two of
//!   them may each release a different copy of one chunk in the same hour.
//!   With `n` own devices live, `n` copies may go at once, and only a margin
//!   of `n` accounts above the goal keeps that race at or above it.
//! - **Pick arbitrarily.** It is the lowest-ranked qualifying holder in
//!   rendezvous order, so devices that do share evidence agree, and the
//!   margin covers the ones that do not.
//! - **Judge on soft evidence.** The caller passes only strictly fresh
//!   holdings (an audit passed within `LIVE_FOR`, a record newer than
//!   `REFRESH_AFTER`, no extended liveness window); this module trusts that
//!   and adds nothing looser.

use crate::target::{Goal, Holding, assess};
use itsanas_crypto::DeviceId;

/// The holder to release for this chunk, if any: among off-account holders,
/// the **lowest** in `order` whose removal leaves at least
/// `goal.accounts + releasers` distinct accounts and a miss of at most
/// `goal.miss_ppm / 4`.
///
/// `releasers` is how many of this account's devices may be releasing at
/// once (the live own devices, this one included; at least one): each may
/// take one copy of the chunk, so `releasers` copies may go together.
#[must_use]
pub fn release_holder(
    strict: &[Holding<'_>],
    order: &[DeviceId],
    goal: &Goal,
    releasers: usize,
    now_hour: u64,
) -> Option<DeviceId> {
    let floor = goal.accounts.saturating_add(releasers.max(1));
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
        verdict.accounts >= floor && verdict.miss_ppm <= goal.miss_ppm / 4
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
            network: None,
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
        assert_eq!(
            release_holder(&held, &order, &GOAL, 1, NOW),
            Some(device(2))
        );

        // Three accounts: dropping one leaves two, which meets the goal but
        // not the margin. Nothing is released.
        assert_eq!(release_holder(&held[..3], &order, &GOAL, 1, NOW), None);
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
            release_holder(&with_extra, &order, &GOAL, 1, NOW),
            Some(device(4))
        );
    }

    #[test]
    fn red_team_concurrent_own_devices_never_release_below_the_goal() {
        // Five accounts. Own devices decide on their own rosters, so each may
        // take a different copy in the same hour. With the old fixed margin
        // (goal + 1) four own devices all release and leave one account;
        // scaled by the releasers, any number releasing at once leaves the
        // goal.
        let rings: Vec<Uptime> = (0..5).map(|phase| steady(NOW, 990, phase * 20)).collect();
        let held: Vec<Holding<'_>> = (0..5)
            .map(|index| {
                holding(
                    index + 1,
                    u8::try_from(index + 1).unwrap(),
                    &rings[usize::from(index)],
                )
            })
            .collect();
        let order = [device(1), device(2), device(3), device(4), device(5)];
        assert_eq!(
            release_holder(&held, &order, &GOAL, 2, NOW),
            Some(device(5))
        );
        assert_eq!(release_holder(&held, &order, &GOAL, 3, NOW), None);
        for releasers in 1..=5 {
            if release_holder(&held, &order, &GOAL, releasers, NOW).is_some() {
                assert!(
                    held.len() - releasers >= GOAL.accounts,
                    "{releasers} releasers would leave {} accounts",
                    held.len() - releasers
                );
            }
        }
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
        assert_eq!(
            release_holder(&held, &order, &GOAL, 1, NOW),
            Some(device(4))
        );
        // A device not in the order is not a candidate.
        assert_eq!(release_holder(&held, &[device(7)], &GOAL, 1, NOW), None);
    }
}
