//! Counting escrow fetches per username, in memory, bounded.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use super::{ESCROW_ATTEMPTS, ESCROW_TRACKED, ESCROW_WINDOW};

/// Per-username attempt counting for escrow fetches.
#[derive(Debug)]
pub struct EscrowLimiter {
    attempts: BTreeMap<String, (u32, Instant)>,
    allowed: u32,
    window: Duration,
    capacity: usize,
}

impl EscrowLimiter {
    /// A limiter with the default budget.
    #[must_use]
    pub fn new() -> Self {
        Self::with(ESCROW_ATTEMPTS, ESCROW_WINDOW, ESCROW_TRACKED)
    }

    /// A limiter with an explicit budget, for tests.
    #[must_use]
    pub fn with(allowed: u32, window: Duration, capacity: usize) -> Self {
        Self {
            attempts: BTreeMap::new(),
            allowed,
            window,
            capacity: capacity.max(1),
        }
    }

    /// Whether one more attempt on `username` is allowed at `now`.
    ///
    /// Counts the attempt when it allows it. Taking `now` rather than reading
    /// the clock keeps the window testable without sleeping.
    pub fn allow(&mut self, username: &str, now: Instant) -> bool {
        self.forget_expired(now);

        if let Some((count, since)) = self.attempts.get_mut(username) {
            if now.duration_since(*since) >= self.window {
                *count = 1;
                *since = now;
                return true;
            }
            if *count >= self.allowed {
                return false;
            }
            *count += 1;
            return true;
        }

        // Full of live windows. Refusing a name never seen before is the safe
        // direction: the alternative is evicting somebody else's counter, which
        // is exactly how an attacker would clear their own.
        if self.attempts.len() >= self.capacity {
            return false;
        }
        self.attempts.insert(username.to_owned(), (1, now));
        true
    }

    /// Drop windows that have elapsed.
    fn forget_expired(&mut self, now: Instant) {
        let window = self.window;
        self.attempts
            .retain(|_, (_, since)| now.duration_since(*since) < window);
    }

    /// How many usernames are currently being counted.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.attempts.len()
    }
}

impl Default for EscrowLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> EscrowLimiter {
        EscrowLimiter::with(3, Duration::from_secs(60), 8)
    }

    #[test]
    fn red_team_grinding_one_account_is_cut_off_after_a_few_attempts() {
        // THE ATTACK: the escrow blob is the one thing a stranger can ask for
        // without proving anything, because a machine recovering from nothing
        // has nothing to prove with. Fetch it once and the passphrase can be
        // ground offline forever — so the only defence is to make *asking*
        // expensive, which is the single job a central component does better
        // than a distributed one.
        //
        // If this test fails, the recovery story is a password list away from
        // being an account takeover.
        let mut limiter = limiter();
        let now = Instant::now();

        for attempt in 1..=3 {
            assert!(limiter.allow("nicolas", now), "attempt {attempt} refused");
        }
        assert!(
            !limiter.allow("nicolas", now),
            "a fourth attempt got through"
        );
    }

    #[test]
    fn red_team_flooding_invented_names_cannot_reset_a_real_account_counter() {
        // THE ATTACK: the limiter is itself a table a stranger writes into, one
        // entry per invented name. If a full table evicted the oldest entry to
        // make room, an attacker would spend their budget on a real account,
        // then flood invented names until their own counter was forgotten.
        let mut limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            assert!(limiter.allow("victim", now));
        }
        assert!(!limiter.allow("victim", now));

        for index in 0..500 {
            let _ = limiter.allow(&format!("invented-{index}"), now);
        }

        assert!(
            !limiter.allow("victim", now),
            "the flood cleared the attacker's own counter"
        );
        assert!(limiter.tracked() <= 8, "the limiter grew past its capacity");
    }

    #[test]
    fn the_budget_comes_back_after_the_window() {
        // Somebody who mistypes their passphrase five times must not be locked
        // out of their own account for good.
        let mut limiter = limiter();
        let start = Instant::now();
        for _ in 0..3 {
            assert!(limiter.allow("nicolas", start));
        }
        assert!(!limiter.allow("nicolas", start));

        let later = start + Duration::from_secs(61);
        assert!(limiter.allow("nicolas", later), "the window never reopened");
    }

    #[test]
    fn expired_windows_are_forgotten_so_the_table_does_not_fill_with_history() {
        let mut limiter = limiter();
        let start = Instant::now();
        for index in 0..8 {
            assert!(limiter.allow(&format!("user-{index}"), start));
        }
        assert_eq!(limiter.tracked(), 8);

        assert!(limiter.allow("someone-new", start + Duration::from_secs(61)));
        assert_eq!(limiter.tracked(), 1);
    }

    #[test]
    fn a_name_that_was_never_asked_about_is_allowed_once_the_table_has_room() {
        let mut limiter = EscrowLimiter::with(3, Duration::from_secs(60), 1);
        let now = Instant::now();
        assert!(limiter.allow("first", now));
        assert!(
            !limiter.allow("second", now),
            "a full table must refuse rather than evict"
        );
    }
}
