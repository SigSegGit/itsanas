//! A sync round for a device that cannot hold its whole account.
//!
//! # Why this is not part of `session`
//!
//! The network layer moves bytes and does not decide which ones matter. The
//! *choice* is `itsanas_policy::keeping`, the listing it chooses from is
//! `itsanas_store::catalogue`, and joining those two to a connection is the
//! shell's job. Putting it in `session` would make the network crate depend on
//! a policy it has no opinion about.
//!
//! # The shape of a round with a budget
//!
//! 1. **Push.** Offer this device's own work. Unchanged, and unconditional: a
//!    device short of room still owes the account everything it has created.
//! 2. **Refresh.** Fetch signed log segments — kilobytes — so the catalogue
//!    knows every file the account has, with sizes and dates.
//! 3. **Choose.** Rank by the configured order, fill the budget, and get back
//!    both halves: what to fetch and what to let go of.
//! 4. **Fetch.** Ask only for the chunks of the chosen files. Everything else
//!    defers exactly as it does when a peer is asleep, and stays listed.
//! 5. **Release.** Let go of content that is here and not chosen — refused for
//!    any chunk no live holder is known to have, which is what keeps a budget
//!    from becoming a delete.
//! 6. **Say so.** Tell the peer what was released, so its ledger stops counting
//!    this device as a copy of chunks it no longer has.
//!
//! Step 6 is the one that is easy to leave out and expensive to leave out. The
//! placement ledger is what somebody consults before believing their data is
//! safe; a device that quietly stops holding chunks while every peer still
//! counts it inflates that number, and the audit — sixteen chunks per peer per
//! round — would take most of a year to notice on a million-chunk account.
//!
//! # A laptop takes none of this path
//!
//! With no budget and no filter the round is exactly what it always was. The
//! selective path costs a second walk of the log; there is no reason to pay it
//! on a machine that is going to want everything anyway.

use std::collections::BTreeSet;

use itsanas_crypto::ChunkId;
use itsanas_net::{PeerClient, protocol::MAX_HAVE_BATCH, session};
use itsanas_policy::keeping::{Candidate, Keeping, Left};
use itsanas_store::{Presence, Release, Store, Vault};

use crate::error::Result;

/// What a selective round did, beyond the ordinary push and pull counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeepingReport {
    /// Files fetched because the choice wanted them here.
    pub fetched: usize,
    /// Files whose content was let go of.
    pub released: usize,
    /// Bytes reclaimed by those releases.
    pub freed: u64,
    /// Files that should have been let go of and could not be, because too few
    /// other live machines hold some chunk of them.
    ///
    /// Not a failure to hide: it means the budget is over its limit *and*
    /// letting go would leave fewer than two copies. The honest answer is to
    /// keep it and say so.
    pub not_safe_yet: usize,
    /// Files left out because they are larger than the whole budget.
    pub too_large: usize,
    /// Files left where they are because the budget was already full.
    pub not_room: usize,
    /// Files outside the path filter.
    pub filtered: usize,
    /// Whether the peer was told what this device released.
    pub told_the_peer: bool,
}

impl KeepingReport {
    /// Whether anything happened worth a line of output.
    #[must_use]
    pub const fn worth_reporting(&self) -> bool {
        self.fetched > 0 || self.released > 0 || self.not_safe_yet > 0
    }
}

