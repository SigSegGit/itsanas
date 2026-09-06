//! When it is safe to spread chunks around, and when it would do harm.
//!
//! # The two goals are opposed, and which one wins depends on the size
//!
//! **Durability** wants copies: every chunk on several machines, so losing one
//! costs nothing. **Confidentiality and scale** want spread: no single machine
//! holding a whole account, because a complete holder is one broken cipher away
//! from reading it, and because if the unit of hosting is "a whole account"
//! then somebody offering four terabytes needs peers who can each take four
//! terabytes -- which makes the largest contributor the hardest to serve.
//!
//! On a small network you cannot have both. With two peers and a target of two
//! copies, every chunk must go to both of them, so both hold everything. That
//! is not a failure to be fixed; it is the only correct answer available.
//! Spreading anyway would give each chunk one holder instead of two and turn a
//! privacy preference into data loss.
//!
//! So spreading is **off below a threshold and on above it**, and the threshold
//! is derived rather than chosen.
//!
//! # Where the threshold comes from
//!
//! Let `copies` be the number of holders each chunk needs, and let `share` be
//! the largest fraction of one account any single holder should end up with.
//! Each chunk goes to `copies` of the `candidates` holders, so on average a
//! holder receives `copies / candidates` of the account. Keeping that at or
//! below `share` needs
//!
//! ```text
//! candidates >= copies / share
//! ```
//!
//! With three copies and a third as the most one holder should have, that is
//! nine candidates. Below nine, spreading cannot deliver the property it exists
//! for, and switching it on would only cost copies.
//!
//! The bare minimum for "nobody holds everything" is `copies + 1` -- with
//! exactly `copies` candidates, every chunk must go to all of them. That bound
//! is not used here because it is useless in practice: at four candidates and
//! three copies each holder still has three quarters of the account, which is a
//! complete copy in every sense that matters except the arithmetic.
//!
//! # Why this is a module and not a comment
//!
//! Because the dangerous direction is switching it on too early, and a rule
//! that lives in somebody's head gets switched on by somebody else.

/// The largest share of one account a single holder should end up with, as a
/// divisor: three means "no more than a third".
///
/// A third rather than a half because two holders with half each is two people
/// who between them have everything twice, and because the point of spreading
/// is that a subpoena, a seizure or a broken cipher at one holder is not an
/// account.
pub const SHARE_DIVISOR: usize = 3;

/// Whether there are enough candidate holders to spread without losing copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spreading {
    /// Whether to place chunks by rendezvous hashing rather than giving every
    /// holder everything.
    pub enabled: bool,
    /// Candidate holders counted.
    pub candidates: usize,
    /// How many there would have to be.
    pub needed: usize,
}

impl Spreading {
    /// How far off the threshold this network is, or zero when it is past it.
    #[must_use]
    pub const fn short_by(&self) -> usize {
        self.needed.saturating_sub(self.candidates)
    }
}

/// How many candidate holders a network needs before spreading helps.
///
/// See the module documentation: `copies / share`, with `share` expressed as
/// [`SHARE_DIVISOR`].
#[must_use]
pub const fn critical_mass(copies: usize) -> usize {
    copies * SHARE_DIVISOR
}

/// Whether to spread, given how many holders are available.
///
/// Off below [`critical_mass`], and off is the *safe* answer: it means every
/// holder takes everything, which is what a small network needs and what this
/// project did before spreading existed.
///
/// `copies` of zero means nothing is being replicated, and nothing is where
/// spreading would help either.
#[must_use]
pub const fn spreading(candidates: usize, copies: usize) -> Spreading {
    let needed = critical_mass(copies);
    Spreading {
        enabled: copies > 0 && candidates >= needed,
        candidates,
        needed,
    }
}

#[cfg(test)]
mod tests {
    use super::{Spreading, critical_mass, spreading};

    #[test]
    fn a_small_network_never_spreads() {
        // The direction that can do damage. Spreading three copies over two
        // peers means one copy per chunk: a privacy preference turned into data
        // loss, on exactly the networks least able to afford it.
        for candidates in 0..critical_mass(3) {
            assert!(
                !spreading(candidates, 3).enabled,
                "spreading turned on with {candidates} candidates, below the \
                 threshold of {}",
                critical_mass(3)
            );
        }
    }

    #[test]
    fn the_threshold_is_where_a_holder_stops_getting_most_of_the_account() {
        // Nine candidates for three copies: each holder receives a third. The
        // arithmetic is the reason for the number, so it is pinned.
        assert_eq!(critical_mass(3), 9);
        assert_eq!(critical_mass(2), 6);

        assert!(spreading(9, 3).enabled);
        assert!(!spreading(8, 3).enabled);
        assert_eq!(spreading(8, 3).short_by(), 1);
        assert_eq!(spreading(9, 3).short_by(), 0);
    }

    #[test]
    fn replicating_nothing_is_not_a_reason_to_spread() {
        let advice: Spreading = spreading(1000, 0);
        assert!(!advice.enabled);
    }
}
