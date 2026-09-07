//! Which files a device keeps when it cannot keep them all.
//!
//! # The half that was missing
//!
//! A device can be told to hold at most so many bytes of its own account. That
//! bounds the *quantity* and says nothing about the *choice*: the first version
//! stopped downloading when the budget ran out, so what a phone ended up with
//! was whatever the merge engine happened to ask for first — which is the order
//! operations were written, possibly years ago, by another machine. A setting
//! that looks like "keep two gigabytes of my files" and delivers "keep the two
//! gigabytes you happened to create first" is worse than no setting, because it
//! invites trust it cannot repay.
//!
//! So the budget needs a rule for *what*, and the rule has to be applied in
//! both directions: fetch what belongs on the device, and let go of what does
//! not. A budget that only ever refuses is a ratchet — it fills once, and from
//! then on the newest file never arrives because the oldest is still there.
//! That was measured, not imagined: the trial device told to keep 200 KiB held
//! 907 KiB of blobs after one explicit fetch, and nothing in the system would
//! ever have brought it back down.
//!
//! # Why the choice is pure, and here
//!
//! It needs the path, the size and the date, and nothing else — no store, no
//! socket, no clock. Made pure, it can be argued with in a test rather than
//! observed on a phone; made a function of its inputs alone it is *stable*,
//! which is the property that stops a device fetching A, releasing B, and doing
//! the reverse on the next round for ever.
//!
//! # What it deliberately is not
//!
//! It is not a knapsack solver. Filling the budget as full as possible would
//! mean answering "keep my recent work" with a pile of tiny old files because
//! they pack better. The order is what somebody asked for; the budget only says
//! where to stop. The one concession is that a file too large for the room
//! *left* is skipped rather than treated as the end — otherwise one oversized
//! file near the top of the order would starve everything behind it for ever.

/// One file the account has, as far as this choice is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    /// The logical path.
    pub path: &'a str,
    /// Size in bytes.
    pub size: u64,
    /// When it was last modified, seconds since the Unix epoch. Advisory:
    /// clocks lie, and this only ever decides an ordering.
    pub modified_unix: u64,
    /// Whether the content is on this device now.
    ///
    /// Not part of the ranking — a file's importance does not change because it
    /// happens to have arrived already — but the caller needs the answer per
    /// file to know whether a verdict means *fetch this* or *let this go*, and
    /// carrying it here keeps the two halves from being computed from two
    /// different listings.
    pub here: bool,
}

/// Which files matter most when there is not room for all of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// Most recently modified first.
    ///
    /// The default, because it is the only one that matches what a phone is
    /// for. The file edited this morning is the one that gets opened on the
    /// train; the archive from six years ago is not.
    #[default]
    Newest,
    /// Least recently modified first.
    ///
    /// For a machine kept as an archive rather than a working copy — the Pi in
    /// the cupboard whose job is the old material the laptops have let go of.
    Oldest,
    /// Smallest first.
    ///
    /// Maximises the *number* of files present rather than their relevance. For
    /// a device with very little room, where "everything except the video" is a
    /// better answer than "the video".
    Smallest,
}

/// What a device has been told to hold of its own account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keeping {
    /// Bytes of this account's own content to hold, or `None` for all of it.
    pub budget: Option<u64>,
    /// Which files matter most when the budget cannot hold everything.
    pub order: Order,
    /// Path prefixes to restrict to. Empty means the whole account.
    ///
    /// A directory prefix matches the directory and everything under it.
    /// `Photos` does not match `Photos-old/x`: a filter that silently matches
    /// more than it names is how a phone fills with the wrong gigabytes.
    pub only: Vec<String>,
}

impl Keeping {
    /// Everything, with no limit. What a laptop wants.
    #[must_use]
    pub fn everything() -> Self {
        Self::default()
    }

    /// Whether this device holds all of its own account.
    ///
    /// True when there is no budget and no filter, which is the case for every
    /// machine that has room — and the case where the whole of this module can
    /// be skipped.
    #[must_use]
    pub fn is_everything(&self) -> bool {
        self.budget.is_none() && self.only.is_empty()
    }

    /// Whether `path` is inside the filter.
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        if self.only.is_empty() {
            return true;
        }
        self.only.iter().any(|prefix| {
            let prefix = prefix.trim_end_matches('/');
            path == prefix
                || (path.len() > prefix.len()
                    && path.starts_with(prefix)
                    && path.as_bytes()[prefix.len()] == b'/')
        })
    }
}

/// Why a file does not belong on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// Outside the path filter.
    Filtered,
    /// The budget was already full when its turn came.
    NoRoom,
    /// Larger than the whole budget, so no ordering would have fitted it.
    ///
    /// Separate from [`Left::NoRoom`] because the two need different sentences:
    /// one resolves itself when something else goes, the other never resolves
    /// and means raising the limit or fetching that file by hand.
    TooLarge,
}

