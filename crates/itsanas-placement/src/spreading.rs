//! When it is safe to spread chunks around, and when it would do harm.
//!
//! # The two goals are opposed, and which one wins depends on the size
//!
//! **Durability** wants copies: every chunk on several machines, so losing one
//! costs nothing. **Confidentiality and scale** want spread: no single machine
//! holding a whole account, because a complete holder is one broken cipher away
//! from reading it, and because if the unit of hosting is "a whole account"
//! then somebody offering four terabytes needs peers who can each take four
//! terabytes -- which makes the largest contributor the hardest to serve and
//! breaks "offer storage to earn storage" exactly at the top.
//!
//! On a small network you cannot have both. With two peers and a target of two
//! copies, every chunk must go to both of them, so both hold everything. That
//! is not a failure to be fixed; it is the only correct answer available.
//! Spreading anyway would give each chunk one holder instead of two and turn a
//! privacy preference into data loss.
//!
//! So spreading is **off below a threshold and on above it**, and the threshold
//! has two halves.
//!
//! # Half one: enough holders
//!
//! Let `copies` be the number of holders each chunk needs, and let `share` be
//! the largest fraction of one account any single holder should end up with.
//! Each chunk goes to `copies` of the `candidates` holders, so a holder
//! receives `copies / candidates` of the account on average, and keeping that
//! at or below `share` needs
//!
//! ```text
//! candidates >= copies / share
//! ```
//!
//! With three copies and a third as the most one holder should have, that is
//! nine candidates.
//!
//! The bare minimum for "nobody holds everything" is `copies + 1` -- with
//! exactly `copies` candidates every chunk must go to all of them. That bound
//! is not used: at four candidates and three copies each holder still has three
//! quarters of the account, which is a complete copy in every sense except the
//! arithmetic.
//!
//! **`share` is a choice, not a derivation.** A third rather than a half
//! because two holders with half each are two people who between them have
//! everything twice. There is no discontinuity in the security at a third; the
//! number is picked, and the count follows from it. Saying otherwise would
//! dress an arbitrary figure in a division.
//!
//! # Half two: enough room, which a count cannot see
//!
//! Nine peers offering a gigabyte each are nine peers. They cannot hold four
//! terabytes three times over, and a threshold counted in machines says "on"
//! while the data has nowhere to go. So the capacity is checked too:
//!
//! ```text
//! offered >= stored * copies
//! ```
//!
//! This is the half that matters most for the economics. Whoever offers the
//! network the most storage has the most of their own to place, and is
//! therefore the hardest to serve -- which is the precise point at which
//! "offer storage to earn storage" has to keep working or the scheme rewards
//! only the small.
//!
//! # Why this is a module and not a comment
//!
//! Because the dangerous direction is switching it on too early, and a rule
//! that lives in somebody's head gets switched on by somebody else.

/// The largest share of one account a single holder should end up with, as a
/// divisor: three means "no more than a third".
///
/// A choice, not a derivation. See the module documentation.
pub const SHARE_DIVISOR: usize = 3;

/// Why spreading is not on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Too few candidate holders for any of them to end up with a small share.
    TooFewHolders { have: usize, need: usize },

    /// Enough machines, not enough room on them.
    ///
    /// Nine peers offering a gigabyte each cannot spread a four-terabyte
    /// account, and a threshold counted in machines cannot see it.
    NotEnoughSpace { offered: u64, needed: u64 },

    /// Nobody has said how much room they have.
    ///
    /// A node learns its peers' pledges from the coordinator, and nothing asks
    /// for them yet. Until it does, the capacity question is unanswered rather
    /// than answered "yes" -- an unknown treated as a pass is how a check comes
    /// to bless the one case it was written for.
    CapacityUnknown,

    /// There is nothing to place.
    NothingStored,
}

/// Whether there is enough of a network to spread over without losing copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spreading {
    /// Whether to place chunks by rendezvous hashing rather than giving every
    /// holder everything.
    pub enabled: bool,
    /// Candidate holders counted.
    pub candidates: usize,
    /// How many there would have to be.
    pub needed: usize,
    /// What stops it, when something does.
    pub blocked_by: Option<Blocked>,
}

