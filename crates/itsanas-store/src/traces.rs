//! How long the traces of deleted files are kept.
//!
//! A delete leaves a trace (a tombstone) at its path, so a machine that was off
//! when the file went, and comes back holding it, does not bring it back. Kept
//! for ever, the traces grow with everything the account ever deleted; let go,
//! a machine switched off longer than they are kept may bring back a file
//! deleted meanwhile. The account chooses, from [`CHOICES`] or
//! [`Keep::Forever`], and every machine reads the same choice (Nicolas,
//! 2026-11-24: one year by default, set beside the trash's own rules).
//!
//! The choice is an empty file of the account whose path says it, under
//! [`PREFIX`], as the trash's rules are, so a machine that downloads nothing
//! reads it from the log. Two changes made apart leave two such files, and
//! every machine reads the one that keeps more.

use crate::{Result, store::Store, trash::DAY, vault::Vault};

/// The durations offered, in days: ten days to five years.
pub const CHOICES: [u32; 13] = [10, 15, 20, 30, 45, 60, 90, 180, 365, 548, 730, 1095, 1825];

/// What an account that chose nothing keeps: a year.
pub const DEFAULT_DAYS: u32 = 365;

/// Where the account's choice lives.
pub const PREFIX: &str = ".itsanas/traces/";

/// The word for [`Keep::Forever`].
const FOREVER: &str = "never";

/// Before this date (2025-01-01), a delete was dated by a clock that was never
/// set: a Raspberry Pi with no battery says 1970. Its trace would look decades
/// old and go at once, so it is kept, whatever the choice.
pub const CLOCK_FLOOR: u64 = 1_735_689_600;

/// When this machine last let traces go, next to its store.
const SWEPT: &str = "traces-swept";

/// How long traces are kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// This many days after the delete.
    For(u32),
    /// Never let go.
    Forever,
}

impl Default for Keep {
    fn default() -> Self {
        Self::For(DEFAULT_DAYS)
    }
}

impl Keep {
    /// As one path component: a number of days, or `never`.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            Self::For(days) => days.to_string(),
            Self::Forever => FOREVER.to_owned(),
        }
    }

    /// Read [`Self::word`]; `None` for anything else, or for zero days.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        if word == FOREVER {
            return Some(Self::Forever);
        }
        word.parse::<u32>()
            .ok()
            .filter(|days| *days > 0)
            .map(Self::For)
    }

    /// Larger keeps more.
    const fn protection(self) -> u64 {
        match self {
            Self::For(days) => days as u64,
            Self::Forever => u64::MAX,
        }
    }

    /// Whether going from `self` to `next` keeps less: the person says who
    /// they are first, as for the trash's own rules.
    #[must_use]
    pub const fn loosened_by(self, next: Self) -> bool {
        next.protection() < self.protection()
    }

    /// The date before which a trace may go, at `now`; `None` when none may.
    #[must_use]
    pub const fn cutoff(self, now: u64) -> Option<u64> {
        match self {
            Self::For(days) => Some(now.saturating_sub((days as u64).saturating_mul(DAY))),
            Self::Forever => None,
        }
    }
}

/// The choice among the account's files `listed`: the one that keeps more.
fn under(listed: &[String]) -> Keep {
    listed
        .iter()
        .filter_map(|path| Keep::from_word(path.strip_prefix(PREFIX)?))
        .max_by_key(|keep| keep.protection())
        .unwrap_or_default()
}

/// The account's choice.
///
/// # Errors
///
/// When the store or the vault cannot be read.
pub fn account(store: &Store, vault: &Vault) -> Result<Keep> {
    Ok(under(&crate::catalogue::reserved(store, vault)?))
}

/// The account's choice, from this machine's index alone.
///
/// # Errors
///
/// When the store cannot be read.
pub fn known_here(store: &Store) -> Result<Keep> {
    Ok(under(&store.list()?))
}

/// [`set`], from the index alone: for a machine that takes in the account,
/// where every older choice is in the index.
///
/// # Errors
///
/// When the store cannot be written.
pub fn set_known(store: &Store, keep: Keep) -> Result<()> {
    for path in store.list()?.iter().filter(|path| path.starts_with(PREFIX)) {
        store.remove_file(path)?;
    }
    store.write_file(&format!("{PREFIX}{}", keep.word()), b"")?;
    store.flush_segment()?;
    Ok(())
}

/// Make `keep` the account's only choice.
///
/// # Errors
///
/// When the store cannot be written.
pub fn set(store: &Store, vault: &Vault, keep: Keep) -> Result<()> {
    for path in crate::catalogue::reserved(store, vault)?
        .iter()
        .filter(|path| path.starts_with(PREFIX))
    {
        crate::remove_known(store, vault, path)?;
    }
    store.write_file(&format!("{PREFIX}{}", keep.word()), b"")?;
    store.flush_segment()?;
    Ok(())
}

/// Let go of the traces older than the account keeps, at most once a day.
/// How many went.
///
/// # Errors
///
/// When the store cannot be read or written.
pub fn sweep(store: &Store, vault: &Vault, now: u64) -> Result<usize> {
    let swept = store.root().join(SWEPT);
    let last = std::fs::read_to_string(&swept)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok());
    if last.is_some_and(|last| last <= now && now - last < DAY) {
        return Ok(0);
    }
    let gone = match account(store, vault)?.cutoff(now) {
        Some(cutoff) => store.forget_traces_before(cutoff)?,
        None => 0,
    };
    std::fs::write(&swept, now.to_string())
        .map_err(|error| crate::error::StoreError::io(SWEPT, error))?;
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_reads_back_and_the_default_is_a_year() {
        for days in CHOICES {
            assert_eq!(
                Keep::from_word(&Keep::For(days).word()),
                Some(Keep::For(days))
            );
        }
        assert_eq!(Keep::from_word("never"), Some(Keep::Forever));
        assert_eq!(
            Keep::from_word("0"),
            None,
            "zero days would let every trace go at once"
        );
        assert_eq!(Keep::default(), Keep::For(365));
        assert!(
            CHOICES.contains(&DEFAULT_DAYS),
            "the default is not one the page offers"
        );
    }

    /// Two machines that chose apart leave two choices; both read the one
    /// that keeps more, and a shorter one is a loosening.
    #[test]
    fn red_team_two_choices_read_as_the_one_that_keeps_more() {
        let listed = vec![
            format!("{PREFIX}30"),
            format!("{PREFIX}{FOREVER}"),
            format!("{PREFIX}365"),
            "photos/a.jpg".to_owned(),
        ];
        assert_eq!(
            under(&listed),
            Keep::Forever,
            "a shorter choice won over keeping forever"
        );
        assert_eq!(under(&listed[..1]), Keep::For(30));
        assert_eq!(under(&[]), Keep::For(365), "no choice is not the default");
        assert!(Keep::Forever.loosened_by(Keep::For(1825)));
        assert!(Keep::For(365).loosened_by(Keep::For(10)));
        assert!(!Keep::For(10).loosened_by(Keep::Forever));
    }
}
