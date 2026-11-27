//! Unlocking a phone while the sync service already holds its node.

use super::*;

/// A coordinator nobody listens at, for the tests that must not reach the
/// official one.
fn nowhere() -> Network {
    Network {
        address: "127.0.0.1:1".to_owned(),
        device: None,
    }
}

/// The phone stuck on Unlock (2026-10-31): the sync service kept the node
/// open in the process, and opening it again was refused by the database
/// as "already open". A second open of the same home answers the node
/// already held. Sabotage: open before letting go of the held node, as the
/// code did, without the reuse -- the second open fails with the phone's
/// own words, "index.redb is already open in another process".
#[test]
fn red_team_unlocking_while_the_sync_service_holds_the_node_opens_nothing_twice() {
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    let slot = Mutex::new(None);
    let first = open_into(&slot, &home, "a passphrase", &nowhere(), NOW).expect("first open");
    let again = open_into(&slot, &home, "a passphrase", &nowhere(), NOW);
    assert!(
        again.is_ok(),
        "unlocking while the node is already open failed ({:?}): the phone stays \
         on Unlock and nothing can be used",
        again.err().map(|f| f.message())
    );
    assert_eq!(again.unwrap(), first);
}

/// The Unlock screen checks the passphrase when the node is already open:
/// what is typed there is judged by the keystore, whoever holds the node, and
/// the right one is answered as before. Sabotage: answer the held node before
/// the check.
#[test]
fn red_team_the_unlock_screen_checks_the_passphrase_when_the_node_is_already_open() {
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    let slot = Mutex::new(None);
    let first = open_into(&slot, &home, "a passphrase", &nowhere(), NOW).expect("first open");

    let wrong = open_into(&slot, &home, "not the passphrase", &nowhere(), NOW);
    assert!(
        matches!(wrong, Err(Failure::Node(NodeError::Unlock))),
        "the Unlock screen opened the account for a wrong passphrase: {:?}",
        wrong.map_err(|failure| failure.message())
    );
    assert!(
        slot.lock().unwrap().is_some(),
        "a wrong passphrase closed the node the sync service holds"
    );
    assert_eq!(
        open_into(&slot, &home, "a passphrase", &nowhere(), NOW).expect("right passphrase"),
        first
    );
}
