//! The daemon's side of placement: one frozen [`Placement`] per round, and
//! what `status` says about availability.
//!
//! The rules are `itsanas_placement`'s and the store-reading is
//! `itsanas_net::placement`'s. What is left for the shell is *when*: build at
//! the start of a round, run the round's census slice, hand the same `Arc` to
//! every push of the round and to the listener, and say the figures out loud.
//!
//! What this module refuses to do:
//!
//! - **Act in `everyone`.** No placement is built and the slot stays empty,
//!   so every push and every `WantHosted` takes the path it took before
//!   placement existed. Measurement (the rings) runs in both modes; acting on
//!   it is the opt-in.
//! - **Rebuild during a round.** One build per round, shared by every push and
//!   by the listener: two pushes of one round ranking a chunk two ways is how
//!   a chunk ends up on every host that answered.
//! - **Fail a round over placement.** A build or census that cannot read the
//!   store leaves the slot empty for that round, which is `everyone`: more
//!   copies than wanted, never fewer.
//! - **Print a figure it has not got.** A device with less than a week of
//!   history reads "not enough history yet", not a percentage of three hours.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::{Arc, Mutex, PoisonError},
};

use itsanas_crypto::{DeviceId, UserId};
use itsanas_net::{CensusReport, Placement, PlacementSlot};
use itsanas_node::config::PlacementMode;
use itsanas_placement::{Uptime, WINDOW_HOURS, lost_after};
use itsanas_store::{Store, holders::rows_per_round};

/// The slot the listener reads and each round replaces. One per process, like
/// the claim book: a node runs one daemon.
pub fn slot() -> &'static PlacementSlot {
    static SLOT: PlacementSlot = PlacementSlot::new(None);
    &SLOT
}

/// The placement the current round pushes with, if `measured`.
pub fn current() -> Option<Arc<Placement>> {
    slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// What the last census slice found and how big it was meant to be, for the
/// snapshot the same process writes after the round.
static LAST_CENSUS: Mutex<Option<Census>> = Mutex::new(None);

/// One round's census slice, as `status` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Census {
    pub report: CensusReport,
    /// Live chunks when the slice was sized.
    pub live: u64,
}

/// The account's own devices as far as this node knows them: the ones the
/// address book lists and the ones whose claim names this account.
///
/// Both, because either alone misses machines: a laptop never published has
/// no book row, and a machine that never dialled here presented no claim. An
/// own device left out would be ranked as a host of the same account, which
/// costs a copy elsewhere; it is never counted for durability twice, because
/// its claim already says whose it is.
pub fn own_devices(
    listed: impl IntoIterator<Item = DeviceId>,
    accounts: &BTreeMap<DeviceId, UserId>,
    owner: UserId,
) -> BTreeSet<DeviceId> {
    let mut own: BTreeSet<DeviceId> = listed.into_iter().collect();
    own.extend(
        accounts
            .iter()
            .filter(|(_, account)| **account == owner)
            .map(|(device, _)| *device),
    );
    own
}

/// Start a round: in `measured`, freeze the placement, run the census slice
/// and swap it into the slot; in `everyone`, empty the slot.
///
/// `interval_secs` sizes the slice so a whole pass over the live chunks fits
/// in `REFRESH_AFTER` ([`rows_per_round`]). Returns what was put in the slot.
pub fn begin_round(
    mode: PlacementMode,
    store: &Store,
    own: BTreeSet<DeviceId>,
    accounts: &BTreeMap<DeviceId, UserId>,
    now: u64,
    interval_secs: u64,
) -> Option<Arc<Placement>> {
    // Before anything else of the round, so the triggers of this round's
    // pushes already see the mode: armed in `measured`, emptied and disarmed
    // in `everyone`, where nothing would ever work the queue off.
    if let Err(error) = store.set_placement_measured(mode == PlacementMode::Measured) {
        eprintln!("itsanas: the placement queue could not be switched ({error})");
    }
    let placement = match mode {
        PlacementMode::Everyone => None,
        PlacementMode::Measured => match Placement::build(store, own, accounts, now) {
            Ok(placement) => {
                census(store, &placement, interval_secs);
                Some(Arc::new(placement))
            }
            Err(error) => {
                eprintln!("itsanas: placement could not be built this round ({error})");
                None
            }
        },
    };
    placement.clone_into(&mut slot().write().unwrap_or_else(PoisonError::into_inner));
    placement
}

/// Run this round's census slice and keep what it found for `status`.
fn census(store: &Store, placement: &Placement, interval_secs: u64) {
    let live = match store.live_chunk_count() {
        Ok(live) => live,
        Err(error) => {
            eprintln!("itsanas: the census could not count live chunks ({error})");
            return;
        }
    };
    match placement.census(store, rows_per_round(live, interval_secs)) {
        Ok(report) => {
            *LAST_CENSUS.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(Census { report, live });
        }
        Err(error) => eprintln!("itsanas: the census did not run this round ({error})"),
    }
}

