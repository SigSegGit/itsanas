//! Where the account's copies are: the coverage, concentration and spreading
//! lines `status` prints, and the members and peers it warns about.

use std::fmt::Write as _;

use itsanas_store::REPLICATION_TARGET;

use crate::SAFE_COPIES;
use crate::device_command::unplug_words;
use crate::safety;
use crate::status::describe_age;
use crate::{config::format_size, error::Result, node::Node};

/// Name the machines of the account resting past a week, calmly: a key in a
/// drawer is not a fault, and the copy count leaves them out until they are
/// heard from again, which a person would otherwise read as copies lost.
pub(crate) fn report_dormant_members(node: &Node, out: &mut String, now: u64) -> Result<()> {
    let dormant = node.store.dormant_members(now)?;
    if dormant.is_empty() {
        return Ok(());
    }
    let _ = writeln!(
        out,
        "\nresting machines of this account (not heard from for over a week; their copies count again once they are)"
    );
    for (device, seen) in &dormant {
        let when = seen.map_or_else(
            || "never heard from here".to_owned(),
            |at| format!("last heard {}", describe_age(now.saturating_sub(at))),
        );
        let _ = writeln!(out, "  {}  {when}", device.short());
    }
    Ok(())
}

/// Say which peers have failed a storage challenge, if any have.
///
/// Silence means every peer that has ever been audited answered, which is the
/// ordinary case and worth no words at all.
pub(crate) fn report_unreliable_peers(node: &Node, out: &mut String) -> Result<()> {
    // The machines resting past a week first: the other half of "which of my
    // machines is not doing its part", and the calm one.
    report_dormant_members(node, out, itsanas_store::now_unix())?;
    if let Some(why) = node.store.twin() {
        let _ = writeln!(
            out,
            "\nTHIS MACHINE WRITES NOTHING: another disk holds its identity ({why}).\n  Two copies of one key image are both in use. Keep one; set the other up\n  afresh as a machine of its own (itsanas setup on a new home). Nothing was deleted."
        );
    }
    let unreliable = node.store.unreliable_devices()?;
    if unreliable.is_empty() {
        return Ok(());
    }

    // Into the text, not to stdout: printed, it went to the daemon's log and
    // never reached the snapshot, so on a running node `status` could not say
    // which machine had failed.
    let _ = writeln!(out, "\npeers that have failed a storage challenge");
    for (device, record) in &unreliable {
        match record.complaint(device) {
            Some(complaint) => {
                let _ = writeln!(out, "  {complaint}");
            }
            None => {
                let _ = writeln!(
                    out,
                    "  {} answered {} and failed {}, and is answering now",
                    device.short(),
                    record.passed,
                    record.failed
                );
            }
        }
    }
    Ok(())
}

/// Whether chunks could be spread around instead of every holder taking
/// everything -- and it is off below a threshold on purpose.
///
/// With two peers and a target of two copies, every chunk must go to both, so
/// spreading would give each chunk one holder instead of two: a privacy
/// preference turned into data loss, on the networks least able to afford it.
///
/// The capacity half is why `offered` exists and why it is `None` today. A node
/// learns its peers' pledges from the coordinator and nothing asks for them, so
/// the honest answer is "not known" rather than "fine" -- nine peers with a
/// gigabyte each cannot spread four terabytes, and a threshold in machines
/// cannot see it.
/// The other direction, and it is not the same question.
///
/// Copies are about surviving loss; this is about who could read you if the
/// sealing ever failed, and about whether this can scale at all — if the unit of
/// hosting were "a whole account", somebody offering four terabytes would need
/// peers who could each take four terabytes.
pub(crate) fn concentration_report(
    node: &Node,
    coverage: &itsanas_store::Coverage,
) -> Result<String> {
    let mut out = String::new();
    macro_rules! w {
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }

    if coverage.someone_holds_everything() {
        w!(
            concat!(
                "  concentrated   one machine holds all {} of your chunks. ",
                "Sealed, but a whole set"
            ),
            coverage.live_chunks
        );
        w!("                 unavoidable with few peers; spread as more join");
    } else if coverage.largest_share > 0 {
        w!(
            "  spread         no machine holds more than {} of your {} chunks",
            coverage.largest_share,
            coverage.live_chunks
        );
    }

    let _ = write!(
        out,
        "{}",
        spreading_report(
            coverage.distinct_holders,
            node.store.stats()?.bytes_on_disk,
            None
        )
    );

    let short = node
        .store
        .under_replicated(REPLICATION_TARGET, itsanas_discover::now_unix())?;
    if !short.is_empty() {
        w!(
            concat!(
                "  headroom       {} chunks are on fewer than {} machines, ",
                "counting this one"
            ),
            short.len(),
            REPLICATION_TARGET
        );
    }

    Ok(out)
}

pub(crate) fn spreading_report(candidates: usize, stored: u64, offered: Option<u64>) -> String {
    use itsanas_placement::Blocked;

    let mut out = String::new();
    macro_rules! w {
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }

    let advice = itsanas_placement::spreading(candidates, REPLICATION_TARGET, stored, offered);
    match advice.blocked_by {
        None => w!(
            "  spreading      on: {} machines with room for {} copies",
            advice.candidates,
            REPLICATION_TARGET
        ),
        Some(Blocked::NothingStored) => {}
        Some(Blocked::TooFewHolders { have, need }) => {
            w!(
                "  spreading      off: {have} machines hold anything of yours, and {need} are needed"
            );
            w!("                 until then every holder takes everything, which is right");
        }
        Some(Blocked::NotEnoughSpace { offered, needed }) => {
            w!(
                "  spreading      off: your peers offer {}, and {} copies need {}",
                format_size(offered),
                REPLICATION_TARGET,
                format_size(needed)
            );
        }
        Some(Blocked::CapacityUnknown) => {
            w!("  spreading      off: nobody has said how much room they have");
            w!("                 the coordinator knows; nothing asks it yet");
        }
    }
    out
}

