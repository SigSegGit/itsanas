//! Hosting for someone else: a stranger's data stored unread, relayed to a
//! device the host never met, and lost blocks fetched back from it.

use super::*;

#[test]
fn a_host_stores_a_strangers_data_and_cannot_read_a_byte_of_it() {
    // The bargain, over a real socket. Bob's laptop hosts Alice's data.
    let alice_keys = itsanas_testkit::alice();
    let alice_device = node(&alice_keys.master, 7);
    let bob_host = node(&MasterSecret::from_bytes([0xB0; 32]), 8);

    for file in &alice_keys.files {
        alice_device
            .store
            .write_file(file.path, &file.content)
            .unwrap();
    }
    alice_device.store.flush_segment().unwrap();

    with_server(&bob_host, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(
            address,
            &alice_device.device,
            alice_device.store.owner(),
            None,
        )
        .unwrap();

        let report = session::push(&alice_device.store, &mut client).expect("push to host");
        assert!(report.chunks_accepted > 0, "the host took nothing");
        assert!(report.segments_accepted > 0);
    });

    // Bob is now holding Alice's data.
    let stats = bob_host.vault.stats().unwrap();
    assert_eq!(stats.owners, 1);
    assert!(stats.chunks > 0);
    assert!(stats.bytes > 0);

    // Bob's own store must not have gained Alice's files. Checked against the
    // paths Alice actually wrote, rather than a name no store would ever hold,
    // and a read error is a failure rather than a pass: a red-team assertion a
    // broken store sails through is not an assertion.
    for file in &alice_keys.files {
        assert!(
            bob_host.store.read_file(file.path).unwrap().is_none(),
            "the host's own store gained Alice's file {}",
            file.path
        );
    }

    // And cannot read any of it.
    let mut everything = Vec::new();
    for address in bob_host
        .vault
        .chunks_for(alice_keys.keys.user_id())
        .unwrap()
    {
        let sealed = bob_host
            .vault
            .get_chunk(alice_keys.keys.user_id(), &address)
            .unwrap()
            .unwrap();

        everything.extend_from_slice(&sealed);
    }

    let canary = itsanas_testkit::ALICE_CANARY.as_bytes();
    assert!(
        !everything.is_empty(),
        "the host holds nothing, so this proves nothing"
    );
    assert!(
        !everything
            .windows(canary.len())
            .any(|window| window == canary),
        "Alice's plaintext is sitting on Bob's disk"
    );
}

#[test]
fn a_host_relays_one_device_to_another_that_it_never_met() {
    // The scenario the whole architecture exists for, over a socket this time.
    // The Pi pushes to a host and switches off. The VM, which has never spoken
    // to the Pi, pulls the Pi's work from that host.
    let host = node(&MasterSecret::from_bytes([0xB1; 32]), 9);
    let pi = node(&alice(), 10);
    let vm = node(&alice(), 11);

    pi.store
        .write_file("from-the-pi.txt", b"the Pi wrote this at 3am")
        .unwrap();
    pi.store.flush_segment().unwrap();

    with_server(&host, Pledge::gigabytes(1), |address| {
        // The Pi uploads, then conceptually powers off.
        let mut pi_client =
            PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::push(&pi.store, &mut pi_client).unwrap();
        drop(pi_client);

        // The VM arrives later and has never met the Pi.
        let mut vm_client =
            PeerClient::connect(address, &vm.device, vm.store.owner(), None).unwrap();
        let report = session::round(&vm.store, &vm.vault, &mut vm_client).unwrap();
        assert!(
            report.pull.adopted > 0,
            "the VM learned nothing from the host: {report:?}"
        );
    });

    assert_eq!(
        vm.store.read_file("from-the-pi.txt").unwrap().unwrap(),
        b"the Pi wrote this at 3am",
        "a host failed to relay one device's work to another"
    );
}

