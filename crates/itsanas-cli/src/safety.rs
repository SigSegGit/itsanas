//! Whether this account's data is safe yet, said where the icon reads it.
//!
//! # Why this exists
//!
//! `itsanas status` has always answered "could you get it all back without
//! this machine?", and nobody runs it: it is a command, and on a working
//! machine the store is held by the daemon. The icon, which people *do* look
//! at, read `status --brief`, which knew whether the daemon was alive and
//! nothing about copies. So a node whose files existed on its own disk and
//! nowhere else was drawn green -- the failure of a backup that was never
//! running, and the reason MVP.md test L was "failing today by construction".
//!
//! The daemon already counts the copies every round (it writes `status` into
//! the snapshot). This puts the count, and since when it has been short, on
//! the snapshot's first line, where `brief_status` reads it without a
//! passphrase, and the trays turn it into a colour and one notification.
//!
//! # The grace, and why it is not given to a new account
//!
//! A file saved a second ago is on this machine only, until the round that
//! pushes it. Turning the icon orange for that would make orange the colour
//! of every save, and nobody reads a colour that means nothing. So a shortfall
//! that *follows a safe state* is left unsaid for [`GRACE_INTERVALS`] rounds.
//!
//! A shortfall that was never preceded by a safe state gets no grace at all:
//! a node installed a minute ago, alone, is exactly the person test L is
//! about -- the one about to drop their files in and walk away. Nor does a
//! header written by a version that did not carry the count: what the node
//! was before is unknown, and unknown is not safe.
//!
//! # Reaching nobody
//!
//! The copies are counted from holders heard from within `LIVE_FOR`, a week.
//! A laptop that runs every round and reaches no other machine (offline, its
//! coordinator moved, every peer off) writes a fresh snapshot each time, so
//! it is not `stale`, and keeps its copies for that week. The header carries
//! when a round last reached another machine (`reached T`), or, before any
//! round has, since when it has been waiting for one (`reached never since
//! T`), and past [`isolated_for`]'s threshold the icon says so.

/// How many sync intervals a shortfall that follows a safe state goes unsaid.
pub const GRACE_INTERVALS: u64 = 2;

/// The copies part of the snapshot's first line: `copies HAVE of WANT [since T]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Copies {
    /// Complete copies elsewhere, counting holders heard from lately
    /// (`Coverage::complete_elsewhere`).
    pub have: usize,
    /// What this system exists to keep (`SAFE_COPIES`).
    pub want: usize,
    /// When a shortfall that followed a safe state began; `None` when the
    /// account is safe, or has never been since this node has known it.
    pub since: Option<u64>,
}

impl Copies {
    /// Whether the account has the copies it is for.
    #[must_use]
    pub const fn met(&self) -> bool {
        self.have >= self.want
    }

    /// This round's field, from what the coverage says now and what the
    /// previous snapshot's header said. `None` when nothing is stored: no data
    /// is no question, not a safe answer to one.
    #[must_use]
    pub fn this_round(
        have: usize,
        want: usize,
        anything_stored: bool,
        previous: Option<Self>,
        now: u64,
    ) -> Option<Self> {
        if !anything_stored {
            return None;
        }
        if have >= want {
            return Some(Self {
                have,
                want,
                since: None,
            });
        }
        let since = match previous {
            // Safe last round and short now: a write on its way out.
            Some(before) if before.met() => Some(now),
            // Already short: the shortfall keeps the age it had, grace or none.
            Some(before) => before.since,
            // Nothing known before (a first round, an older header): no grace.
            None => None,
        };
        Some(Self { have, want, since })
    }

    /// The words appended to the header, with their leading space.
    #[must_use]
    pub fn render(&self) -> String {
        match self.since {
            Some(since) => format!(" copies {} of {} since {since}", self.have, self.want),
            None => format!(" copies {} of {}", self.have, self.want),
        }
    }

    /// Read back from a header; `None` when it carries no count.
    #[must_use]
    pub fn parse(header: &str) -> Option<Self> {
        let words: Vec<&str> = header.split_whitespace().collect();
        let at = words.iter().position(|word| *word == "copies")?;
        let have = words.get(at + 1)?.parse().ok()?;
        if words.get(at + 2) != Some(&"of") {
            return None;
        }
        let want = words.get(at + 3)?.parse().ok()?;
        let since = match words.get(at + 4) {
            Some(&"since") => Some(words.get(at + 5)?.parse().ok()?),
            _ => None,
        };
        Some(Self { have, want, since })
    }