/// The last census slice this process ran, if any.
pub fn last_census() -> Option<Census> {
    *LAST_CENSUS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A dial to `expect` failed: an hour this node tried and did not reach it,
/// which is what its ring is made of (seen over *tried*).
///
/// Only a pinned device: an address that answered nobody names nobody. The
/// store notes only a device it already knows, so a dial to a stranger's
/// address writes nothing, and a failed write costs one hour of one ring,
/// never the round.
pub fn unreached(store: &Store, expect: Option<DeviceId>) {
    if let Some(device) = expect {
        let _ = store.note_unreached_at(&device, itsanas_discover::now_unix());
    }
}

/// [`render`] from the store, at the wall clock.
///
/// # Errors
///
/// If the store cannot be read.
pub fn report(mode: PlacementMode, store: &Store) -> itsanas_store::Result<String> {
    Ok(render(
        mode,
        &store.uptimes()?,
        store.short_count()?,
        last_census(),
        itsanas_discover::now_unix(),
    ))
}

/// `7 d 14 h`, or `31 h` under a day: how long a person reads an absence as.
fn describe_hours(hours: u64) -> String {
    if hours < 24 {
        format!("{hours} h")
    } else {
        format!("{} d {} h", hours / 24, hours % 24)
    }
}

/// One device's availability as this node measured it, at `now` (unix secs).
///
/// "up 83 % (19 days observed), longest absence 31 h, presumed lost after
/// 7 d 0 h". The percentage is floored, like every figure placement uses: a
/// host never reads as more available than it was.
#[must_use]
pub fn describe_uptime(uptime: &Uptime, now: u64) -> String {
    let hour = now / 3600;
    let lost = lost_after(Some(uptime));
    let Some(per_mille) = uptime.availability(hour, lost / 3600) else {
        return "not enough history yet".to_owned();
    };
    let observed = hour
        .saturating_sub(uptime.first_hour())
        .saturating_add(1)
        .min(WINDOW_HOURS);
    format!(
        "up {} % ({} days observed), longest absence {}, presumed lost after {}",
        per_mille / 10,
        observed / 24,
        // In hours, as the spec's line reads: an absence is felt by the
        // hour, a window is planned by the day.
        format_args!("{} h", uptime.longest_gap_hours()),
        describe_hours(lost / 3600)
    )
}

/// The `availability` section of `status`: one line per device this node
/// keeps a ring for, then, in `measured`, the short queue and the census.
#[must_use]
pub fn render(
    mode: PlacementMode,
    rings: &[(DeviceId, Uptime)],
    short: u64,
    census: Option<Census>,
    now: u64,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "availability, as this machine measured it");
    if rings.is_empty() {
        let _ = writeln!(out, "  no device measured yet");
    }
    for (device, uptime) in rings {
        let _ = writeln!(out, "  {device}  {}", describe_uptime(uptime, now));
    }
    let _ = writeln!(out, "  placement       {}", mode.name());
    if mode == PlacementMode::Measured {
        let _ = writeln!(
            out,
            "  short           {short} chunks short of their target"
        );
        match census {
            Some(Census { report, live }) => {
                let _ = writeln!(
                    out,
                    "  census          {} of {live} live chunks examined last round \
                     ({} short, {} settled)",
                    report.examined, report.short, report.settled
                );
            }
            None => {
                let _ = writeln!(out, "  census          not run by this process yet");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use itsanas_crypto::{DeviceKeys, SecretBytes};

    use super::*;

    const HOUR: u64 = 3600;
    /// An hour index far from zero, so nothing passes by underflowing.
    const START: u64 = 500_000;

    fn device(seed: u8) -> DeviceId {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32])).device_id()
    }

    /// A ring tried every hour for `hours`, seen except during `gap` (a range
    /// of offsets from the start).
    fn ring(hours: u64, gap: std::ops::Range<u64>) -> Uptime {
        let mut uptime = Uptime::new(START);
        for offset in 0..hours {
            uptime.note(START + offset, !gap.contains(&offset));
        }
        uptime
    }

    #[test]
    fn a_device_with_weeks_of_history_reads_its_share_its_gap_and_its_window() {
        // 20 days, absent for 31 hours once: 449 of 480 hours seen is 93.5 %,
        // floored to 93; the window is 2 × 31 h, under the 7-day floor.
        let uptime = ring(480, 100..131);
        let now = (START + 479) * HOUR;

        assert_eq!(
            describe_uptime(&uptime, now),
            "up 93 % (20 days observed), longest absence 31 h, presumed lost after 7 d 0 h"
        );
    }

    #[test]
    fn red_team_a_device_seen_for_three_hours_gets_no_percentage() {
        // Three good hours are not a 100 % host. A figure this early is what
        // a stranger would use to buy itself a rank.
        let uptime = ring(3, 0..0);

        assert_eq!(
            describe_uptime(&uptime, (START + 2) * HOUR),
            "not enough history yet"
        );
    }

    #[test]
    fn a_long_gap_widens_the_window_up_to_thirteen_days() {
        // Gone for five days once: the window is clamped to 13 days, not 10
        // days of a 2 × gap that would read longer than the ceiling allows.
        let uptime = ring(27 * 24, 48..(48 + 7 * 24));
        let text = describe_uptime(&uptime, (START + 27 * 24 - 1) * HOUR);

        assert!(text.contains("longest absence 168 h"), "{text}");
        assert!(text.contains("presumed lost after 13 d 0 h"), "{text}");
    }

    #[test]
    fn everyone_shows_the_measurement_and_no_queue() {
        // Measurement is always on, so a member can read it before choosing
        // `measured`; the queue means nothing until then.
        let text = render(
            PlacementMode::Everyone,
            &[(device(1), ring(3, 0..0))],
            7,
            None,
            (START + 2) * HOUR,
        );

        assert!(text.contains("not enough history yet"), "{text}");
        assert!(text.contains("placement       everyone"), "{text}");
        assert!(!text.contains("short of their target"), "{text}");
    }

    #[test]
    fn measured_shows_the_short_queue_and_the_census_slice() {
        let census = Census {
            report: CensusReport {
                examined: 12,
                short: 3,
                settled: 9,
            },
            live: 4000,
        };
        let text = render(PlacementMode::Measured, &[], 5, Some(census), START * HOUR);

        assert!(text.contains("5 chunks short of their target"), "{text}");
        assert!(
            text.contains("12 of 4000 live chunks examined last round (3 short, 9 settled)"),
            "{text}"
        );
        assert!(text.contains("no device measured yet"), "{text}");
    }

    #[test]
    fn red_team_an_own_device_known_only_by_its_claim_is_still_own() {
        // A laptop never published has no book row; its claim is the only
        // word that it is ours. Ranked as a host, it would take a copy meant
        // for another household.
        let (me, them) = (
            itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([1; 32]))
                .user_id(),
            itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([2; 32]))
                .user_id(),
        );
        let accounts = BTreeMap::from([(device(2), me), (device(3), them)]);

        let own = own_devices([device(1)], &accounts, me);

        assert_eq!(own, BTreeSet::from([device(1), device(2)]));
    }

    fn store() -> (tempfile::TempDir, Store) {
        use itsanas_crypto::{MasterSecret, UserKeys};
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open_for_testing(
            dir.path().join("store"),
            UserKeys::derive(&MasterSecret::from_bytes([7; 32])),
            DeviceKeys::from_seed(&SecretBytes::new([8; 32])),
            // Small chunks, so one file is several and the census has a page.
            itsanas_store::ChunkerConfig::new(1024, 4096, 16 * 1024).expect("chunker"),
        )
        .expect("store");
        (dir, store)
    }

    fn noise(bytes: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        (0..bytes)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect()
    }

    // The only test that touches the process-wide slot, so parallel tests
    // cannot race on it.
    #[test]
    fn measured_fills_the_slot_and_runs_the_census_and_everyone_empties_it() {
        let (_dir, store) = store();
        store.write_file("a.bin", &noise(64 * 1024)).expect("write");
        let now = 1_800_000_000;
        // A census slice of every chunk: an interval as long as a pass.
        let interval = itsanas_store::holders::REFRESH_AFTER;

        let placed = begin_round(
            PlacementMode::Measured,
            &store,
            BTreeSet::new(),
            &BTreeMap::new(),
            now,
            interval,
        )
        .expect("measured builds a placement");
        let shared = current().expect("and puts it in the slot");
        assert!(Arc::ptr_eq(&placed, &shared), "one placement per round");
        let census = last_census().expect("the census ran");
        assert!(census.report.examined > 1, "{census:?}");
        assert_eq!(census.report.examined, census.report.short);
        assert!(store.short_count().expect("count") > 1);

        let none = begin_round(
            PlacementMode::Everyone,
            &store,
            BTreeSet::new(),
            &BTreeMap::new(),
            now,
            interval,
        );
        assert!(none.is_none());
        assert!(current().is_none(), "everyone answers as before placement");
    }

    #[test]
    fn red_team_a_failed_dial_counts_against_a_known_device_and_writes_nothing_for_a_stranger() {
        let (_dir, store) = store();
        let (known, stranger) = (device(0x21), device(0x22));
        // Two hours ago, so the failed dial now lands in a later hour.
        let earlier = itsanas_discover::now_unix() - 2 * HOUR;
        store.note_seen_at(&known, earlier).expect("seen");

        unreached(&store, Some(known));
        unreached(&store, Some(stranger));
        unreached(&store, None);

        let ring = store.uptime(&known).expect("read").expect("a ring");
        assert_eq!(ring.attempted_hours(), 2, "the failed hour was tried");
        assert_eq!(ring.seen_hours(), 1);
        assert!(store.uptime(&stranger).expect("read").is_none());
    }
}
