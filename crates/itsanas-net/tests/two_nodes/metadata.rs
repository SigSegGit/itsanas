//! Metadata-only rounds, for a phone on mobile data: what changed is learnt
//! and listed without being downloaded.

use super::*;

#[test]
fn a_metadata_round_learns_what_changed_without_downloading_it() {
    // What a phone on mobile data needs: know that work is waiting, keep the
    // verified segments so the next round on wifi resumes instead of starting
    // over, and download nothing.
    //
    // Nothing may be half-written. An operation is either applied with its
    // content or deferred, which is the same guarantee a sleeping peer gets.
    //
    // Note what is *not* asserted: that the file appears in a listing. It does
    // not, because a deferred operation writes no index entry, and pretending
    // otherwise here is how that gap would stay hidden.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks in it")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        let report = session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");

        assert_eq!(
            report.adopted, 0,
            "a metadata round materialised a file it never downloaded"
        );
        assert!(
            report.deferred > 0,
            "the operation was neither applied nor deferred, so it was lost"
        );
    });

    assert!(
        phone
            .store
            .read_file("notes/report.txt")
            .expect("read")
            .is_none(),
        "the file is readable after a round that never fetched its bytes"
    );

    // The segments are kept, so this node can relay them onwards and the next
    // round resumes rather than starting over.
    assert!(
        !phone
            .vault
            .heads_for(phone.store.owner())
            .expect("heads")
            .is_empty(),
        "the log segments were discarded, so the cheap half of the work was wasted"
    );

    // Back on wifi.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("full pull");
    });

    assert_eq!(
        phone
            .store
            .read_file("notes/report.txt")
            .expect("read")
            .as_deref(),
        Some(&b"a real file with real chunks in it"[..]),
        "the deferred operation never completed once the content was reachable"
    );
}

#[test]
fn a_metadata_round_offers_the_log_but_sends_no_chunks() {
    // The other direction, and the one that costs a phone money: a photo taken
    // on mobile data must not upload itself. The peer still learns that it
    // happened, so nothing is lost and the upload resumes on wifi.
    let master = alice();
    let phone = node(&master, 1);
    let host = node(&master, 2);

    phone
        .store
        .write_file(
            "photos/img.jpg",
            b"pretend this is four megabytes of photograph",
        )
        .expect("write");
    phone.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        let report = session::push_scoped(&phone.store, &mut client, session::Scope::Metadata)
            .expect("metadata push");

        assert!(
            report.segments_accepted > 0,
            "the peer was not even told that anything had happened"
        );
        assert_eq!(
            report.chunks_offered, 0,
            "a metadata push offered chunks, which is the upload it exists to avoid"
        );
        assert_eq!(report.chunks_accepted, 0);
        assert_eq!(
            report.holders_recorded, 0,
            "chunks were recorded as stored somewhere they were never sent"
        );
    });
}

#[test]
fn a_metadata_round_makes_the_file_listable_before_it_is_downloaded() {
    // The behaviour everyone expects from a phone client: everything is
    // listed, tapping one downloads it. Before the catalogue existed, a
    // metadata round left the file invisible — deferred means no index entry,
    // and `list` reports the index. A client could show only what it had
    // already downloaded, which on a metered connection is nothing.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("photos/holiday.jpg", b"pretend this is a large photograph")
        .expect("write");
    laptop
        .store
        .write_file("notes/todo.txt", b"milk")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    assert!(
        itsanas_store::catalogue(&phone.store, &phone.vault)
            .expect("catalogue")
            .files
            .is_empty(),
        "a phone that has synced nothing should know of nothing"
    );

    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");
    });

    // The index still holds nothing — no half-written state.
    assert_eq!(
        phone.store.list().expect("list"),
        [] as [std::string::String; 0]
    );

    let known = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    let paths: Vec<&str> = known.iter().map(|k| k.path.as_str()).collect();
    assert_eq!(paths, vec!["notes/todo.txt", "photos/holiday.jpg"]);
    assert!(
        known
            .iter()
            .all(|k| k.presence == itsanas_store::Presence::Absent),
        "nothing was downloaded, so nothing should claim to be here"
    );
    assert_eq!(
        known
            .iter()
            .find(|k| k.path == "notes/todo.txt")
            .expect("listed")
            .size,
        4,
        "the size comes from the log, so it is known before the content is"
    );

    // On wifi.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("content pull");
    });

    let known = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    assert_eq!(known.len(), 2, "the same two files, not four");
    assert!(
        known
            .iter()
            .all(|k| k.presence == itsanas_store::Presence::Local),
        "everything was downloaded, so nothing should still be marked absent"
    );
    assert_eq!(
        itsanas_store::absent_count(&phone.store, &phone.vault).expect("count"),
        0
    );
}