/// What belongs on this device, and what does not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    /// Indices into the candidates that belong here, best first.
    pub keep: Vec<usize>,
    /// Indices that do not, each with the reason.
    pub leave: Vec<(usize, Left)>,
    /// Total size of everything kept.
    pub kept_bytes: u64,
}

impl Choice {
    /// Files that should be fetched: wanted, and not here yet.
    #[must_use]
    pub fn to_fetch(&self, candidates: &[Candidate<'_>]) -> Vec<usize> {
        self.keep
            .iter()
            .copied()
            .filter(|index| candidates.get(*index).is_some_and(|file| !file.here))
            .collect()
    }

    /// Files whose content should be let go of: here, and not wanted.
    ///
    /// The direction that makes the budget a window rather than a ratchet, and
    /// the one the caller must handle carefully: letting go of the only copy in
    /// existence is data loss, so the store refuses unless somebody else is
    /// known to hold it.
    #[must_use]
    pub fn to_release(&self, candidates: &[Candidate<'_>]) -> Vec<usize> {
        self.leave
            .iter()
            .map(|(index, _)| *index)
            .filter(|index| candidates.get(*index).is_some_and(|file| file.here))
            .collect()
    }

    /// How many were left out for each reason.
    #[must_use]
    pub fn left_because(&self, reason: Left) -> usize {
        self.leave
            .iter()
            .filter(|(_, actual)| *actual == reason)
            .count()
    }
}

/// Decide what belongs on this device.
///
/// Deterministic: the same listing and the same settings give the same answer,
/// on every device and on every round. That is not tidiness — it is what stops
/// two rounds disagreeing and spending a data plan swapping the same two files
/// back and forth.
#[must_use]
pub fn choose(candidates: &[Candidate<'_>], keeping: &Keeping) -> Choice {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by(|left, right| {
        let (left_file, right_file) = (&candidates[*left], &candidates[*right]);
        match keeping.order {
            Order::Newest => right_file.modified_unix.cmp(&left_file.modified_unix),
            Order::Oldest => left_file.modified_unix.cmp(&right_file.modified_unix),
            Order::Smallest => left_file.size.cmp(&right_file.size),
        }
        // Ties broken by path, so the order is total. Without it the answer
        // depends on the listing's order, which depends on the log, which
        // differs between devices — and two devices disagreeing about what
        // matters most is exactly the thrashing this is meant to prevent.
        .then_with(|| left_file.path.cmp(right_file.path))
    });

    let mut choice = Choice::default();
    for index in order {
        let file = &candidates[index];

        if !keeping.covers(file.path) {
            choice.leave.push((index, Left::Filtered));
            continue;
        }

        let Some(budget) = keeping.budget else {
            choice.kept_bytes = choice.kept_bytes.saturating_add(file.size);
            choice.keep.push(index);
            continue;
        };

        if file.size > budget {
            choice.leave.push((index, Left::TooLarge));
            continue;
        }

        if choice.kept_bytes.saturating_add(file.size) > budget {
            // Skipped, not stopped. One oversized file near the top of the
            // order must not starve everything behind it for ever.
            choice.leave.push((index, Left::NoRoom));
            continue;
        }

        choice.kept_bytes = choice.kept_bytes.saturating_add(file.size);
        choice.keep.push(index);
    }

    choice
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Choice, Keeping, Left, Order, choose};

