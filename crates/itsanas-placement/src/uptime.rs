//! How often a device was there when this node looked, measured by this node.
//!
//! One ring of hourly bits per device: the hours this node *tried* to reach it
//! (or was reached by it), and among those, the hours it was seen. Availability
//! is the second count over the first.
//!
//! What this module refuses to do:
//!
//! - **Count an hour nobody looked.** A laptop that slept all night observed
//!   nothing; marking every host "absent" for those hours would make a host
//!   that never misses a session look like it is off a third of the time. Only
//!   attempted hours are in the denominator.
//! - **Trust a figure built from too little.** No availability before
//!   [`MIN_SPAN_HOURS`] of history and [`MIN_ATTEMPTED_HOURS`] attempts: a host
//!   seen in its first three hours would otherwise read as 100 % and attract
//!   copies it has not earned.
//! - **Let the clock rewrite history.** A clock that steps back is ignored
//!   outright; only a forward jump past the whole window starts the ring over.
//! - **Use floating point.** Everything is ‰ and ppm in integers, so two of a
//!   user's devices given the same rings reach the same verdict bit for bit.
//! - **Read anyone else's figures.** The rings are fed only by this node's own
//!   sessions; nothing here takes a coordinator's or a peer's report.

use serde::{Deserialize, Serialize};

/// Hours the ring remembers: 28 days.
pub const WINDOW_HOURS: u64 = 672;

/// History needed before a figure exists: a week, so a weekly rhythm (the
/// machine off every weekend) is in it at least once.
pub const MIN_SPAN_HOURS: u64 = 7 * 24;

/// Attempted hours needed before a figure exists, for the same reason.
pub const MIN_ATTEMPTED_HOURS: u64 = 24;

/// Thousandths: 1000 is always there.
pub type PerMille = u16;

/// Words per ring: 11 × 64 = 704 bits, enough for [`WINDOW_HOURS`].
const WORDS: usize = 11;

const PPM: u64 = 1_000_000;

/// One device's hourly record, as this node saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uptime {
    /// The first hour of the record (moved forward by a reset).
    first_hour: u64,
    /// The latest hour noted; the ring covers the window ending here.
    last_hour: u64,
    /// Hours this node tried, bit `hour % WINDOW_HOURS`.
    attempted: [u64; WORDS],
    /// Hours it was seen; always a subset of `attempted`.
    seen: [u64; WORDS],
}

fn bit(hour: u64) -> (usize, u64) {
    let position = hour % WINDOW_HOURS;
    // position < 672, so the word index is < 11 and fits any usize.
    let word = usize::try_from(position / 64).unwrap_or(0);
    (word, 1u64 << (position % 64))
}

impl Uptime {
    /// An empty record starting at `hour`.
    #[must_use]
    pub const fn new(hour: u64) -> Self {
        Self {
            first_hour: hour,
            last_hour: hour,
            attempted: [0; WORDS],
            seen: [0; WORDS],
        }
    }

    /// One observation: this node tried to reach the device (or it reached us)
    /// in `hour`, and did or did not see it. Returns whether anything changed,
    /// so callers write only then.
    ///
    /// A clock that steps **back** is ignored entirely: resetting on it would
    /// let one bad NTP answer erase four weeks of evidence and make every host
    /// "unknown" — hence unavailable — at once, which is a re-send storm. Only
    /// a forward jump past [`WINDOW_HOURS`] resets, because then every bit in
    /// the ring is older than the window anyway.
    pub fn note(&mut self, hour: u64, seen: bool) -> bool {
        if hour < self.last_hour {
            return false;
        }

        let mut changed = false;
        if hour - self.last_hour >= WINDOW_HOURS {
            *self = Self::new(hour);
            changed = true;
        } else {
            // The positions between the last note and this one still hold
            // bits from a window ago: clear them, or a host seen 28 days ago
            // would read as seen yesterday. At most WINDOW_HOURS steps.
            for stale in self.last_hour + 1..=hour {
                let (word, mask) = bit(stale);
                if self.attempted[word] & mask != 0 {
                    self.attempted[word] &= !mask;
                    self.seen[word] &= !mask;
                    changed = true;
                }
            }
            if hour != self.last_hour {
                self.last_hour = hour;
                changed = true;
            }
        }

        let (word, mask) = bit(hour);
        if self.attempted[word] & mask == 0 {
            self.attempted[word] |= mask;
            changed = true;
        }
        if seen && self.seen[word] & mask == 0 {
            self.seen[word] |= mask;
            changed = true;
        }
        changed
    }