/// How many candidate holders a network needs before spreading helps.
#[must_use]
pub const fn critical_mass(copies: usize) -> usize {
    copies * SHARE_DIVISOR
}

/// How much room `copies` copies of `stored` bytes needs.
#[must_use]
pub const fn space_needed(stored: u64, copies: usize) -> u64 {
    stored.saturating_mul(copies as u64)
}

/// Whether to spread, given the network and what there is to place.
///
/// `offered` is `None` when the peers' pledges are not known -- which is the
/// state today. Unknown blocks rather than passes: off is the safe answer, and
/// off means every holder takes everything, which is what a small network
/// needs and what this project did before spreading existed.
#[must_use]
pub const fn spreading(
    candidates: usize,
    copies: usize,
    stored: u64,
    offered: Option<u64>,
) -> Spreading {
    let needed = critical_mass(copies);

    let blocked = if copies == 0 || stored == 0 {
        Some(Blocked::NothingStored)
    } else if candidates < needed {
        Some(Blocked::TooFewHolders {
            have: candidates,
            need: needed,
        })
    } else {
        match offered {
            None => Some(Blocked::CapacityUnknown),
            Some(offered) => {
                let required = space_needed(stored, copies);
                if offered < required {
                    Some(Blocked::NotEnoughSpace {
                        offered,
                        needed: required,
                    })
                } else {
                    None
                }
            }
        }
    };

    Spreading {
        enabled: blocked.is_none(),
        candidates,
        needed,
        blocked_by: blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::{Blocked, critical_mass, spreading};

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn a_small_network_never_spreads() {
        // The direction that can do damage. Spreading three copies over two
        // peers means one copy per chunk: a privacy preference turned into data
        // loss, on exactly the networks least able to afford it.
        for candidates in 0..critical_mass(3) {
            let advice = spreading(candidates, 3, GIB, Some(u64::MAX));
            assert!(
                !advice.enabled,
                "spreading turned on with {candidates} candidates, below the threshold"
            );
        }
    }

    #[test]
    fn enough_machines_with_no_room_is_not_enough() {
        // The objection a count of machines cannot see. Nine peers offering a
        // gigabyte each are nine peers; they cannot hold four terabytes three
        // times over, and a threshold counting only machines says "on" while
        // the data has nowhere to go.
        let four_terabytes = 4 * 1024 * GIB;
        let advice = spreading(9, 3, four_terabytes, Some(9 * GIB));

        assert!(
            !advice.enabled,
            "nine gigabytes were said to hold four terabytes three times"
        );
        assert!(matches!(
            advice.blocked_by,
            Some(Blocked::NotEnoughSpace { .. })
        ));

        // The same nine machines, with room, are fine.
        assert!(spreading(9, 3, four_terabytes, Some(12 * 1024 * GIB)).enabled);
    }

    #[test]
    fn not_knowing_the_capacity_blocks_rather_than_passes() {
        // A node learns its peers' pledges from the coordinator and nothing
        // asks yet, so this is today's real state. An unknown treated as a pass
        // is how a check comes to bless the one case it was written for.
        let advice = spreading(100, 3, GIB, None);
        assert!(!advice.enabled);
        assert!(matches!(advice.blocked_by, Some(Blocked::CapacityUnknown)));
    }

    #[test]
    fn the_threshold_is_where_a_holder_stops_getting_most_of_the_account() {
        // Nine candidates for three copies, because each then receives a third.
        assert_eq!(critical_mass(3), 9);
        assert_eq!(critical_mass(2), 6);

        assert!(spreading(9, 3, GIB, Some(u64::MAX)).enabled);
        let short = spreading(8, 3, GIB, Some(u64::MAX));
        assert!(!short.enabled);
        assert_eq!(
            short.blocked_by,
            Some(Blocked::TooFewHolders { have: 8, need: 9 })
        );
    }

    #[test]
    fn nothing_stored_is_not_a_reason_to_spread() {
        assert!(matches!(
            spreading(1000, 3, 0, Some(u64::MAX)).blocked_by,
            Some(Blocked::NothingStored)
        ));
        assert!(matches!(
            spreading(1000, 0, GIB, Some(u64::MAX)).blocked_by,
            Some(Blocked::NothingStored)
        ));
    }
}
