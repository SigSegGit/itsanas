//! The listing: a file this device made and released is still listed, one
//! deleted stays deleted, and a chain left out is named.

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::Operation;
use itsanas_testkit as testkit;

use crate::store_for;

/// A file this device created and then released is still listed, and can still
/// be fetched.
///
/// Found on a Raspberry Pi, not in a test: a 200 KiB file put on a device with
/// a 300 KiB limit, pushed to two hosts, released exactly as designed — and
/// then gone from `itsanas ls` on the machine that had made it, with `itsanas
/// get` answering "no such file" for a file two other machines were holding.
///
/// The cause was a reasonable-sounding rule that stopped being true. The
/// catalogue walked only *other* devices' chains, because the index is the
/// authority for anything this machine wrote. Releasing content removes the
/// index entry, so what this device wrote and let go of was in no log the walk
/// read.
#[test]
fn a_file_this_device_made_and_released_is_still_listed_and_still_fetchable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());
    let vault = itsanas_store::Vault::open(dir.path().join("vault")).expect("vault");

    let payload = vec![9u8; 200_000];
    store.write_file("mine.bin", &payload).expect("write");
    store.flush_segment().expect("flush");

    let chunks = store.stat("mine.bin").expect("stat").expect("here").chunks;
    for _ in 0..itsanas_store::holders::SAFE_TO_RELEASE {
        let host = DeviceKeys::generate().expect("device key").device_id();
        store.record_holders(&chunks, &host).expect("record");
        store.note_audit(&host, true).expect("audit");
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    match store.release("mine.bin", now).expect("release") {
        itsanas_store::Release::Gone(_) => {}
        other => panic!("release refused with two live holders: {other:?}"),
    }

    // The vault is empty: no other device has ever spoken to this one. The
    // listing must still show the file.
    let listing = itsanas_store::catalogue(&store, &vault).expect("catalogue");
    let known = listing
        .files
        .iter()
        .find(|file| file.path == "mine.bin")
        .expect("a file this device made vanished from its own listing");
    assert_eq!(known.presence, itsanas_store::Presence::Absent);
    assert_eq!(known.size, 200_000);

    // And it must be fetchable, which means its chunks have to resolve.
    let wanted = itsanas_store::chunks_for(&store, &vault, "mine.bin")
        .expect("chunks_for")
        .expect("`itsanas get` would have answered \"no such file\"");
    assert_eq!(wanted, chunks);
}

/// A file this device created, released, and then genuinely deleted stays
/// deleted.
///
/// The other half of the rule above. Now that the walk reads this device's own
/// chain, a `Remove` in that chain has to still win — otherwise reading one's
/// own log resurrects every file one has ever deleted.
#[test]
fn a_deleted_file_is_not_resurrected_by_reading_this_devices_own_log() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::generate().expect("master secret");
    let store = store_for(&master, dir.path());
    let vault = itsanas_store::Vault::open(dir.path().join("vault")).expect("vault");

    store
        .write_file("gone.txt", b"here for now")
        .expect("write");
    store.flush_segment().expect("flush");
    assert!(store.remove_file("gone.txt").expect("remove"));
    store.flush_segment().expect("flush");

    let listing = itsanas_store::catalogue(&store, &vault).expect("catalogue");
    assert!(
        !listing.files.iter().any(|file| file.path == "gone.txt"),
        "a deleted file came back into the listing"
    );
    assert!(
        itsanas_store::chunks_for(&store, &vault, "gone.txt")
            .expect("chunks_for")
            .is_none(),
        "a deleted file could still be fetched"
    );
}

/// A chain left out of the listing makes it say so, and names the device.
///
/// The walk leaves out another device's chain whose segment does not open, so
/// one forgery in the vault cannot fail every listing for good. The same skip
/// takes a genuine sibling's segment damaged on this disk, and a listing that
/// then called itself complete would show that sibling's deleted files as
/// present and its new ones as gone, with nothing to say why. Sabotage: drop
/// `complete = false` or `left_out.push(device)` in `walk_vault`.
#[test]
fn red_team_a_chain_the_listing_leaves_out_marks_it_incomplete_and_is_named() {
    use itsanas_crypto::SecretBytes;
    use itsanas_store::{LogEntry, SegmentEnvelope, VersionVector};

    let master = testkit::alice().master;
    let here = tempfile::tempdir().expect("temp dir");
    let store = store_for(&master, here.path());
    let vault = itsanas_store::Vault::open(here.path().join("vault")).expect("vault");

    // A sibling's honest chain, as its push would leave it in this vault.
    let elsewhere = tempfile::tempdir().expect("temp dir");
    let sibling = store_for(&master, elsewhere.path());
    sibling
        .write_file("sibling.txt", b"written on the laptop")
        .expect("write");
    sibling.flush_segment().expect("flush");
    for envelope in sibling.segments().expect("segments") {
        vault
            .put_segment(&envelope)
            .expect("an honest segment is kept");
    }

    // And one chain whose body does not open under this account's key.
    let stranger = DeviceKeys::from_seed(&SecretBytes::new([0x5A; 32]));
    let wrong_root = UserKeys::derive(&MasterSecret::from_bytes([0x5B; 32]));
    let unopenable = SegmentEnvelope::create(
        wrong_root.oplog_root(),
        store.owner(),
        &stranger,
        None,
        vec![LogEntry {
            sequence: 1,
            recorded_unix: 0,
            operation: Operation::Remove {
                path: "sibling.txt".to_owned(),
                version: VersionVector::default(),
            },
        }],
    )
    .expect("segment");
    vault
        .put_segment(&unopenable)
        .expect("a vault checks the signature, not membership, so it keeps this");

    let listing = itsanas_store::catalogue(&store, &vault)
        .expect("one unopenable chain failed the whole listing");
    assert!(
        listing.files.iter().any(|file| file.path == "sibling.txt"),
        "the honest sibling's file went missing beside the chain left out"
    );
    assert!(
        !listing.complete,
        "a listing that left a device's chain out called itself complete: that device's \
         files and deletes vanish without a word"
    );
    assert_eq!(
        listing.left_out,
        vec![stranger.device_id()],
        "the listing did not name the device whose chain it left out"
    );
}