    /// Whether the icon must say so now: short, and either with no grace or
    /// past it. `every` is the sync interval in seconds.
    #[must_use]
    pub fn exposed(&self, now: u64, every: u64) -> bool {
        !self.met()
            && self.since.is_none_or(|since| {
                now.saturating_sub(since) >= every.saturating_mul(GRACE_INTERVALS)
            })
    }
}

/// The shortest silence that reads `isolated`: a laptop closed for an evening
/// is not news, and orange every evening is a colour nobody reads.
pub const ISOLATED_AFTER: u64 = 6 * 3600;

/// The same in rounds, for a long interval: twelve hourly rounds, not six,
/// before one slow coordinator turns the icon orange.
pub const ISOLATED_INTERVALS: u64 = 12;

/// When a round last reached another machine, from a header's `reached T`.
#[must_use]
pub fn last_reached(header: &str) -> Option<u64> {
    let words: Vec<&str> = header.split_whitespace().collect();
    let at = words.iter().position(|word| *word == "reached")?;
    words.get(at + 1)?.parse().ok()
}

/// Since when a node that has never reached another machine has been waiting
/// to, from a header's `reached never since T`: the daemon stamps it on the
/// first snapshot that has no contact to carry. Without it, a node with
/// nothing stored (no `copies`, so never `exposed`) and no contact would read
/// green for good -- a new Mac behind a firewall, waiting to pull the account
/// down, and a laptop upgraded while offline from a version that wrote no
/// `reached`.
#[must_use]
pub fn waiting_since(header: &str) -> Option<u64> {
    let words: Vec<&str> = header.split_whitespace().collect();
    let at = words.iter().position(|word| *word == "reached")?;
    if words.get(at + 1..at + 3)? != ["never", "since"] {
        return None;
    }
    words.get(at + 3)?.parse().ok()
}

/// The header's contact words for this snapshot, with their leading space:
/// ` reached T` when a round reached a machine, now (`seen`, this process's)
/// or before (the `previous` header's), else ` reached never since T`.
///
/// Carried from the previous header, so a restarted daemon neither forgets a
/// week of silence nor starts it from zero. A time later than `now` is
/// dropped, from either side: a machine that booted with its clock in the
/// future (a resumed VM, a dead RTC) and reached a peer then would otherwise
/// keep that contact for good -- every later write keeps the larger value --
/// and read `healthy` cut off for days, until the real clock caught up.
#[must_use]
pub fn reached_words(seen: Option<u64>, previous: &str, now: u64) -> String {
    let past = |at: &u64| *at <= now;
    let reached = seen.filter(past).max(last_reached(previous).filter(past));
    if let Some(at) = reached {
        return format!(" reached {at}");
    }
    let since = waiting_since(previous).filter(past).unwrap_or(now);
    format!(" reached never since {since}")
}

/// How long this node has reached no other machine -- since the last contact,
/// or since it started waiting for a first one ([`waiting_since`]) -- once
/// that is long enough to say. `None` for a header that carries neither,
/// written by a version before this one: the next snapshot carries one.
/// `every` is the sync interval in seconds.
#[must_use]
pub fn isolated_for(header: &str, now: u64, every: u64) -> Option<u64> {
    let since = last_reached(header).or_else(|| waiting_since(header))?;
    let silent = now.saturating_sub(since);
    let threshold = ISOLATED_AFTER.max(every.saturating_mul(ISOLATED_INTERVALS));
    (silent >= threshold).then_some(silent)
}

#[cfg(test)]
mod tests {
    use super::{Copies, GRACE_INTERVALS, ISOLATED_AFTER, isolated_for, reached_words};

    const EVERY: u64 = 300;

    #[test]
    fn the_count_reads_back_as_written_and_a_header_without_one_has_none() {
        for copies in [
            Copies {
                have: 0,
                want: 2,
                since: None,
            },
            Copies {
                have: 1,
                want: 2,
                since: Some(1_792_000_000),
            },
            Copies {
                have: 3,
                want: 2,
                since: None,
            },
        ] {
            let header = format!(
                "snapshot 1792000300 every 300 version 0.1.13{}",
                copies.render()
            );
            assert_eq!(Copies::parse(&header), Some(copies), "{header}");
        }
        assert_eq!(
            Copies::parse("snapshot 1792000300 every 300 version 0.1.12"),
            None
        );
        assert_eq!(Copies::parse("snapshot 1 every 300 copies two of 2"), None);
    }