/// One round against `client`, honouring what this device was told to keep.
///
/// # Errors
///
/// If the peer fails, or the store cannot be read or written.
pub fn round(
    store: &Store,
    vault: &Vault,
    keeping: &Keeping,
    client: &mut PeerClient,
    scope: session::Scope,
) -> Result<(session::RoundReport, KeepingReport)> {
    if keeping.is_everything() {
        // Every machine with room takes this path, and it is the path this
        // project had before budgets existed. The selective one costs a second
        // walk of the log to answer a question whose answer is "all of it".
        return Ok((
            session::round_scoped(store, vault, client, scope)?,
            KeepingReport::default(),
        ));
    }

    // A round that got this far spoke to the peer, which is what makes its past
    // acknowledgements count as copies. Same reason as `session::round_scoped`.
    store.note_seen(&client.peer_device())?;

    let push = session::push_scoped(store, client, scope)?;
    let fetched = session::refresh(store, vault, client)?;

    let mut report = KeepingReport::default();

    if !scope.moves_content() {
        // The list is current and nothing was downloaded, which is the whole
        // point of a metadata round. Choosing now would be choosing what to
        // fetch on a connection this device has decided not to fetch over.
        return Ok((
            session::RoundReport {
                push,
                pull: itsanas_sync::SyncReport::default(),
            },
            report,
        ));
    }
    drop(fetched);

    let listing = itsanas_store::catalogue(store, vault)?;
    let candidates: Vec<Candidate<'_>> = listing
        .files
        .iter()
        .map(|known| Candidate {
            path: known.path.as_str(),
            size: known.size,
            modified_unix: known.modified_unix,
            here: known.presence == Presence::Local,
        })
        .collect();

    let choice = itsanas_policy::keeping::choose(&candidates, keeping);
    report.too_large = choice.left_because(Left::TooLarge);
    report.not_room = choice.left_because(Left::NoRoom);
    report.filtered = choice.left_because(Left::Filtered);

    let wanted_paths: BTreeSet<String> = choice
        .to_fetch(&candidates)
        .into_iter()
        .map(|index| candidates[index].path.to_owned())
        .collect();
    report.fetched = wanted_paths.len();

    let wanted = itsanas_store::chunks_for_all(store, vault, &wanted_paths)?;
    let pull = session::fetch_only(store, vault, client, &wanted)?;

    let freed = release_all(
        store,
        &choice.to_release(&candidates),
        &candidates,
        &mut report,
    )?;
    report.told_the_peer = tell_the_peer(store, client, &freed);

    Ok((session::RoundReport { push, pull }, report))
}