    /// The first hour of the record.
    #[must_use]
    pub const fn first_hour(&self) -> u64 {
        self.first_hour
    }

    /// The latest hour noted.
    #[must_use]
    pub const fn last_hour(&self) -> u64 {
        self.last_hour
    }

    /// The oldest hour still inside the ring.
    const fn window_start(&self) -> u64 {
        let start = self.last_hour.saturating_sub(WINDOW_HOURS - 1);
        if start > self.first_hour {
            start
        } else {
            self.first_hour
        }
    }

    /// Whether `hour` is inside this ring's window and was attempted.
    fn attempted_at(&self, hour: u64) -> bool {
        if hour < self.window_start() || hour > self.last_hour {
            return false;
        }
        let (word, mask) = bit(hour);
        self.attempted[word] & mask != 0
    }

    /// Whether `hour` is inside this ring's window and the device was seen.
    fn seen_at(&self, hour: u64) -> bool {
        if hour < self.window_start() || hour > self.last_hour {
            return false;
        }
        let (word, mask) = bit(hour);
        self.seen[word] & mask != 0
    }

    /// Hours attempted inside the window.
    #[must_use]
    pub fn attempted_hours(&self) -> u64 {
        self.attempted
            .iter()
            .map(|word| u64::from(word.count_ones()))
            .sum()
    }

    /// Hours seen inside the window.
    #[must_use]
    pub fn seen_hours(&self) -> u64 {
        self.seen
            .iter()
            .map(|word| u64::from(word.count_ones()))
            .sum()
    }

    /// The latest hour the device was seen, if the window holds one.
    #[must_use]
    pub fn last_seen_hour(&self) -> Option<u64> {
        (self.window_start()..=self.last_hour)
            .rev()
            .find(|hour| self.seen_at(*hour))
    }

    /// Longest closed gap between two seen hours inside the window, in hours.
    ///
    /// "Closed": an absence still running is not a gap yet, it is what
    /// [`Uptime::availability`] reads against the liveness window.
    #[must_use]
    pub fn longest_gap_hours(&self) -> u64 {
        let mut previous: Option<u64> = None;
        let mut longest = 0;
        for hour in self.window_start()..=self.last_hour {
            if self.seen_at(hour) {
                if let Some(before) = previous {
                    longest = longest.max(hour - before - 1);
                }
                previous = Some(hour);
            }
        }
        longest
    }

    /// Seen over attempted, in ‰.
    ///
    /// `None` until [`MIN_SPAN_HOURS`] since the first hour and
    /// [`MIN_ATTEMPTED_HOURS`] attempted. `Some(0)` when the current absence
    /// (`now_hour` − last seen) exceeds `lost_after_hours`: a host that has
    /// been gone longer than its own window is presumed lost, whatever its
    /// past average says, or a dead host with a fine record would keep
    /// counting as a copy.
    #[must_use]
    pub fn availability(&self, now_hour: u64, lost_after_hours: u64) -> Option<PerMille> {
        if now_hour.saturating_sub(self.first_hour) < MIN_SPAN_HOURS {
            return None;
        }
        let attempted = self.attempted_hours();
        if attempted < MIN_ATTEMPTED_HOURS {
            return None;
        }
        match self.last_seen_hour() {
            Some(last) if now_hour.saturating_sub(last) <= lost_after_hours => {}
            _ => return Some(0),
        }
        let share = self.seen_hours() * 1000 / attempted;
        Some(PerMille::try_from(share).unwrap_or(1000))
    }

    /// Several devices as one unit (an account's own machines): attempted
    /// OR, seen OR, first hour min, last hour max. `None` for no rings.
    #[must_use]
    pub fn union(rings: &[&Self]) -> Option<Self> {
        let first = rings.iter().map(|ring| ring.first_hour).min()?;
        let last = rings.iter().map(|ring| ring.last_hour).max()?;
        let mut joined = Self::new(last);
        joined.first_hour = first;
        for hour in joined.window_start()..=last {
            let (word, mask) = bit(hour);
            if rings.iter().any(|ring| ring.attempted_at(hour)) {
                joined.attempted[word] |= mask;
            }
            if rings.iter().any(|ring| ring.seen_at(hour)) {
                joined.seen[word] |= mask;
            }
        }
        Some(joined)
    }
}