#[test]
fn a_round_that_deferred_nothing_does_not_replay_the_chain_next_time() {
    // The replay exists so deferred work is retried. Doing it unconditionally
    // turned the daemon's per-round cost from "the new segments" into "the
    // whole chain, times the number of peers" — a regression introduced with
    // the fix and measured afterwards.
    //
    // The marker is what makes it conditional, and it is only moved by a round
    // that finished everything. If this test fails in the "outstanding"
    // direction the cost regression is back; if it fails in the other, work
    // that could not finish is never looked at again.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("doc.txt", b"content")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    assert!(
        !phone.store.has_unapplied(&phone.vault).expect("check"),
        "an empty vault cannot have outstanding work"
    );

    // A metadata round takes the segments and finishes nothing.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");
    });

    assert!(
        phone.store.has_unapplied(&phone.vault).expect("check"),
        "segments were taken and never applied, which is the definition of outstanding"
    );

    // A content round finishes it.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        let report = session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("content pull");
        assert_eq!(report.deferred, 0);
    });

    assert!(
        !phone.store.has_unapplied(&phone.vault).expect("check"),
        "everything applied, so nothing is outstanding and the next round should not replay"
    );
    assert!(phone.store.read_file("doc.txt").expect("read").is_some());
}

#[test]
fn red_team_a_file_refused_for_room_is_fetched_once_room_is_made() {
    // A device short of disk defers the file rather than fill space its
    // pledge owes other people. The applied markers must stay where they
    // were: advanced past the deferred file, they would tell every later
    // round that the chain has nothing left to apply, and the file would
    // never arrive, even after the owner frees the space. The engine and
    // the node halves are tested apart; this drives both through the wire.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    let content = a_file_of_many_chunks(4, 300_000);
    laptop.store.write_file("big.bin", &content).expect("write");
    laptop.store.flush_segment().expect("flush");

    let ceiling = |bytes: Option<u64>| {
        phone
            .store
            .set_write_budget(Some(itsanas_store::WriteBudget {
                allowed: u64::MAX,
                elsewhere: 0,
                local_ceiling: bytes,
                folder_copy: itsanas_store::FolderCopy::None,
            }))
            .expect("budget");
    };
    let pull = || {
        with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &phone.device, phone.store.owner(), None)
                .expect("dial");
            session::pull(&phone.store, &phone.vault, &mut client).expect("pull")
        })
    };

    ceiling(Some(10_000));
    let report = pull();
    assert!(
        report.deferred > 0,
        "a file past the disk ceiling was not deferred: {report:?}"
    );
    assert!(phone.store.read_file("big.bin").expect("read").is_none());
    assert!(
        phone.store.has_unapplied(&phone.vault).expect("check"),
        "the round that deferred a file marked its chain applied"
    );

    ceiling(None);
    let report = pull();
    assert_eq!(report.deferred, 0, "{report:?}");
    assert_eq!(
        phone.store.read_file("big.bin").expect("read").as_deref(),
        Some(&content[..]),
        "room was made and the deferred file still never arrived"
    );
}
