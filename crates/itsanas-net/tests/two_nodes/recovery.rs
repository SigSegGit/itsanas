//! Getting everything back: a replacement device pulls a whole corpus from a
//! stranger.

use super::*;

#[test]
fn a_replacement_device_pulls_a_whole_corpus_back_from_a_stranger() {
    replacement_pulls_a_whole_corpus_back(false);
}

/// The same restore when the lost machine placed its content by measured
/// availability: a stranger it never measured is outside the roster, and
/// still takes every chunk that had no proven copy anywhere.
#[test]
fn a_replacement_device_pulls_a_whole_corpus_back_from_a_stranger_in_measured_mode() {
    replacement_pulls_a_whole_corpus_back(true);
}

/// A plain push, or one placed by measured availability from `store`'s own
/// records, as the daemon does in `measured`.
fn push_measured_or_not(
    store: &Store,
    client: &mut PeerClient,
    measured: bool,
) -> session::PushReport {
    if !measured {
        return session::push(store, client).expect("push");
    }
    // Armed as the daemon arms it, and the census run over everything: the
    // chunks were written before the queue was armed, as on an upgrade.
    store.set_placement_measured(true).expect("arm");
    let placement = itsanas_net::Placement::build(
        store,
        std::collections::BTreeSet::new(),
        &std::collections::BTreeMap::new(),
        itsanas_store::now_unix(),
    )
    .expect("placement");
    placement.census(store, 1_000_000).expect("census");
    session::push_placed(store, client, session::Scope::Everything, Some(&placement)).expect("push")
}

fn replacement_pulls_a_whole_corpus_back(measured: bool) {
    // MVP acceptance test D, the half that was never checked. Recovery from
    // username and passphrase restores the *account* — the user id, the keys,
    // the ability to speak. It says nothing at all about whether the files come
    // back, and that is the only part the user cares about.
    //
    // So: a machine writes a corpus, edits one file, deletes another, uploads
    // to a host belonging to somebody else entirely, and is then destroyed. A
    // replacement device is built from the same master secret with a **new
    // device id**, has never met the machine that is gone, and pulls.
    //
    // What has to survive the trip: file contents byte for byte, an edit
    // arriving as the edit rather than the original, and a deletion arriving as
    // a deletion rather than as a resurrected file. The last is the one that
    // fails quietly — a restore that brings back everything you ever deleted
    // looks like it worked.
    let master = alice();
    let host = node(&MasterSecret::from_bytes([0xB7; 32]), 40);

    let corpus: Vec<(String, Vec<u8>)> = (0..12)
        .map(|index| {
            (
                format!("documents/report-{index:02}.bin"),
                a_file_of_many_chunks(index, 96 << 10),
            )
        })
        .collect();

    {
        let doomed = node(&master, 41);
        for (path, bytes) in &corpus {
            doomed.store.write_file(path, bytes).expect("write");
        }
        doomed
            .store
            .write_file("documents/report-00.bin", b"second thoughts")
            .expect("edit");
        doomed
            .store
            .remove_file("documents/report-11.bin")
            .expect("delete");
        doomed.store.flush_segment().expect("flush");

        with_server(&host, Pledge::gigabytes(1), |address| {
            let mut client =
                PeerClient::connect(address, &doomed.device, doomed.store.owner(), None)
                    .expect("dial");
            let report = push_measured_or_not(&doomed.store, &mut client, measured);
            assert!(report.chunks_accepted > 0, "the host took nothing");
        });
        // And now the machine is gone: its store, its blobs, its device key.
    }

    // A new machine. Same master secret, because that is what `itsanas login`
    // reconstructs from the passphrase; different device seed, because a
    // recovered install is a *new device* and must not pretend to be the one
    // that died.
    let replacement = node(&master, 42);
    assert!(
        replacement.store.list().expect("list").is_empty(),
        "the replacement was not actually empty"
    );

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(
            address,
            &replacement.device,
            replacement.store.owner(),
            None,
        )
        .expect("dial");
        let report =
            session::round(&replacement.store, &replacement.vault, &mut client).expect("round");
        assert!(
            report.pull.adopted > 0,
            "the replacement learned nothing from the host: {report:?}"
        );
        assert_eq!(
            report.pull.deferred, 0,
            "content was deferred on an unmetered round, so the restore is \
             incomplete and nothing said so"
        );
    });

    for (path, bytes) in &corpus {
        if path == "documents/report-11.bin" {
            assert_eq!(
                replacement.store.read_file(path).expect("read"),
                None,
                "a deleted file came back from the dead. A restore that \
                 resurrects everything you ever deleted looks exactly like a \
                 restore that worked."
            );
            continue;
        }
        let expected: &[u8] = if path == "documents/report-00.bin" {
            b"second thoughts"
        } else {
            bytes
        };
        assert_eq!(
            replacement.store.read_file(path).expect("read").as_deref(),
            Some(expected),
            "{path} did not come back intact"
        );
    }

    // The replacement now also knows *where* its data lives, without having
    // uploaded a byte. That is what makes the next round cheap instead of a
    // full re-upload of everything it just downloaded.
    assert!(
        replacement.store.stats().expect("stats").holder_records > 0,
        concat!(
            "a restored device does not know that the host holds its data, ",
            "so its next round will re-upload everything it just downloaded ",
            "from it"
        )
    );
}
