//! A host that lies: poisoned or noisy chunks, forged chains, rubbish in
//! answer to a repair. None of it is written down or stalls the round.

use super::*;

#[test]
fn red_team_a_relay_cannot_poison_a_chunk_on_the_ordinary_pull_path() {
    // THE ATTACK, through the door the repair defence did not cover.
    //
    // `Store::accept_chunk` verifies, and a red-team test proves a host cannot
    // answer a *repair* request with rubbish. But repair was never the only way
    // a peer's bytes reach this disk: every ordinary pull fetches chunks for
    // the operations it is applying, and that path used a second method which
    // wrote them unverified, arguing that a chunk which fails to open is caught
    // later by `read_file`.
    //
    // It is not caught later, and the reasoning against it was already written
    // down fifteen lines away. Noise on disk under a real address makes
    // `has_chunk` true, so nothing looks for the real bytes: not the repair
    // scan, which checks presence; not `doctor`, whose recorded loss the next
    // scan clears because the blob is now there. A recoverable loss made
    // permanent, on the path every chunk of every sync takes.
    //
    // If this test fails, any peer that relays for you can destroy any file of
    // yours, silently, by answering one fetch with the wrong bytes.
    let master = alice();
    let liar = node(&MasterSecret::from_bytes([0xE1; 32]), 80);
    let author = node(&master, 81);
    let victim = node(&master, 82);

    let content = a_file_of_many_chunks(41, 128 << 10);
    let chunks = author
        .store
        .write_file("thesis.bin", &content)
        .expect("write")
        .chunks;
    author.store.flush_segment().expect("flush");

    // The author uploads and goes away. The liar now holds the only copy the
    // victim can reach, which is the ordinary relay arrangement.
    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).expect("dial");
        session::push(&author.store, &mut client).expect("push");
    });

    // It substitutes noise of the right length for every chunk it holds.
    for chunk in &chunks {
        let sealed = liar
            .vault
            .get_chunk(author.store.owner(), chunk)
            .expect("vault")
            .expect("held");
        liar.vault
            .remove_chunk(author.store.owner(), chunk)
            .expect("remove");
        liar.vault
            .put_chunk(author.store.owner(), chunk, &vec![0x5Cu8; sealed.len()])
            .expect("substitute");
    }

    // The victim pulls, in the ordinary way, having never heard of repair.
    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &victim.device, victim.store.owner(), None).expect("dial");
        let _ = session::round(&victim.store, &victim.vault, &mut client);
    });

    for chunk in &chunks {
        assert!(
            !victim.store.has_chunk(chunk),
            concat!(
                "forged bytes were written under a real chunk address, so ",
                "nothing will ever look for the real ones: not the repair scan, ",
                "which checks presence, and not doctor, whose finding the next ",
                "scan clears because the blob is now there"
            )
        );
    }

    // The file is absent rather than corrupt, which is the whole difference:
    // absent is recoverable from any other holder, corrupt is for ever.
    assert!(
        victim
            .store
            .read_file("thesis.bin")
            .unwrap_or(None)
            .is_none(),
        "the victim reassembled a file out of the liar's noise"
    );
}

#[test]
fn red_team_a_relay_that_serves_noise_is_not_written_down_as_a_holder() {
    // THE ATTACK. The same relay as above, answering an ordinary pull with
    // noise. Refusing the bytes is not the end of it: the pull used to record
    // every chunk a peer *answered* as held by it, before anything checked
    // them, and to adopt the file as if they had arrived. The ledger then
    // counts a copy on the liar that does not exist, and repair asks the liar
    // first.
    let master = alice();
    let liar = node(&MasterSecret::from_bytes([0xE2; 32]), 83);
    let author = node(&master, 84);
    let victim = node(&master, 85);

    let content = a_file_of_many_chunks(43, 128 << 10);
    let chunks = author
        .store
        .write_file("thesis.bin", &content)
        .expect("write")
        .chunks;
    author.store.flush_segment().expect("flush");

    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).expect("dial");
        session::push(&author.store, &mut client).expect("push");
    });
    for chunk in &chunks {
        let sealed = liar
            .vault
            .get_chunk(author.store.owner(), chunk)
            .expect("vault")
            .expect("held");
        liar.vault
            .remove_chunk(author.store.owner(), chunk)
            .expect("remove");
        liar.vault
            .put_chunk(author.store.owner(), chunk, &vec![0x5Cu8; sealed.len()])
            .expect("substitute");
    }

    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &victim.device, victim.store.owner(), None).expect("dial");
        let _ = session::round(&victim.store, &victim.vault, &mut client);
    });

    for chunk in &chunks {
        assert!(
            victim
                .store
                .remote_holders(chunk)
                .expect("holders")
                .iter()
                .all(|holder| holder.device != liar.store.device_id()),
            "a relay that answered with noise was recorded as holding the chunk; \
             the ledger counts a copy that does not exist"
        );
    }
    assert!(
        victim.store.stat("thesis.bin").expect("stat").is_none(),
        "the file was adopted although not one of its chunks arrived; the round \
         read as finished and its markers moved past the segment"
    );
}

