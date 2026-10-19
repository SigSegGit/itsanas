//! How long a host may stay silent before its copies stop counting.
//!
//! A fixed week calls a laptop that comes on every Sunday "lost" every
//! Saturday, and the owner re-sends its share to someone else each week: a
//! transfer storm for nothing. So each host gets a window from its **own**
//! history: twice the longest absence this node has seen it come back from.
//!
//! What this module refuses to do:
//!
//! - **Go below a week.** [`LOST_AFTER_FLOOR`] equals the store's `LIVE_FOR`
//!   (asserted there): measured placement may wait longer than today, never
//!   give up sooner.
//! - **Go past thirteen days.** [`LOST_AFTER_CEILING`] stays under the store's
//!   `CONFIRMED_FOR` (asserted there), so a record still counted is one the
//!   host confirmed recently enough to trust.
//! - **Reward no history.** An unknown host gets the floor, not the ceiling:
//!   a stranger seen once must not buy itself thirteen days of credit.

use crate::uptime::Uptime;

/// The shortest window, in seconds: seven days.
pub const LOST_AFTER_FLOOR: u64 = 7 * 24 * 3600;

/// The longest window, in seconds: thirteen days.
pub const LOST_AFTER_CEILING: u64 = 13 * 24 * 3600;

/// clamp(2 × longest closed gap, floor, ceiling), in seconds. Unknown history
/// gives the floor.
#[must_use]
pub fn lost_after(uptime: Option<&Uptime>) -> u64 {
    uptime.map_or(LOST_AFTER_FLOOR, |ring| {
        ring.longest_gap_hours()
            .saturating_mul(2 * 3600)
            .clamp(LOST_AFTER_FLOOR, LOST_AFTER_CEILING)
    })
}

/// [`lost_after`] in whole hours, as [`crate::uptime::Unit`] takes it.
#[must_use]
pub fn lost_after_hours(uptime: Option<&Uptime>) -> u64 {
    lost_after(uptime) / 3600
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::ring;

    #[test]
    fn the_window_grows_with_a_hosts_own_history_and_never_past_thirteen_days() {
        let always = ring(0, 600, |_| true);
        assert_eq!(lost_after(Some(&always)), LOST_AFTER_FLOOR);

        // Seen once every five days: absences of 119 hours, window 238 hours.
        let five_days = ring(0, 600, |hour| hour % 120 == 0);
        assert_eq!(lost_after(Some(&five_days)), 238 * 3600);
        assert!(lost_after(Some(&five_days)) > LOST_AFTER_FLOOR);

        // Seen once every ten days: twice that is twenty, capped at thirteen.
        let ten_days = ring(0, 600, |hour| hour % 240 == 0);
        assert_eq!(lost_after(Some(&ten_days)), LOST_AFTER_CEILING);
    }

    #[test]
    fn red_team_a_host_with_no_history_gets_the_floor_not_the_ceiling() {
        assert_eq!(lost_after(None), LOST_AFTER_FLOOR);
        let once = ring(0, 1, |_| true);
        assert_eq!(lost_after(Some(&once)), LOST_AFTER_FLOOR);
        assert_eq!(lost_after_hours(None), 7 * 24);
    }
}