    /// The rule test L is about, from the first round of a node on its own.
    #[test]
    fn red_team_a_node_never_safe_is_exposed_from_its_first_round() {
        let now = 1_792_000_000;
        let first = Copies::this_round(0, 2, true, None, now).expect("stored");
        assert!(
            first.exposed(now, EVERY),
            "a node whose files are on its own disk only was given a grace on its first round: \
             the person installing it sees green, drops their files in and leaves ({first:?})"
        );
        let later = Copies::this_round(1, 2, true, Some(first), now + EVERY).expect("stored");
        assert!(
            later.exposed(now + EVERY, EVERY),
            "a node that has never been safe took a grace from its second round ({later:?})"
        );
        assert_eq!(
            Copies::this_round(0, 2, false, None, now),
            None,
            "no data is no question"
        );
    }

    #[test]
    fn red_team_a_save_on_a_safe_account_is_unsaid_for_the_grace_and_said_after() {
        let start = 1_792_000_000;
        let safe = Copies::this_round(2, 2, true, None, start).expect("stored");
        assert!(!safe.exposed(start, EVERY));
        let saved = Copies::this_round(0, 2, true, Some(safe), start + EVERY).expect("stored");
        assert_eq!(saved.since, Some(start + EVERY));
        assert!(
            !saved.exposed(start + EVERY, EVERY),
            "a file saved this round turned the icon orange: orange would mean every save"
        );
        let still = Copies::this_round(1, 2, true, Some(saved), start + 2 * EVERY).expect("stored");
        assert_eq!(
            still.since, saved.since,
            "the shortfall lost its age between rounds"
        );
        let past = start + EVERY + GRACE_INTERVALS * EVERY;
        assert!(
            still.exposed(past, EVERY),
            "a shortfall past its grace is still drawn as safe: the copies that were meant to \
             leave never did, and nothing says so ({still:?} at {past})"
        );
        let back = Copies::this_round(2, 2, true, Some(still), past).expect("stored");
        assert_eq!(
            back,
            Copies {
                have: 2,
                want: 2,
                since: None
            }
        );
    }

    fn header(words: &str) -> String {
        format!("snapshot 1 every {EVERY} version 0.1.13{words}")
    }

    /// A Mac set up to pull the account down, with an empty folder, behind a
    /// firewall: nothing stored, so no `copies` and never `exposed`, and no
    /// round ever reaches anybody. Without a start to age it read green for
    /// good. The same for a header from before `reached` (an upgrade while
    /// offline). Sabotage: restart the wait at every snapshot, or have
    /// `isolated_for` read only `reached T`.
    #[test]
    fn red_team_a_node_that_never_reached_anybody_turns_isolated() {
        let start = 1_792_000_000;
        let first = reached_words(None, &header(""), start);
        assert_eq!(first, format!(" reached never since {start}"));
        let later = start + ISOLATED_AFTER;
        let carried = reached_words(None, &header(&first), later);
        assert_eq!(
            carried, first,
            "the wait for a first contact started again at the next snapshot: a node that \
             never synced never turns from green"
        );
        assert_eq!(
            isolated_for(&header(&carried), later, EVERY),
            Some(ISOLATED_AFTER),
            "a node that has reached no machine since it started reads green past the \
             threshold: its files are nowhere else and nothing says so"
        );
        let reached = reached_words(Some(later), &header(&carried), later);
        assert_eq!(reached, format!(" reached {later}"));
        assert_eq!(isolated_for(&header(&reached), later, EVERY), None);
    }

    /// A VM resumed with its clock a day ahead reaches a peer and stamps
    /// `reached` a day in the future; NTP then puts the clock right. Kept,
    /// that contact outlives every write (the larger value wins) and the
    /// machine reads healthy cut off for that whole day. Sabotage: drop the
    /// filter on either side.
    #[test]
    fn red_team_a_contact_dated_in_the_future_is_not_kept() {
        let now = 1_792_000_000;
        let ahead = now + 24 * 3600;
        let carried = reached_words(None, &header(&format!(" reached {ahead}")), now);
        assert_eq!(
            carried,
            format!(" reached never since {now}"),
            "a contact dated in the future was carried: the machine reads healthy cut off \
             until the real clock passes it"
        );
        let own = reached_words(Some(ahead), &header(""), now);
        assert_eq!(
            own,
            format!(" reached never since {now}"),
            "this process's own contact, stamped before the clock went back, was kept"
        );
        let waiting = reached_words(None, &header(&format!(" reached never since {ahead}")), now);
        assert_eq!(waiting, format!(" reached never since {now}"));
        let earlier = now - 3600;
        assert_eq!(
            reached_words(None, &header(&format!(" reached {earlier}")), now),
            format!(" reached {earlier}"),
            "a real past contact was dropped"
        );
    }
}