#[test]
fn red_team_one_forged_chain_does_not_stall_the_pull_of_the_others() {
    // THE ATTACK (§8 2c). A host serves the account's genuine chain from one
    // device and, beside it, a chain it made up: a fresh device key -- free,
    // anybody mints one -- signing a segment under the victim's user id whose
    // body is sealed under a key that is not the account's. The signature
    // verifies, so the vault keeps it; the body does not open. `apply_segments`
    // answered that with an error for the whole pull, so the genuine file was
    // never adopted, and since the forged segment stays in the vault and is
    // replayed, every later round failed the same way. One stranger, one
    // segment, an account's sync stopped on every node that pulled from it.
    //
    // What must happen instead: the genuine chain is applied, the forged one is
    // left out, and the round says a chain was refused.
    use itsanas_store::{LogEntry, Operation, SegmentEnvelope, VersionVector};

    let master = alice();
    let liar = node(&MasterSecret::from_bytes([0xE3; 32]), 86);
    let author = node(&master, 87);
    let victim = node(&master, 88);

    let content = a_file_of_many_chunks(47, 64 << 10);
    author
        .store
        .write_file("honest.bin", &content)
        .expect("write");
    author.store.flush_segment().expect("flush");

    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).expect("dial");
        session::push(&author.store, &mut client).expect("push");
    });

    // The forgery, under the victim's account id and a key nobody in it holds.
    let stranger = DeviceKeys::from_seed(&SecretBytes::new([0x01; 32]));
    let wrong_root = UserKeys::derive(&MasterSecret::from_bytes([0xE4; 32]));
    let forged = SegmentEnvelope::create(
        wrong_root.oplog_root(),
        author.store.owner(),
        &stranger,
        None,
        vec![LogEntry {
            sequence: 1,
            recorded_unix: 0,
            operation: Operation::Remove {
                path: "honest.bin".to_owned(),
                version: VersionVector::default(),
            },
        }],
    )
    .expect("forge");
    liar.vault
        .put_segment(&forged)
        .expect("a host checks the signature, not membership, so it keeps this");

    for attempt in ["first", "second"] {
        if attempt == "second" {
            // New honest work, so the second round has something new to
            // apply beside the forged chain that is still in the vault: the
            // refusal is said again, not once and then forgotten, and the new
            // file still arrives.
            author
                .store
                .write_file("later.bin", b"written after the first round")
                .expect("write");
            author.store.flush_segment().expect("flush");
            with_server(&liar, Pledge::gigabytes(1), |address| {
                let mut client =
                    PeerClient::connect(address, &author.device, author.store.owner(), None)
                        .expect("dial");
                session::push(&author.store, &mut client).expect("push");
            });
        }
        let report = with_server(&liar, Pledge::gigabytes(1), |address| {
            let mut client =
                PeerClient::connect(address, &victim.device, victim.store.owner(), None)
                    .expect("dial");
            session::round(&victim.store, &victim.vault, &mut client)
        })
        .unwrap_or_else(|error| {
            panic!(
                "the {attempt} round failed outright ({error}): one forged chain stalls \
                 every honest chain the same host serves"
            )
        });

        assert_eq!(
            report.pull.refused_chains, 1,
            "the {attempt} round left a chain out without saying so"
        );
        assert_eq!(
            report.pull.refused_devices,
            vec![stranger.device_id()],
            "the {attempt} round counted a refused chain without naming its device: \
             the owner cannot tell a forgery from a stale sibling of their own"
        );
        assert_eq!(
            victim
                .store
                .read_file("honest.bin")
                .expect("read")
                .as_deref(),
            Some(content.as_slice()),
            "the genuine device's file was not adopted beside the forged chain"
        );
    }
    assert!(
        victim.store.read_file("later.bin").expect("read").is_some(),
        "honest work written after the forgery was not adopted"
    );
}