#[test]
fn a_disk_that_quietly_lost_a_block_gets_it_back_from_a_host() {
    // The half of repair that pushing cannot do. `push` restores *replication*
    // — it offers a peer what the peer lacks — and it can put nothing back on
    // this disk. A chunk missing here is the one failure the placement ledger
    // was built to survive, and until now surviving it meant a human running
    // `doctor`, reading the output, and knowing what to do next.
    //
    // A dropped block, an inode lost to a power cut, a backup restored
    // partially. The file is unreadable; the bytes are on three other machines;
    // nothing was reaching for them.
    let master = alice();
    let host = node(&MasterSecret::from_bytes([0xC1; 32]), 50);
    let owner = node(&master, 51);

    let chunks = owner
        .store
        .write_file("thesis.bin", &a_file_of_many_chunks(21, 512 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // The disk loses a block. Not deleted through the API: the index still
    // wants the chunk, which is exactly what makes this a fault rather than a
    // deletion.
    let lost = chunks[chunks.len() / 2];
    owner
        .store
        .blobs()
        .damage(&lost)
        .expect("remove the blob behind the store's back");

    assert!(
        owner.store.read_file("thesis.bin").is_err(),
        "the fixture did not actually break the file"
    );

    // Enough rounds for the bounded scan to reach it, wherever the cursor lands.
    let mut restored = 0;
    for _ in 0..12 {
        with_server(&host, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            restored += report.restored;
        });
        if restored > 0 {
            break;
        }
    }

    assert_eq!(restored, 1, "the lost chunk was never fetched back");
    assert!(
        owner.store.read_file("thesis.bin").expect("read").is_some(),
        "the chunk came back but the file is still unreadable"
    );
}

#[test]
fn red_team_a_stranger_is_not_told_which_chunks_this_node_has_lost() {
    // THE ATTACK, and it is one repair introduced. Asking a peer "do you have
    // chunk X?" tells it this node does not. The ids are blinded so it learns
    // nothing about the content — but it learns which chunks now exist only on
    // hosts, which is exactly the list to delete if you want to destroy
    // somebody's data. A healing mechanism that publishes the map of the
    // wounds.
    //
    // The first version asked every peer it connected to about everything it
    // had lost, strangers the discovery loop had dialled included.
    //
    // If this test fails, anyone who can complete a handshake with this node
    // learns which of its files are one deletion from being gone.
    let master = alice();
    let holder = node(&MasterSecret::from_bytes([0xD1; 32]), 70);
    let stranger = node(&MasterSecret::from_bytes([0xD2; 32]), 71);
    let owner = node(&master, 72);

    let chunks = owner
        .store
        .write_file("private.bin", &a_file_of_many_chunks(31, 256 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    // Only the real host is ever given the data, so only it is ever recorded.
    with_server(&holder, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // Lose several blocks, so the queue is not empty and the sweep cannot miss.
    for chunk in chunks.iter().take(3) {
        owner.store.blobs().damage(chunk).expect("lose a block");
    }
    let found = owner.store.verify_integrity(false).expect("doctor");
    assert_eq!(
        found.missing_chunks.len(),
        3,
        "the fixture did not break anything"
    );

    // The stranger answers the phone and is asked nothing, however many rounds
    // it hangs around for.
    for _ in 0..8 {
        with_server(&stranger, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            assert_eq!(
                report.asked, 0,
                concat!(
                    "a peer that has never been given a byte of this node's data ",
                    "was told which chunks it has lost"
                )
            );
            assert!(report.not_asked >= 3, "the losses were not even considered");
        });
    }

    // And the host that does hold them is still asked, or the rule would be
    // privacy bought by breaking the feature.
    let mut restored = 0;
    for _ in 0..4 {
        with_server(&holder, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            restored += report.restored;
        });
        if restored == 3 {
            break;
        }
    }
    assert_eq!(
        restored, 3,
        "the peer that does hold the data was not asked"
    );
}

#[test]
fn what_doctor_finds_is_what_repair_fixes_first() {
    // Two detectors that ignored each other. `doctor` walks every file and
    // knows the whole answer in one pass; the daemon's sampling scan reaches a
    // given chunk after fifty-five days on a terabyte store. Somebody running
    // `doctor` because a file would not open therefore learned the answer and
    // had no way to act on it, and the fix was the slowest path in the system.
    //
    // The queue is what joins them. This checks that a loss `doctor` found is
    // repaired without waiting for the sampler to stumble across it.
    let master = alice();
    let host = node(&MasterSecret::from_bytes([0xD3; 32]), 73);
    let owner = node(&master, 74);

    let chunks = owner
        .store
        .write_file("ledger.bin", &a_file_of_many_chunks(32, 256 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    let lost = chunks[chunks.len() - 1];
    owner.store.blobs().damage(&lost).expect("lose a block");
    assert_eq!(
        owner.store.loss_count().expect("count"),
        0,
        "nothing knows yet"
    );

    owner.store.verify_integrity(false).expect("doctor");
    assert_eq!(
        owner.store.loss_count().expect("count"),
        1,
        "doctor found the loss and told nobody"
    );

    // One round. Not a sweep: the queue means the sampler's odds do not matter.
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::repair(&owner.store, &mut client, 0).expect("repair");
        assert_eq!(
            report.restored, 1,
            "a loss doctor had already found was not repaired in the next round"
        );
    });

    assert_eq!(
        owner.store.loss_count().expect("count"),
        0,
        "the chunk came back and the queue still says it is missing"
    );
    assert!(owner.store.read_file("ledger.bin").expect("read").is_some());
}
