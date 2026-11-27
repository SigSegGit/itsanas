//! A write: what the machines lend, and a file put back under a trashed path.

use super::*;
use itsanas_cli::trash;

/// A write counts what the account's machines lend, asked when it is missing
/// or an hour old and not otherwise, so a file added offline does not wait on a
/// connect timeout every time. Sabotage: drop the call from `put_file` (a phone
/// on an account above 10 GiB is refused every write); drop the age check; leave
/// the file alone when the coordinator cannot say.
#[test]
fn red_team_a_write_asks_what_the_machines_lend_at_most_once_an_hour() {
    with_directory(|address, _| {
        let dir = tempfile::tempdir().unwrap();
        let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
        nas.config.pledge_bytes = 100 * GIB;
        itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
        let (me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));
        itsanas_node::coordinator::register_with(&me, None, NOW).unwrap();

        let source = dir.path().join("source");
        std::fs::write(&source, b"a photo").unwrap();
        put_file(&me, "one.jpg", source.to_str().unwrap()).unwrap();
        assert_eq!(
            Node::account_pledge(&me.home, &me.config),
            100 * GIB,
            "a write did not ask what the account's machines lend"
        );

        nas.config.pledge_bytes = 200 * GIB;
        itsanas_node::coordinator::register_with(&nas, None, NOW + 1).unwrap();
        put_file(&me, "two.jpg", source.to_str().unwrap()).unwrap();
        assert_eq!(
            Node::others_pledged(&me.home),
            100 * GIB,
            "every write asked, so every file added offline waits on a timeout"
        );

        let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
        std::fs::File::options()
            .write(true)
            .open(me.home.join("others-pledged"))
            .unwrap()
            .set_modified(two_hours_ago)
            .unwrap();
        put_file(&me, "three.jpg", source.to_str().unwrap()).unwrap();
        assert_eq!(
            Node::others_pledged(&me.home),
            200 * GIB,
            "an hour on, the phone still believes the old figure"
        );
    });

    let dir = tempfile::tempdir().unwrap();
    let (mut me, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    me.config.coordinator = Some("127.0.0.1:1".to_owned());
    refresh_old_pledges(&me, std::time::SystemTime::now());
    assert!(
        Node::others_pledged_age(&me.home, std::time::SystemTime::now()).is_some(),
        "nothing remembers that the coordinator was asked, so the next write waits too"
    );
    assert_eq!(Node::others_pledged(&me.home), 0);
}

/// A photo deleted, then added again under the same name, stayed hidden by the
/// trash and was deleted for good when the delay ran out. Sabotage: drop the
/// `restore_shared` in `write_file`.
#[test]
fn red_team_a_file_put_back_under_a_trashed_path_leaves_the_trash() {
    let dir = tempfile::tempdir().unwrap();
    let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    let source = dir.path().join("a.jpg");
    std::fs::write(&source, b"photo").unwrap();
    put_file(&node, "a.jpg", source.to_str().unwrap()).unwrap();
    itsanas_cli::phone::trash_put(&node, "a.jpg", 30).unwrap();
    std::fs::write(&source, b"the photo again").unwrap();
    put_file(&node, "a.jpg", source.to_str().unwrap()).unwrap();
    // At once, not at the next round, which also takes out a file edited
    // since it was trashed: until then every machine hides it.
    assert!(
        itsanas_store::deletion::shared(&node.store, &node.vault)
            .unwrap()
            .is_empty(),
        "a file added again stays in the account's trash, hidden on every machine, until the next round"
    );
    assert!(
        trash::list(&node.home).is_empty()
            && itsanas_cli::phone::purged_trash(&node).unwrap().is_empty(),
        "a file added again stays hidden, and is deleted for good when the old delay runs out"
    );
}