/// Let go of every file the choice left behind, and collect what was freed.
fn release_all(
    store: &Store,
    indices: &[usize],
    candidates: &[Candidate<'_>],
    report: &mut KeepingReport,
) -> Result<Vec<ChunkId>> {
    let now = itsanas_discover::now_unix();
    let mut freed = Vec::new();

    for index in indices {
        let path = candidates[*index].path;

        // The chunk ids have to be read before the release, because afterwards
        // the index no longer knows what the file was made of — and they are
        // what the peer has to be told.
        let was = store.stat(path)?.map(|entry| entry.chunks);

        match store.release(path, now)? {
            Release::Gone(outcome) => {
                report.released += 1;
                report.freed = report.freed.saturating_add(outcome.bytes);
                if let Some(chunks) = was {
                    freed.extend(chunks);
                }
            }
            // Refused because too few other live machines hold it. The budget
            // stays over its limit, which is the correct outcome: an over-full
            // device is a nuisance, and taking the number of copies below two
            // is not.
            Release::NotSafeYet { .. } => report.not_safe_yet += 1,
            Release::NotHere => {}
        }
    }

    Ok(freed)
}

/// Tell the peer which chunks this device no longer holds.
///
/// Best effort, and deliberately not fatal: an old peer answers `false` without
/// a request being sent, and a peer that fails mid-notice has still given this
/// device the sync it dialled for. The audit remains the backstop for both.
fn tell_the_peer(store: &Store, client: &mut PeerClient, freed: &[ChunkId]) -> bool {
    if freed.is_empty() {
        return false;
    }

    // A chunk another kept file still uses was never deleted, so saying it was
    // gone would understate this device's copies — the error in the direction
    // that causes needless repair traffic rather than false confidence, but an
    // error all the same.
    let gone: Vec<ChunkId> = freed
        .iter()
        .copied()
        .filter(|chunk| !store.has_chunk(chunk))
        .collect();

    let mut told = false;
    for batch in gone.chunks(MAX_HAVE_BATCH) {
        match client.dropped(store.owner(), batch.to_vec()) {
            Ok(accepted) => told |= accepted,
            Err(_) => return told,
        }
    }
    told
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
    use itsanas_net::{PeerClient, PeerServer, PeerService, Pledge};
    use itsanas_policy::keeping::{Keeping, Order};
    use itsanas_store::{ChunkerConfig, Presence, Store, Vault};

    use super::{KeepingReport, round};

    struct Machine {
        _dir: tempfile::TempDir,
        store: Store,
        vault: Vault,
        device: DeviceKeys,
    }

    fn machine(master: &MasterSecret, seed: u8) -> Machine {
        let dir = tempfile::tempdir().expect("temp dir");
        let device = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        let store = Store::open_for_testing(
            dir.path().join("store"),
            UserKeys::derive(master),
            DeviceKeys::from_seed(&device.seed()),
            ChunkerConfig::default(),
        )
        .expect("store");
        let vault = Vault::open(dir.path().join("vault")).expect("vault");
        Machine {
            _dir: dir,
            store,
            vault,
            device,
        }
    }

    /// Stops the accept loop even when the body panics, so a failed assertion
    /// fails instead of hanging — the same guard, for the same reason, as the
    /// one in `itsanas-net`'s two-node tests.
    struct StopOnDrop<'a>(&'a AtomicBool, std::net::SocketAddr);

    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
            let _ = std::net::TcpStream::connect(self.1);
        }
    }

    fn with_server<T>(host: &Machine, body: impl FnOnce(std::net::SocketAddr) -> T) -> T {
        let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
        let address = server.local_addr().expect("local address");
        let shutdown = AtomicBool::new(false);
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1));

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = server.serve_until(&service, &host.device, &shutdown);
            });
            let _stop = StopOnDrop(&shutdown, address);
            body(address)
        })
    }

    fn sync(small: &Machine, big: &Machine, keeping: &Keeping) -> KeepingReport {
        with_server(big, |address| {
            let mut client =
                PeerClient::connect(address, &small.device, small.store.owner(), None).unwrap();
            round(
                &small.store,
                &small.vault,
                keeping,
                &mut client,
                itsanas_net::session::Scope::Everything,
            )
            .expect("a selective round is not an error")
            .1
        })
    }

    /// Record a second live holder for everything this device holds.
    ///
    /// Releasing needs `SAFE_TO_RELEASE` other live machines, and these tests
    /// run against one server. A second peer would be a second `with_server`
    /// and a second thread to prove the same thing about the *choice*; the
    /// ledger is the input the decision reads, so it is written directly.
    /// `content_is_not_released_until_two_other_machines_have_it` is where the
    /// threshold itself is tested.
    fn note_a_second_holder(machine: &Machine) {
        let held: Vec<_> = machine
            .store
            .entries()
            .expect("entries")
            .into_iter()
            .flat_map(|(_, entry)| entry.chunks)
            .collect();
        let elsewhere = DeviceKeys::from_seed(&SecretBytes::new([0xEE; 32])).device_id();
        machine
            .store
            .record_holders(&held, &elsewhere)
            .expect("record");
    }

    fn tight() -> Keeping {
        // Smallest-first, and a budget that fits either the large file alone or
        // the small one, but not both. Ordering by size rather than by date
        // because `write_file` stamps the current second, and files written in
        // one test share it — a test whose ordering key is constant is not
        // testing an ordering.
        Keeping {
            budget: Some(250 * 1024),
            order: Order::Smallest,
            only: Vec::new(),
        }
    }

    /// A device short of room lets go of what it holds to make room for
    /// something the order ranks higher.
    ///
    /// The property that separates a budget from a ratchet, and the one the
    /// first version did not have: it filled up once and from then on nothing
    /// new could ever arrive, because nothing old could ever leave. Measured on
    /// a real device before this existed — told to keep 200 KiB, holding 907
    /// KiB, with no path back down.
    #[test]
    fn a_full_device_makes_room_for_a_better_ranked_file() {
        let master = MasterSecret::from_bytes([0x5C; 32]);
        let big = machine(&master, 80);
        let small = machine(&master, 81);
        let keeping = tight();

        big.store
            .write_file("big.bin", &vec![1u8; 200 * 1024])
            .expect("write");
        big.store.flush_segment().expect("flush");

        let first = sync(&small, &big, &keeping);
        assert_eq!(first.fetched, 1, "the first round fetched nothing");
        assert!(
            small.store.read_file("big.bin").expect("read").is_some(),
            "the only file that fits was not taken"
        );
        note_a_second_holder(&small);

        // A smaller file appears. It now outranks the large one, and there is
        // not room for both.
        big.store
            .write_file("small.bin", &vec![2u8; 100 * 1024])
            .expect("write");
        big.store.flush_segment().expect("flush");

        let second = sync(&small, &big, &keeping);

        assert_eq!(second.released, 1, "nothing was let go of to make room");
        assert!(
            second.freed >= 200 * 1024,
            "letting go of a 200 KiB file freed only {} bytes",
            second.freed
        );
        assert!(
            small.store.read_file("small.bin").expect("read").is_some(),
            "the better-ranked file never arrived"
        );
        assert!(
            small.store.read_file("big.bin").expect("read").is_none(),
            "the device kept both and is over its limit"
        );

        // Released, not deleted: the account still has it, and this device can
        // still see it and ask for it.
        let listing = itsanas_store::catalogue(&small.store, &small.vault).expect("catalogue");
        let released = listing
            .files
            .iter()
            .find(|file| file.path == "big.bin")
            .expect("the released file vanished from the account");
        assert_eq!(released.presence, Presence::Absent);
    }

    /// A device that lets go of content tells its peer, and the peer stops
    /// counting it as a copy.
    ///
    /// Without this the release makes the device a liar: every ledger that
    /// recorded it goes on reporting a copy that is not there, and the number
    /// somebody consults before believing their data is safe is inflated by
    /// exactly the machine that just made room. The audit would find it
    /// eventually — sixteen chunks per peer per round, which on a million-chunk
    /// account is most of a year.
    #[test]
    fn releasing_content_withdraws_this_device_from_the_peers_ledger() {
        let master = MasterSecret::from_bytes([0x6D; 32]);
        let big = machine(&master, 82);
        let small = machine(&master, 83);
        let keeping = tight();

        big.store
            .write_file("big.bin", &vec![3u8; 200 * 1024])
            .expect("write");
        big.store.flush_segment().expect("flush");
        let chunks = big
            .store
            .stat("big.bin")
            .expect("stat")
            .expect("here")
            .chunks;

        sync(&small, &big, &keeping);
        note_a_second_holder(&small);

        // The holder's own ledger learns the small device has them, the way it
        // does in production: by pushing, and being told there is nothing
        // missing.
        with_server(&small, |address| {
            let mut client =
                PeerClient::connect(address, &big.device, big.store.owner(), None).unwrap();
            itsanas_net::session::push_scoped(
                &big.store,
                &mut client,
                itsanas_net::session::Scope::Everything,
            )
            .expect("push");
        });

        let recorded = big
            .store
            .remote_holders(&chunks[0])
            .expect("holders")
            .into_iter()
            .filter(|holder| holder.device == small.device.device_id())
            .count();
        assert_eq!(
            recorded, 1,
            "the holder never recorded the small device, so this test proves nothing"
        );

        // Now the small device makes room, which means letting that file go.
        big.store
            .write_file("small.bin", &vec![4u8; 100 * 1024])
            .expect("write");
        big.store.flush_segment().expect("flush");

        let report = sync(&small, &big, &keeping);
        assert_eq!(report.released, 1, "nothing was released");
        assert!(report.told_the_peer, "the peer was never told");

        let still = big
            .store
            .remote_holders(&chunks[0])
            .expect("holders")
            .into_iter()
            .filter(|holder| holder.device == small.device.device_id())
            .count();
        assert_eq!(
            still, 0,
            "the peer still counts a device that let the chunk go as a copy"
        );
    }

    /// A device with no budget and no filter behaves exactly as it always did.
    #[test]
    fn a_machine_with_room_takes_the_ordinary_path() {
        let master = MasterSecret::from_bytes([0x7E; 32]);
        let big = machine(&master, 84);
        let laptop = machine(&master, 85);

        for name in ["one.bin", "two.bin", "three.bin"] {
            big.store
                .write_file(name, &vec![5u8; 100 * 1024])
                .expect("write");
        }
        big.store.flush_segment().expect("flush");

        let report = sync(&laptop, &big, &Keeping::everything());
        assert_eq!(report, KeepingReport::default(), "a laptop chose anything");
        assert_eq!(
            laptop.store.list().expect("list").len(),
            3,
            "a machine with room did not take the whole account"
        );
    }
}