/// The part of `status` that answers the question this project exists for.
///
/// Separated because it is the headline and deserves to be readable on its own,
/// and because `render_status` went over its line budget the moment it grew.
/// Chunks this account has that this machine does not hold.
///
/// The measure of how much of the reassurance in `status` is out of scope. It
/// is zero on every machine that holds its whole account, which is why it went
/// unnoticed until one did not.
pub(crate) fn chunks_not_here(node: &Node) -> Result<usize> {
    let listing = itsanas_store::catalogue(&node.store, &node.vault)?;
    let absent: std::collections::BTreeSet<String> = listing
        .files
        .into_iter()
        .filter(|file| file.presence == itsanas_store::Presence::Absent)
        .map(|file| file.path)
        .collect();

    if absent.is_empty() {
        return Ok(0);
    }

    Ok(
        itsanas_store::chunks_for_all(&node.store, &node.vault, &absent)?
            .into_iter()
            .filter(|chunk| !node.store.has_chunk(chunk))
            .count(),
    )
}

/// Written to the end of `status`, and returns the copies it counted for the
/// snapshot's first line (`since` is the daemon's to decide, from the
/// previous header); none when nothing is stored.
pub(crate) fn coverage_report(node: &Node, status: &mut String) -> Result<Option<safety::Copies>> {
    let mut out = String::new();
    macro_rules! w {
        () => {{ let _ = writeln!(out); }};
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }

    // What this question can and cannot cover, said before the answer rather
    // than after it.
    //
    // `Store::coverage` walks chunks with a live local reference -- that is,
    // the ones this machine holds. On a machine that holds its whole account
    // that is the account. On one that has released content it is a slice, and
    // the reassuring line underneath was being computed over the slice while
    // reading as though it covered everything. Worse: the released chunks are
    // exactly the ones this machine can no longer audit, because a challenge is
    // verified against a local copy.
    //
    // So the count comes first, and the headline changes with it.
    let unspoken = chunks_not_here(node)?;
    if unspoken > 0 {
        w!("could you get back what is ON THIS MACHINE, without this machine?");
    } else {
        w!("could you get it all back without this machine?");
    }

    // The headline is a minimum, not an average, and it does not count this
    // machine. A file comes back only if every one of its chunks does, so an
    // account with almost everything on three machines and one chunk on none
    // has no complete copy at all -- and an average would report that as
    // "nearly three" and read as comfortable.
    let coverage = node.store.coverage(itsanas_discover::now_unix())?;
    if coverage.live_chunks == 0 {
        w!("  nothing stored yet");
    } else {
        match coverage.complete_elsewhere {
            0 => w!(
                concat!(
                    "  NO             not one complete copy exists anywhere ",
                    "else; {} of {} chunks are only here"
                ),
                coverage.only_here,
                coverage.live_chunks
            ),
            1 => w!(concat!(
                "  one copy       every chunk is on one other machine, so ",
                "the network could rebuild all of it once"
            )),
            copies => {
                // Not "any N machines could rebuild it". They could not: N
                // machines chosen at random may hold overlapping subsets and
                // nothing else. What is true is that every chunk is on at least
                // N of them, so the set survives losing any N-1 holders of any
                // chunk -- and rebuilding draws from many peers, not from one.
                w!(
                    concat!(
                        "  {} copies      every chunk is on at least {} other ",
                        "machines; rebuilding draws on all of them"
                    ),
                    copies,
                    copies
                );
            }
        }

        if unspoken > 0 {
            w!(
                concat!(
                    "  not covered    {} more chunks belong to this account and ",
                    "are not on this machine"
                ),
                unspoken
            );
            w!("                 nothing above speaks for them, and this node cannot");
            w!("                 audit them: a challenge is checked against a local copy");
        }

        // The question a USB key, a laptop before a trip or a phone before a
        // reset is plugged in to answer: may it go now? (HANDOVER §8 0e step
        // 5.) Said from the same count as the promise below, so the two never
        // disagree.
        w!("{}", unplug_words(coverage.complete_elsewhere));

        if !coverage.meets(SAFE_COPIES) {
            w!(
                concat!(
                    "  the promise    {} complete copies is what this is for; ",
                    "you have {}"
                ),
                SAFE_COPIES,
                coverage.complete_elsewhere
            );
            w!("                 run `itsanas sync`, or add a peer, to spread it");
        }

        // How much of the reassurance above is memory rather than observation.
        // A holder record says a device once acknowledged a chunk; it says
        // nothing about whether that device still exists. This fleet had a
        // destroyed machine listed as a holder until somebody read a log.
        if coverage.resting_on_memory() {
            w!(
                concat!(
                    "  unconfirmed    the ledger remembers {} copies; {} holder ",
                    "records have gone quiet and are not counted above"
                ),
                coverage.claimed_elsewhere,
                coverage.stale_records
            );
        }

        let _ = write!(out, "{}", concentration_report(node, &coverage)?);
    }

    let copies = (coverage.live_chunks > 0).then_some(safety::Copies {
        have: coverage.complete_elsewhere,
        want: SAFE_COPIES,
        since: None,
    });
    status.push_str(&out);
    Ok(copies)
}
