//! Earlier versions brought back: from whichever machine holds them, never
//! from a host's noise, and never one whose chunks are gone.

use super::*;

/// `itsanas versions --restore` on a phone that holds an earlier version and
/// has only listed the newest: the earlier one becomes the file on the laptop
/// too, not a conflict copy beside the newest. Sabotage: date the version
/// back from what this device holds alone, without what the log shows.
#[test]
fn red_team_an_earlier_version_brought_back_on_a_phone_replaces_the_newest_everywhere() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("essay.txt", b"the good draft")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_all(&phone, &laptop);
    laptop
        .store
        .write_file("essay.txt", b"the draft that broke it")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_listing(&phone, &laptop);

    let listed =
        itsanas_store::versions(&phone.store, &phone.vault, "essay.txt").expect("versions");
    assert_eq!(listed.len(), 2, "both versions are not listed: {listed:?}");
    let good = listed
        .iter()
        .position(|version| version.size == 14)
        .expect("the good draft is listed");
    assert!(
        listed[good].here,
        "the version the phone holds is said not to be here"
    );
    assert!(
        itsanas_store::bring_back_version(&phone.store, &phone.vault, "essay.txt", good)
            .expect("bring back")
    );

    pull_all(&laptop, &phone);
    assert_eq!(
        laptop.store.read_file("essay.txt").expect("read"),
        Some(b"the good draft".to_vec()),
        "the version brought back on the phone lost to the one it was meant to replace"
    );
    assert_eq!(
        laptop.store.list().expect("list"),
        ["essay.txt"],
        "bringing a version back left a conflict copy beside it"
    );
}

/// A file deleted for good comes back from a machine that still holds its
/// chunks: nothing collects them but `itsanas gc`.
#[test]
fn a_file_deleted_for_good_comes_back_from_a_machine_that_still_holds_it() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("taxes.pdf", b"the only copy")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop.store.remove_file("taxes.pdf").expect("remove");
    laptop.store.flush_segment().expect("flush");

    let listed =
        itsanas_store::versions(&laptop.store, &laptop.vault, "taxes.pdf").expect("versions");
    assert_eq!(listed.len(), 1);
    assert!(
        !listed[0].current,
        "a deleted file's last version is called current"
    );
    assert!(
        itsanas_store::bring_back_version(&laptop.store, &laptop.vault, "taxes.pdf", 0)
            .expect("bring back")
    );
    pull_all(&phone, &laptop);
    assert_eq!(
        phone.store.read_file("taxes.pdf").expect("read"),
        Some(b"the only copy".to_vec()),
        "a file brought back after its delete did not reach the other machine"
    );
}

/// A version whose chunks were collected here is never published again:
/// every machine would list a file that opens nowhere. Sabotage: drop the
/// check that its chunks are here.
#[test]
fn red_team_a_version_whose_chunks_are_gone_is_never_brought_back() {
    let master = alice();
    let laptop = node(&master, 1);
    laptop
        .store
        .write_file("old.txt", b"collected since")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop.store.remove_file("old.txt").expect("remove");
    laptop.store.flush_segment().expect("flush");
    laptop
        .store
        .collect_garbage(std::time::Duration::ZERO)
        .expect("gc");

    let listed =
        itsanas_store::versions(&laptop.store, &laptop.vault, "old.txt").expect("versions");
    assert!(
        !listed[0].here,
        "a version whose chunks were collected is said to be here"
    );
    assert!(
        !itsanas_store::bring_back_version(&laptop.store, &laptop.vault, "old.txt", 0)
            .expect("bring back"),
        "a version with no chunks here was published again"
    );
    assert!(
        laptop.store.list().expect("list").is_empty(),
        "the account lists a file whose content is nowhere"
    );
}

/// A version this machine never held comes back from a machine that did: its
/// chunks are fetched, checked, then it is the file on every machine (§8 15).
/// Sabotage: `fetch_version` answers true without fetching.
#[test]
fn red_team_a_version_never_held_here_comes_back_from_another_machine() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("essay.txt", b"the good draft")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop
        .store
        .write_file("essay.txt", b"the draft that broke it")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_listing(&phone, &laptop);

    let listed =
        itsanas_store::versions(&phone.store, &phone.vault, "essay.txt").expect("versions");
    let good = listed
        .iter()
        .position(|version| version.size == 14)
        .expect("the good draft is listed");
    assert!(
        !listed[good].here,
        "the phone holds a version it never fetched"
    );
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        assert!(
            session::fetch_version(&phone.store, &phone.vault, &mut client, "essay.txt", good)
                .expect("fetch"),
            "the laptop holds every chunk of the version and none came"
        );
    });
    assert!(
        itsanas_store::bring_back_version(&phone.store, &phone.vault, "essay.txt", good)
            .expect("bring back"),
        "a version whose chunks were just fetched was not brought back"
    );
    pull_all(&laptop, &phone);
    assert_eq!(
        laptop.store.read_file("essay.txt").expect("read"),
        Some(b"the good draft".to_vec()),
        "the version brought back from the laptop's chunks is not the file"
    );
}

