//! Where the data went: a round records which peer now holds this node's
//! data, and never a host that refused it.

use super::*;

#[test]
fn a_sync_round_records_which_peer_now_holds_this_nodes_data() {
    // The replacement for a coordinator-published node set. An owner already
    // keeps a log of their own chunks, so they can simply write down where they
    // put them — and then nothing has to agree with anybody about membership.
    //
    // Without this the repair loop has no idea whether a chunk exists anywhere
    // but on this disk, and the honest answer to "is my data safe?" is "no
    // idea".
    let master = alice();
    let laptop = node(&master, 1);
    let pi = node(&master, 2);

    let chunks = laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");
    assert_ne!(chunks, [] as [itsanas_crypto::ChunkId; 0]);

    for chunk in &chunks {
        assert!(
            laptop
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "nothing should be recorded before anything has been sent"
        );
    }

    with_server(&pi, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        let report = session::push(&laptop.store, &mut client).expect("push");
        assert!(report.chunks_accepted > 0, "the push sent nothing");
        assert_eq!(
            report.holders_recorded,
            laptop.store.blobs().addresses().expect("addresses").len(),
            "every offered chunk should be accounted for"
        );
    });

    for chunk in &chunks {
        let holders = laptop.store.remote_holders(chunk).expect("holders");
        assert_eq!(holders.len(), 1, "chunk {chunk:?} was not recorded");
        assert_eq!(
            holders[0].device,
            pi.store.device_id(),
            "the record names the wrong machine"
        );
    }

    assert!(
        laptop
            .store
            .under_replicated(2, itsanas_store::now_unix())
            .expect("risk")
            .is_empty(),
        "one remote holder plus this device meets a target of two"
    );
    assert!(
        !laptop
            .store
            .under_replicated(3, itsanas_store::now_unix())
            .expect("risk")
            .is_empty(),
        "a target of three is not met by one remote holder"
    );
}

#[test]
fn a_peer_that_already_had_the_data_is_still_recorded_as_holding_it() {
    // The property that makes the ledger converge rather than only grow. A
    // second round sends nothing, because the peer already has everything — and
    // the ledger must still come out of that round knowing the peer holds it.
    //
    // This is what a device restored from its recovery phrase depends on: it
    // learns where its data lives by asking, instead of re-uploading its entire
    // store to find out. The answer costs nothing extra, since it is the same
    // round trip that decides what to send.
    let master = alice();
    let laptop = node(&master, 1);
    let pi = node(&master, 2);

    let chunks = laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");

    with_server(&pi, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        session::push(&laptop.store, &mut client).expect("first push");
    });

    // Forget everything, as a freshly restored device would have.
    let dropped = laptop
        .store
        .forget_device(&pi.store.device_id())
        .expect("forget");
    assert!(dropped > 0);

    with_server(&pi, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        let report = session::push(&laptop.store, &mut client).expect("second push");

        assert_eq!(
            report.chunks_accepted, 0,
            "the peer already had everything, so nothing should have been sent"
        );
        assert_eq!(
            report.holders_recorded, dropped,
            "the ledger should have been rebuilt from what the peer said it had"
        );
    });

    for chunk in &chunks {
        assert_eq!(
            laptop.store.remote_holders(chunk).expect("holders").len(),
            1,
            "the ledger did not recover without re-uploading"
        );
    }
}

#[test]
fn a_host_that_refuses_to_store_is_not_recorded_as_holding_anything() {
    // A node that has pledged nothing still answers, because refusing to host
    // does not stop it being a peer. Recording it as a holder anyway would let
    // a node believe its data was replicated onto a machine that declined it —
    // the worst possible error, since it is indistinguishable from safety until
    // the day the local disk dies.
    let master = alice();
    let laptop = node(&master, 1);
    let stingy = node(&master, 2);

    let chunks = laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");

    with_server(&stingy, Pledge { bytes: 0 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        let report = session::push(&laptop.store, &mut client).expect("push");
        assert_eq!(
            report.chunks_accepted, 0,
            "a node pledging nothing accepted data"
        );
        assert_eq!(report.holders_recorded, 0);
    });

    for chunk in &chunks {
        assert!(
            laptop
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "a refusal was recorded as safe storage"
        );
    }
    assert!(
        laptop
            .store
            .under_replicated(2, itsanas_store::now_unix())
            .expect("risk")
            .iter()
            .all(AtRisk::only_copy),
        "every chunk should still be reported as existing only here"
    );
}
