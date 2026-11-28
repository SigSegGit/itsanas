//! How available the hosts of this account's copies should be: a member's
//! choice, read as an **order**, never as an exclusion.
//!
//! The hosts that meet the chosen level are tried first; the others follow,
//! so a demanding level never leaves a chunk with fewer copies than the goal
//! asks. What a level cannot have it is told about, it does not lose data for.
//!
//! The level is this node's own measure ([`crate::Uptime`]): hourly, over
//! [`crate::WINDOW_HOURS`] (four weeks). That resolution cannot tell 99.9 %
//! from 99.99 %: one missed hour in four weeks is already 99.85 %. So the
//! top measured level is "never missed in the window", and nothing above it
//! is offered until something measures finer.

use crate::liveness::lost_after_hours;
use crate::target::Holding;
use crate::uptime::PerMille;

/// A level of availability a host must have been measured at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostTier {
    /// Any host that passes its audits, in the order placement already uses.
    #[default]
    Cheapest,
    /// Hosts measured at least this, per mille, first.
    AtLeast(PerMille),
    /// Every host by its measured availability, the best first.
    Best,
}

/// The levels a member is offered, with their word in the configuration.
pub const TIERS: [(HostTier, &str); 6] = [
    (HostTier::Cheapest, "cheapest"),
    (HostTier::AtLeast(900), "90"),
    (HostTier::AtLeast(950), "95"),
    (HostTier::AtLeast(990), "99"),
    (HostTier::AtLeast(999), "99.9"),
    (HostTier::Best, "best"),
];

impl HostTier {
    /// The word for this level in the configuration.
    #[must_use]
    pub fn name(self) -> &'static str {
        TIERS
            .iter()
            .find(|(tier, _)| *tier == self)
            .map_or("cheapest", |(_, word)| word)
    }

    /// The level a word names, `None` for one this build does not offer.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        let word = word.trim().trim_end_matches('%');
        TIERS
            .iter()
            .find(|(_, name)| *name == word)
            .map(|(tier, _)| *tier)
    }

    /// Whether `holding` meets this level by this node's measure at
    /// `now_hour`. A host with too little history meets only
    /// [`HostTier::Cheapest`]: a stranger seen for an hour is not 100 %.
    #[must_use]
    pub fn met_by(self, holding: &Holding<'_>, now_hour: u64) -> bool {
        match self {
            Self::Cheapest => true,
            Self::AtLeast(floor) => measured(holding, now_hour).is_some_and(|got| got >= floor),
            Self::Best => measured(holding, now_hour).is_some(),
        }
    }

    /// Put `ranked` (candidates, in placement's order) in this level's order:
    /// the hosts that meet it first, keeping their order among themselves
    /// (the best first, for [`HostTier::Best`]), then the rest as they were.
    pub fn order(self, ranked: &mut [Holding<'_>], now_hour: u64) {
        match self {
            Self::Cheapest => {}
            Self::AtLeast(_) => {
                ranked.sort_by_key(|holding| !self.met_by(holding, now_hour));
            }
            Self::Best => {
                ranked.sort_by_key(|holding| {
                    std::cmp::Reverse(measured(holding, now_hour).map_or(-1, i32::from))
                });
            }
        }
    }
}

/// `holding`'s availability as this node measured it, if it has enough
/// history to have one.
fn measured(holding: &Holding<'_>, now_hour: u64) -> Option<PerMille> {
    let uptime = holding.uptime?;
    uptime.availability(now_hour, lost_after_hours(Some(uptime)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{device, steady};
    use crate::target::Account;

    const NOW: u64 = 10_000;

    fn host(index: u16, uptime: Option<&crate::Uptime>) -> Holding<'_> {
        Holding {
            device: device(index),
            account: Account::Unknown,
            own: false,
            uptime,
            proven: true,
            recent: true,
            network: None,
        }
    }

    #[test]
    fn red_team_a_demanding_level_puts_the_hosts_that_meet_it_first_and_keeps_the_others() {
        // Ranked: a host measured at 50 %, one at 99.9 %, one with no
        // history, one at 96 %. Asking 95 %: the two that meet it come first,
        // in their own order, and the two that do not stay in the list, so a
        // chunk can still reach its goal through them.
        let half = steady(NOW, 500, 0);
        let always = steady(NOW, 1000, 0);
        let mostly = steady(NOW, 960, 0);
        let mut ranked = vec![
            host(1, Some(&half)),
            host(2, Some(&always)),
            host(3, None),
            host(4, Some(&mostly)),
        ];
        HostTier::AtLeast(950).order(&mut ranked, NOW);
        let order: Vec<_> = ranked.iter().map(|h| h.device).collect();
        assert_eq!(order, vec![device(2), device(4), device(1), device(3)]);
    }

    #[test]
    fn red_team_a_stranger_with_no_history_meets_no_measured_level() {
        let new = host(1, None);
        for (tier, _) in TIERS {
            assert_eq!(
                tier.met_by(&new, NOW),
                tier == HostTier::Cheapest,
                "{tier:?}"
            );
        }
    }

    #[test]
    fn the_best_level_orders_every_host_by_its_measure() {
        let half = steady(NOW, 500, 0);
        let always = steady(NOW, 1000, 0);
        let mostly = steady(NOW, 900, 0);
        let mut ranked = vec![
            host(1, Some(&half)),
            host(2, None),
            host(3, Some(&always)),
            host(4, Some(&mostly)),
        ];
        HostTier::Best.order(&mut ranked, NOW);
        let order: Vec<_> = ranked.iter().map(|h| h.device).collect();
        assert_eq!(order, vec![device(3), device(4), device(1), device(2)]);
    }

    #[test]
    fn the_cheapest_level_leaves_placements_order_alone() {
        let half = steady(NOW, 500, 0);
        let always = steady(NOW, 1000, 0);
        let mut ranked = vec![host(1, Some(&half)), host(2, Some(&always))];
        HostTier::Cheapest.order(&mut ranked, NOW);
        assert_eq!(ranked[0].device, device(1));
    }

    #[test]
    fn every_level_is_written_and_read_back_as_its_word() {
        for (tier, word) in TIERS {
            assert_eq!(tier.name(), word);
            assert_eq!(HostTier::parse(word), Some(tier));
        }
        assert_eq!(HostTier::parse("99.9%"), Some(HostTier::AtLeast(999)));
        assert_eq!(
            HostTier::parse("99.99"),
            None,
            "a level nothing here can measure"
        );
    }
}