/// A machine that no longer holds the chunks cannot make the version come
/// back: nothing is published, the file stays as it is.
#[test]
fn a_version_no_machine_holds_stays_out() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    laptop
        .store
        .write_file("old.txt", b"collected since")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop.store.write_file("old.txt", b"newer").expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop
        .store
        .collect_garbage(std::time::Duration::ZERO)
        .expect("gc");
    pull_listing(&phone, &laptop);
    let listed = itsanas_store::versions(&phone.store, &phone.vault, "old.txt").expect("versions");
    let gone = listed
        .iter()
        .position(|version| version.size == 15)
        .expect("listed");
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        assert!(
            !session::fetch_version(&phone.store, &phone.vault, &mut client, "old.txt", gone)
                .expect("fetch"),
            "a version whose chunks are collected everywhere was said to be here"
        );
    });
    assert!(
        !itsanas_store::bring_back_version(&phone.store, &phone.vault, "old.txt", gone)
            .expect("bring back")
    );
}

/// The account's machines all gone, a version still comes back from a host of
/// another account that keeps its chunks; one that answers with noise brings
/// nothing back. Sabotage: store a fetched chunk without `accept_chunk`.
#[test]
fn red_team_a_version_comes_back_from_a_host_and_never_from_its_noise() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    let host = node(&MasterSecret::from_bytes([0xC3; 32]), 54);
    let liar = node(&MasterSecret::from_bytes([0xC4; 32]), 55);
    laptop
        .store
        .write_file("essay.txt", b"the good draft")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    for keeper in [&host, &liar] {
        with_server(keeper, Pledge::gigabytes(1), |address| {
            let mut client =
                PeerClient::connect(address, &laptop.device, laptop.store.owner(), None)
                    .expect("dial");
            session::push(&laptop.store, &mut client).expect("push");
        });
    }
    laptop
        .store
        .write_file("essay.txt", b"the draft that broke it")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    pull_listing(&phone, &laptop);
    let good = itsanas_store::versions(&phone.store, &phone.vault, "essay.txt")
        .expect("versions")
        .iter()
        .position(|version| version.size == 14)
        .expect("listed");
    let owner = laptop.store.owner();
    for chunk in itsanas_store::versions(&phone.store, &phone.vault, "essay.txt").expect("versions")
        [good]
        .chunks()
    {
        let sealed = liar
            .vault
            .get_chunk(owner, chunk)
            .expect("vault")
            .expect("held");
        liar.vault.remove_chunk(owner, chunk).expect("remove");
        liar.vault
            .put_chunk(owner, chunk, &vec![0x7A; sealed.len()])
            .expect("noise");
    }
    drop(laptop);

    let ask = |keeper: &Node| {
        let mut fetched = false;
        with_server(keeper, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &phone.device, phone.store.owner(), None)
                .expect("dial");
            fetched =
                session::fetch_version(&phone.store, &phone.vault, &mut client, "essay.txt", good)
                    .expect("fetch");
        });
        fetched
    };
    assert!(!ask(&liar), "a host's noise was taken for the version");
    assert!(
        !itsanas_store::bring_back_version(&phone.store, &phone.vault, "essay.txt", good)
            .expect("bring back"),
        "a version made of a host's noise was published to every machine"
    );
    assert!(
        ask(&host),
        "a host keeping every chunk of the version gave none"
    );
    assert!(
        itsanas_store::bring_back_version(&phone.store, &phone.vault, "essay.txt", good)
            .expect("bring back")
    );
    assert_eq!(
        phone.store.read_file("essay.txt").expect("read"),
        Some(b"the good draft".to_vec())
    );
}

/// A file listed here and on none of the account's machines that answer is
/// fetched from a host of another account that keeps its chunks.
#[test]
fn a_file_only_listed_here_is_fetched_from_a_host() {
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);
    let host = node(&MasterSecret::from_bytes([0xC5; 32]), 56);
    let chunks = laptop
        .store
        .write_file("photo.jpg", b"kept by a stranger")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        session::push(&laptop.store, &mut client).expect("push");
    });
    pull_listing(&phone, &laptop);
    drop(laptop);
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::fetch_only(
            &phone.store,
            &phone.vault,
            &mut client,
            &chunks.into_iter().collect(),
        )
        .expect("fetch");
    });
    assert_eq!(
        phone.store.read_file("photo.jpg").expect("read"),
        Some(b"kept by a stranger".to_vec()),
        "a host keeping every chunk of the file gave nothing"
    );
}
