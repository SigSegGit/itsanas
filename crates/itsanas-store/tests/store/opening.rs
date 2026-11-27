//! Opening a store: a held one says so before any key is asked for, and
//! the published test identities are refused by the normal constructor.

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::Store;
use itsanas_testkit as testkit;

use crate::store_for;

/// A held store says so, without anybody being asked for a key first.
///
/// `itsanas status` prints the daemon's snapshot when the store is locked,
/// which is the normal state of a machine doing its job. It used to reach that
/// arm through an open that resolves the passphrase first, so on this laptop
/// `itsanas status` answered "no terminal to prompt on. Set
/// `ITSANAS_PASSPHRASE`" -- demanding the keystore secret in order to print a
/// plaintext file lying beside it. If this probe stops working the prompt comes
/// back, and with it a system whose owner cannot ask whether their data is safe
/// without unsealing their keys.
#[test]
fn red_team_a_held_store_says_so_before_anybody_is_asked_for_a_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("store");
    let master = MasterSecret::from_bytes([7; 32]);

    assert!(
        !Store::is_locked(&root),
        "a store that does not exist yet reported itself held, so `status` \
         would print a snapshot instead of saying there is no node here"
    );

    let held = store_for(&master, &root);
    assert!(
        Store::is_locked(&root),
        "a store this process is holding reported itself free; `status` would \
         go on to ask for a passphrase it does not need, which is the bug this \
         test exists for"
    );

    drop(held);
    assert!(
        !Store::is_locked(&root),
        "a released store still reported itself held, so `status` would print \
         a stale snapshot for a node nothing is running"
    );
}

#[test]
fn the_published_test_identities_are_refused_by_the_normal_constructor() {
    // README.md and SECURITY.md both promise this. Before this test existed the
    // check was defined, exported, and called by nothing.
    for user in testkit::everyone() {
        let dir = tempfile::tempdir().unwrap();
        let opened = Store::open(
            dir.path(),
            UserKeys::derive(&user.master),
            DeviceKeys::generate().unwrap(),
        );

        assert!(
            opened.is_err(),
            "{} opened a normal store despite having a published recovery \
             phrase; real data stored under it would be readable by anyone",
            user.username
        );
    }

    // An ordinary identity is unaffected.
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Store::open(
            dir.path(),
            UserKeys::derive(&MasterSecret::from_bytes([99; 32])),
            DeviceKeys::generate().unwrap(),
        )
        .is_ok(),
        "the ban list rejected an ordinary identity"
    );
}