/// The forgery of §8 2c: a free device key signing a segment under `owner`'s
/// account id, its body sealed under a key that is not the account's. The
/// signature verifies, so any vault keeps it; the body never opens.
fn a_forged_chain(owner: itsanas_crypto::UserId) -> (DeviceKeys, itsanas_store::SegmentEnvelope) {
    use itsanas_store::{LogEntry, Operation, SegmentEnvelope, VersionVector};

    let stranger = DeviceKeys::from_seed(&SecretBytes::new([0x02; 32]));
    let wrong_root = UserKeys::derive(&MasterSecret::from_bytes([0xE5; 32]));
    let forged = SegmentEnvelope::create(
        wrong_root.oplog_root(),
        owner,
        &stranger,
        None,
        vec![LogEntry {
            sequence: 1,
            recorded_unix: 0,
            operation: Operation::Remove {
                path: "honest.bin".to_owned(),
                version: VersionVector::default(),
            },
        }],
    )
    .expect("forge");
    (stranger, forged)
}

#[test]
fn red_team_a_forged_chain_pushed_into_the_vault_does_not_stall_drain_vault() {
    // THE ATTACK (§8 2c, the push door). A node behind NAT cannot be dialled,
    // so its siblings learn its work only from what it pushes into their
    // vaults, and `drain_vault` is the one thing that applies it. A stranger
    // with a free device key pushes one forged segment beside it. Applied as
    // one batch, the forgery failed the whole drain, on every daemon loop for
    // good: the daemon's only intake from that sibling, and backup restore's,
    // stopped by one segment.
    let master = alice();
    let victim = node(&master, 89);
    let author = node(&master, 90);

    author
        .store
        .write_file("honest.bin", b"pushed by a sibling nobody can dial")
        .expect("write");
    author.store.flush_segment().expect("flush");
    with_server(&victim, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).expect("dial");
        session::push(&author.store, &mut client).expect("push");
    });

    let (stranger, forged) = a_forged_chain(victim.store.owner());
    victim
        .vault
        .put_segment(&forged)
        .expect("a vault checks the signature, not membership, so it keeps this");

    let report = session::drain_vault(&victim.store, &victim.vault).unwrap_or_else(|error| {
        panic!(
            "draining the vault failed outright ({error}): one forged segment stops \
             every sibling behind NAT from ever reaching this node"
        )
    });

    assert_eq!(
        victim
            .store
            .read_file("honest.bin")
            .expect("read")
            .as_deref(),
        Some(b"pushed by a sibling nobody can dial".as_slice()),
        "the sibling's pushed file was not adopted beside the forged chain"
    );
    assert_eq!(
        report.refused_chains, 1,
        "the drain left a chain out without saying so"
    );
    assert_eq!(
        report.refused_devices,
        vec![stranger.device_id()],
        "the drain did not name the device whose chain it refused"
    );
}

