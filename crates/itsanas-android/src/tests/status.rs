//! What a phone says about itself: safe, its vault, its silence, a failing
//! machine.

use super::*;

/// A phone's files with no copy elsewhere are the person MVP test L is about.
/// Sabotage: hard-code `safe` to true.
#[test]
fn red_team_a_phone_says_its_files_are_not_safe_yet() {
    let dir = tempfile::tempdir().unwrap();
    let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert_eq!(
        status["safe"], true,
        "an empty account was called unsafe: {status}"
    );

    let source = dir.path().join("source");
    std::fs::write(&source, b"kept on this phone alone").unwrap();
    node.store
        .write_stream("one.txt", std::fs::File::open(&source).unwrap())
        .unwrap();
    node.store.flush_segment().unwrap();

    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert_eq!(
        status["safe"], false,
        "files on this phone alone were called safe: losing it loses them, unwarned: {status}"
    );
    assert_eq!(status["copiesWanted"], 2, "{status}");
    assert_eq!(status["copiesElsewhere"], 0, "{status}");
}

/// The promise a phone cannot keep, said where its owner looks. Sabotage:
/// hard-code `vaultRefuses` to false.
#[test]
fn red_team_a_phone_says_its_vault_takes_no_write() {
    let dir = tempfile::tempdir().unwrap();
    let (mut node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    node.config.pledge_bytes = 100 << 20;
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert_eq!(
        status["vaultRefuses"], false,
        "a good vault failed: {status}"
    );

    let refusing = dir.path().join("not-a-directory");
    std::fs::write(&refusing, b"a file where the vault should be").unwrap();
    let real = node.config.vault.replace(refusing);
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    node.config.vault = real;
    assert_eq!(
        status["vaultRefuses"], true,
        "this phone goes on promising room every push to it fails on: {status}"
    );
}

/// A phone that reached nobody for a day says so, with no daemon and no icon.
/// The wait starts at a round with no machine to try. Sabotage: hard-code
/// `silentFor` to null, or drop the note on either path.
#[test]
fn red_team_a_phone_says_it_has_reached_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert!(
        status["silentFor"].is_null(),
        "a phone that has tried nothing says it reached nobody: {status}"
    );
    let started = std::fs::read_to_string(node.home.join("last-contact")).unwrap_or_default();
    assert!(
        started.contains("reached never since"),
        "looking at a phone starts no wait: if its scheduled sync never runs it reads as \
         fine for ever ({started:?})"
    );

    let two_days_ago = itsanas_discover::now_unix() - 2 * 86_400;
    assert!(sync_node(&node, true, two_days_ago).is_err());
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert!(
        status["silentFor"]
            .as_u64()
            .is_some_and(|s| s >= 2 * 86_400),
        "two days of reaching nobody and the phone says nothing: {status}"
    );
}

/// A machine that lied about what it holds, said on the phone from the same
/// rule as the desktop's icon. Sabotage: hard-code `failing` to 0.
#[test]
fn red_team_a_phone_says_a_machine_failed_its_storage_check() {
    let dir = tempfile::tempdir().unwrap();
    let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert_eq!(status["failing"], 0, "nobody failed anything: {status}");

    let liar = itsanas_crypto::DeviceId::from_bytes([7; 32]);
    node.store.note_audit(&liar, false).unwrap();
    let status: serde_json::Value = serde_json::from_str(&status_of(&node).unwrap()).unwrap();
    assert_eq!(
        status["failing"], 1,
        "a machine stopped holding this phone's files and the phone says nothing: {status}"
    );
}