/// One independent thing that may or may not be reachable: a remote holder,
/// or all of this account's other devices as one.
#[derive(Clone, Copy, Debug)]
pub struct Unit<'a> {
    /// Its record; `None` is unknown, which counts as never there.
    pub uptime: Option<&'a Uptime>,
    /// Its liveness window, in hours (see [`crate::liveness`]).
    pub lost_after_hours: u64,
}

/// P(none of `units` reachable) in ppm: the **worse** of the independent
/// product Π(1 − aᵢ) and the empirical share of hours, attempted for all of
/// them, where none was seen (empirical only with ≥ [`MIN_ATTEMPTED_HOURS`]
/// such hours). A unit with unknown availability counts as 0.
///
/// The product alone is what the textbook gives and what fails in practice:
/// three hosts each up 70 % of the time but all switched off at night are
/// never all three available at 3 a.m., and the product would call them
/// 97 % safe. The empirical figure catches that, and taking the worse means
/// correlation can only ever ask for more copies.
#[must_use]
pub fn miss_ppm(units: &[Unit<'_>], now_hour: u64) -> u32 {
    let mut independent = PPM;
    for unit in units {
        let available = unit
            .uptime
            .and_then(|ring| ring.availability(now_hour, unit.lost_after_hours))
            .map_or(0, u64::from);
        independent = independent * (PPM - available.min(1000) * 1000) / PPM;
    }

    let rings: Option<Vec<&Uptime>> = units.iter().map(|unit| unit.uptime).collect();
    let empirical = match rings {
        Some(rings) if !rings.is_empty() => {
            let start = now_hour.saturating_sub(WINDOW_HOURS - 1);
            let mut joint = 0u64;
            let mut missed = 0u64;
            for hour in start..=now_hour {
                if rings.iter().all(|ring| ring.attempted_at(hour)) {
                    joint += 1;
                    if !rings.iter().any(|ring| ring.seen_at(hour)) {
                        missed += 1;
                    }
                }
            }
            (joint >= MIN_ATTEMPTED_HOURS).then(|| missed * PPM / joint)
        }
        _ => None,
    };

    let worse = empirical.map_or(independent, |figure| figure.max(independent));
    u32::try_from(worse).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::ring;

    const NOW: u64 = 10_000;
    const LOOSE: u64 = 13 * 24;

    #[test]
    fn hours_this_node_did_not_try_are_not_counted_against_a_device() {
        // This node runs only half of each day; the host is always seen when
        // looked for. It must read 100 %, not 50 %.
        let mut uptime = Uptime::new(NOW - 400);
        for hour in NOW - 400..NOW {
            if hour % 24 < 12 {
                assert!(uptime.note(hour, true));
            }
        }
        assert_eq!(uptime.attempted_hours(), 204);
        assert_eq!(uptime.availability(NOW, LOOSE), Some(1000));
    }

    #[test]
    fn noting_the_same_observation_twice_changes_nothing() {
        let mut uptime = Uptime::new(NOW);
        assert!(uptime.note(NOW, false));
        assert!(!uptime.note(NOW, false));
        assert!(uptime.note(NOW, true), "a sighting upgrades the hour");
        assert!(
            !uptime.note(NOW, false),
            "a later miss in the same hour does not undo it"
        );
        assert_eq!(uptime.seen_hours(), 1);
    }

    #[test]
    fn a_clock_stepping_back_never_erases_the_history() {
        let mut uptime = ring(NOW - 300, 300, |_| true);
        let before = uptime.clone();
        assert!(!uptime.note(NOW - 5_000, false), "a step back is ignored");
        assert!(!uptime.note(3, true));
        assert_eq!(uptime, before);
        assert_eq!(uptime.availability(NOW, LOOSE), Some(1000));
    }

    #[test]
    fn a_forward_jump_past_the_window_starts_over() {
        let mut uptime = ring(NOW - 300, 300, |_| true);
        let later = NOW + WINDOW_HOURS + 10;
        assert!(uptime.note(later, false));
        assert_eq!(uptime.attempted_hours(), 1);
        assert_eq!(uptime.seen_hours(), 0);
        assert_eq!(uptime.first_hour(), later);
        assert_eq!(uptime.availability(later, LOOSE), None, "too young again");
    }

    #[test]
    fn hours_older_than_the_window_fall_out_of_the_ring() {
        // Seen for the first 100 hours, then looked for and never seen: after
        // a full window only misses remain.
        let mut uptime = ring(0, 100, |_| true);
        for hour in 100..100 + WINDOW_HOURS {
            uptime.note(hour, false);
        }
        assert_eq!(uptime.seen_hours(), 0);
        assert_eq!(uptime.attempted_hours(), WINDOW_HOURS);
    }

    #[test]
    fn a_figure_needs_a_week_and_a_day_of_attempts() {
        let young = ring(NOW - 100, 100, |_| true);
        assert_eq!(young.availability(NOW, LOOSE), None, "under a week");
        let mut sparse = Uptime::new(NOW - 400);
        for hour in (NOW - 400..NOW).step_by(40) {
            sparse.note(hour, true);
        }
        assert_eq!(sparse.availability(NOW, LOOSE), None, "ten attempts only");
    }

    #[test]
    fn a_current_absence_past_the_window_reads_as_zero() {
        // A perfect record up to 200 hours ago, then nothing seen.
        let mut uptime = ring(NOW - 600, 400, |_| true);
        for hour in NOW - 200..NOW {
            uptime.note(hour, false);
        }
        assert_eq!(uptime.availability(NOW, 7 * 24), Some(0));
        assert_eq!(uptime.availability(NOW, 13 * 24), Some(666));
    }

    #[test]
    fn the_longest_gap_is_between_two_sightings() {
        let uptime = ring(NOW - 300, 300, |hour| {
            hour == NOW - 300 || hour == NOW - 250 || hour == NOW - 240
        });
        assert_eq!(uptime.longest_gap_hours(), 49);
        assert_eq!(uptime.last_seen_hour(), Some(NOW - 240));
    }

    #[test]
    fn unknown_availability_counts_as_unreachable() {
        let young = ring(NOW - 10, 10, |_| true);
        let units = [
            Unit {
                uptime: None,
                lost_after_hours: LOOSE,
            },
            Unit {
                uptime: Some(&young),
                lost_after_hours: LOOSE,
            },
        ];
        assert_eq!(miss_ppm(&units, NOW), 1_000_000);
        assert_eq!(
            miss_ppm(&[], NOW),
            1_000_000,
            "no holder at all is a sure miss"
        );
    }

    #[test]
    fn correlated_hosts_need_more_copies_than_independent_ones_with_the_same_averages() {
        fn unit(ring: &Uptime) -> Unit<'_> {
            Unit {
                uptime: Some(ring),
                lost_after_hours: LOOSE,
            }
        }

        // Both pairs are up half the time. The correlated pair is off at the
        // same hours; the independent-looking pair alternates.
        let start = NOW - 600;
        let night = |hour: u64| hour % 24 >= 12;
        let correlated_a = ring(start, 600, night);
        let correlated_b = ring(start, 600, night);
        let alternate = ring(start, 600, |hour| hour % 24 < 12);

        let correlated = miss_ppm(&[unit(&correlated_a), unit(&correlated_b)], NOW);
        let independent = miss_ppm(&[unit(&correlated_a), unit(&alternate)], NOW);

        assert_eq!(independent, 250_000, "the product of two halves");
        assert_eq!(correlated, 500_000, "the empirical figure wins when worse");
        assert!(correlated > independent);
    }

    #[test]
    fn a_union_sees_what_any_member_saw() {
        let start = NOW - 400;
        let day = ring(start, 400, |hour| hour % 24 < 12);
        let night = ring(start, 400, |hour| hour % 24 >= 12);
        let both = Uptime::union(&[&day, &night]).expect("two rings");
        assert_eq!(both.availability(NOW, LOOSE), Some(1000));
        assert_eq!(both.first_hour(), start);
        assert!(Uptime::union(&[]).is_none());
    }
}