    fn file(path: &str, size: u64, modified_unix: u64, here: bool) -> Candidate<'_> {
        Candidate {
            path,
            size,
            modified_unix,
            here,
        }
    }

    fn paths<'a>(indices: &[usize], candidates: &[Candidate<'a>]) -> Vec<&'a str> {
        indices
            .iter()
            .map(|index| candidates[*index].path)
            .collect()
    }

    #[test]
    fn the_budget_keeps_what_was_asked_for_not_what_arrived_first() {
        // The defect this module exists for. The listing is deliberately given
        // oldest-first, which is the order a log replays in: if the answer came
        // from arrival order, `ancient.txt` would be the one kept.
        let files = [
            file("ancient.txt", 60, 1_000, false),
            file("last-year.txt", 60, 500_000, false),
            file("this-morning.txt", 60, 900_000, false),
        ];

        let keeping = Keeping {
            budget: Some(120),
            ..Keeping::everything()
        };
        let choice = choose(&files, &keeping);

        assert_eq!(
            paths(&choice.keep, &files),
            ["this-morning.txt", "last-year.txt"],
            "the budget kept the wrong files"
        );
        assert_eq!(choice.kept_bytes, 120);
        assert_eq!(choice.left_because(Left::NoRoom), 1);
    }

    #[test]
    fn a_file_too_large_for_the_room_left_does_not_starve_the_rest() {
        // The boundary that decides whether the setting is usable at all. A
        // phone whose account starts with a film must still get the documents
        // behind it, and "stop at the first thing that does not fit" is the
        // obvious implementation that would not.
        let files = [
            file("film.mkv", 8_000, 900_000, false),
            file("notes.txt", 10, 800_000, false),
            file("receipt.pdf", 20, 700_000, false),
        ];

        let choice = choose(
            &files,
            &Keeping {
                budget: Some(100),
                ..Keeping::everything()
            },
        );

        assert_eq!(paths(&choice.keep, &files), ["notes.txt", "receipt.pdf"]);
        assert_eq!(choice.left_because(Left::TooLarge), 1);
    }

    #[test]
    fn the_answer_does_not_depend_on_the_order_the_files_were_listed_in() {
        // Stability is the anti-thrashing property: two rounds that see the
        // same account must want the same files, or the device spends a data
        // plan swapping them. Ties on the sort key are where an unstable
        // implementation shows itself, so every file here has the same date.
        let forwards = [
            file("a", 10, 5, false),
            file("b", 10, 5, false),
            file("c", 10, 5, false),
        ];
        let backwards = [
            file("c", 10, 5, false),
            file("b", 10, 5, false),
            file("a", 10, 5, false),
        ];

        let keeping = Keeping {
            budget: Some(20),
            ..Keeping::everything()
        };

        assert_eq!(
            paths(&choose(&forwards, &keeping).keep, &forwards),
            paths(&choose(&backwards, &keeping).keep, &backwards),
        );
    }

    #[test]
    fn what_is_here_and_not_wanted_is_offered_up_and_what_is_wanted_is_fetched() {
        // Both directions from one decision. Computing them separately is how a
        // device comes to release a file it is about to fetch again.
        let files = [
            file("old.bin", 100, 1_000, true),
            file("new.bin", 100, 900_000, false),
        ];

        let choice = choose(
            &files,
            &Keeping {
                budget: Some(100),
                ..Keeping::everything()
            },
        );

        assert_eq!(paths(&choice.to_fetch(&files), &files), ["new.bin"]);
        assert_eq!(paths(&choice.to_release(&files), &files), ["old.bin"]);
    }

    #[test]
    fn a_filter_matches_a_directory_and_not_a_name_that_merely_starts_the_same() {
        let files = [
            file("Photos/one.jpg", 1, 1, false),
            file("Photos", 1, 1, false),
            file("Photos-old/two.jpg", 1, 1, false),
            file("Documents/three.txt", 1, 1, false),
        ];

        let choice = choose(
            &files,
            &Keeping {
                only: vec!["Photos".to_owned()],
                ..Keeping::everything()
            },
        );

        let mut kept = paths(&choice.keep, &files);
        kept.sort_unstable();
        assert_eq!(kept, ["Photos", "Photos/one.jpg"]);
        assert_eq!(choice.left_because(Left::Filtered), 2);
    }

    #[test]
    fn no_budget_and_no_filter_keeps_everything() {
        let files = [file("a", u64::MAX, 1, false), file("b", u64::MAX, 2, false)];
        let choice = choose(&files, &Keeping::everything());
        assert_eq!(choice.keep.len(), 2);
        assert!(choice.leave.is_empty());
        assert!(Keeping::everything().is_everything());
    }

    #[test]
    fn smallest_first_keeps_the_most_files_and_oldest_first_keeps_the_archive() {
        // Same account, same budget, three orders, three different answers --
        // which is the point: the order is the setting, and a device that
        // ignored it would give the same answer to all three.
        let files = [
            file("big-and-new.bin", 90, 900_000, false),
            file("small-and-old.txt", 10, 1_000, false),
            file("small-and-new.txt", 10, 800_000, false),
        ];
        let budget = Some(100);

        let smallest = choose(
            &files,
            &Keeping {
                budget,
                order: Order::Smallest,
                ..Keeping::everything()
            },
        );
        let mut kept = paths(&smallest.keep, &files);
        kept.sort_unstable();
        assert_eq!(
            kept,
            ["small-and-new.txt", "small-and-old.txt"],
            "smallest-first should keep the two that fit rather than the one that fills it"
        );

        let newest = choose(
            &files,
            &Keeping {
                budget,
                order: Order::Newest,
                ..Keeping::everything()
            },
        );
        assert_eq!(
            paths(&newest.keep, &files),
            ["big-and-new.bin", "small-and-new.txt"],
            "newest-first should spend the budget on the recent file, whatever its size"
        );

        let oldest = choose(
            &files,
            &Keeping {
                budget: Some(10),
                order: Order::Oldest,
                ..Keeping::everything()
            },
        );
        assert_eq!(paths(&oldest.keep, &files), ["small-and-old.txt"]);
    }

    #[test]
    fn an_empty_choice_asks_for_nothing() {
        let choice = Choice::default();
        assert!(choice.to_fetch(&[]).is_empty());
        assert!(choice.to_release(&[]).is_empty());
    }
}