#[test]
fn red_team_a_forged_chain_does_not_stop_fetch_only() {
    // THE ATTACK (§8 2c, the on-demand door). A device with a budget opens a
    // file by `chunks_for` then `fetch_only`: `itsanas get`, the phone's open,
    // a budgeted keeping round. Both read every chain in the vault, and the
    // forged one, kept by an earlier metadata round, sits there for good. It
    // failed both calls, so a file the account plainly had could not be
    // opened on any later day.
    let master = alice();
    let liar = node(&MasterSecret::from_bytes([0xE6; 32]), 91);
    let author = node(&master, 92);
    let victim = node(&master, 93);

    let content = a_file_of_many_chunks(48, 64 << 10);
    author
        .store
        .write_file("honest.bin", &content)
        .expect("write");
    author.store.flush_segment().expect("flush");
    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &author.device, author.store.owner(), None).expect("dial");
        session::push(&author.store, &mut client).expect("push");
    });
    let (stranger, forged) = a_forged_chain(victim.store.owner());
    liar.vault
        .put_segment(&forged)
        .expect("a host checks the signature, not membership, so it keeps this");

    let report = with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &victim.device, victim.store.owner(), None).expect("dial");
        // The list first, as a phone on a metered link does: the forged
        // segment is now in the victim's own vault.
        session::pull_scoped(
            &victim.store,
            &victim.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("a metadata round");

        let wanted: std::collections::BTreeSet<ChunkId> =
            itsanas_store::chunks_for(&victim.store, &victim.vault, "honest.bin")
                .unwrap_or_else(|error| {
                    panic!(
                        "looking the file up failed ({error}): `itsanas get` and the \
                         phone answer an error for a file the account has"
                    )
                })
                .expect("the genuine file is in the account's log")
                .into_iter()
                .collect();
        session::fetch_only(&victim.store, &victim.vault, &mut client, &wanted).unwrap_or_else(
            |error| {
                panic!(
                    "fetching the file failed outright ({error}): one forged chain \
                     stops every on-demand open"
                )
            },
        )
    });

    assert_eq!(
        victim
            .store
            .read_file("honest.bin")
            .expect("read")
            .as_deref(),
        Some(content.as_slice()),
        "the file asked for did not arrive beside the forged chain"
    );
    assert_eq!(
        report.refused_chains, 1,
        "the fetch left a chain out without saying so"
    );
    assert_eq!(
        report.refused_devices,
        vec![stranger.device_id()],
        "the fetch did not name the device whose chain it refused"
    );
}

#[test]
fn red_team_a_host_cannot_answer_a_repair_request_with_rubbish() {
    // THE ATTACK. A host cannot read what it stores, so the one way it could
    // destroy data is to wait until the owner asks for a chunk back and answer
    // with noise.
    //
    // If the bytes were written unverified, `has_chunk` would become true, the
    // repair scan would stop looking, no other peer would ever be asked, and a
    // loss that was **recoverable** would be permanent. That is strictly worse
    // than the host refusing to answer at all, which is the shape of failure
    // this project refuses everywhere else.
    //
    // If this test fails, any host you have ever stored with can destroy any
    // file of yours that your own disk has damaged.
    let master = alice();
    let liar = node(&MasterSecret::from_bytes([0xC2; 32]), 52);
    let owner = node(&master, 53);

    let chunks = owner
        .store
        .write_file("evidence.bin", &a_file_of_many_chunks(22, 256 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // Every chunk it holds is replaced with the same length of noise. It still
    // answers every request, promptly, with something.
    for chunk in &chunks {
        let sealed = liar
            .vault
            .get_chunk(owner.store.owner(), chunk)
            .expect("vault")
            .expect("held");
        liar.vault
            .remove_chunk(owner.store.owner(), chunk)
            .expect("remove");
        liar.vault
            .put_chunk(owner.store.owner(), chunk, &vec![0x7Au8; sealed.len()])
            .expect("substitute");
    }

    let lost = chunks[0];
    owner.store.blobs().damage(&lost).expect("lose a block");

    let mut forged = 0;
    for _ in 0..12 {
        with_server(&liar, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            assert_eq!(
                report.restored, 0,
                "rubbish was accepted as a repaired chunk"
            );
            forged += report.forged;
        });
        if forged > 0 {
            break;
        }
    }

    assert!(forged > 0, "the substitution was never even attempted");
    assert!(
        !owner.store.has_chunk(&lost),
        concat!(
            "the forged bytes were written under the missing chunk's address, ",
            "so the scan will stop looking for it and no other peer will ever ",
            "be asked. A recoverable loss has been made permanent."
        )
    );
    assert!(
        owner
            .store
            .remote_holders(&lost)
            .expect("holders")
            .iter()
            .all(|holder| holder.device != liar.store.device_id()),
        "a host that answered with rubbish is still recorded as holding it"
    );
}
