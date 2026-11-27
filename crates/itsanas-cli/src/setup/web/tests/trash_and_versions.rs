//! The trash, earlier versions, and the deletion rules set from Settings.

use super::*;

/// A page with a node in its slot, as a phone's or a computer's: `files` written
/// and sealed, `prepare` run on the node first.
fn page_with_node(
    dir: &Path,
    files: &[&str],
    prepare: impl FnOnce(&itsanas_node::Node),
) -> (Page, &'static super::Slot) {
    let home = dir.join("node");
    let (node, _) = itsanas_node::Node::create(&home, PASSPHRASE, "camille").expect("create");
    for path in files {
        node.store.write_stream(path, &b"bytes"[..]).expect("write");
    }
    node.store.flush_segment().expect("seal");
    prepare(&node);
    let slot: &'static super::Slot =
        Box::leak(Box::new(Mutex::new(Some(std::sync::Arc::new(node)))));
    let page = Page::start_with(&home, dir, Mode::Settings, &Seen::default(), move |b| {
        b.features = Features::DESKTOP;
        b.nodes = Arc::new(super::Held(slot));
    });
    (page, slot)
}

/// Wait for the page's job to end; its state.
fn job_ended(page: &Page) -> String {
    for _ in 0..200 {
        let state = page.api("GET", "/api/state", "");
        if !state.body.contains("\"phase\":\"running\"") {
            return state.body;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("the job never ended");
}

/// The trash in Settings: it lists the account's trash, restores from it, and
/// deletes for good only what is in it. Sabotage: drop the `stray` refusal in
/// `deleting::purge`.
#[test]
fn red_team_the_page_trash_acts_only_on_what_is_in_it() {
    use itsanas_store::deletion;
    let dir = tempfile::tempdir().expect("tempdir");
    let (page, slot) = page_with_node(dir.path(), &["a.txt", "b.txt"], |node| {
        deletion::trash_shared(&node.store, "a.txt", itsanas_store::now_unix(), 30).expect("trash");
        node.store.flush_segment().expect("seal");
        deletion::set_rules_here(
            &node.store,
            &node.vault,
            Some(&deletion::Rules {
                ask_auth: false,
                ..deletion::Rules::default()
            }),
        )
        .expect("rules");
    });
    let node = || slot.lock().expect("slot").clone().expect("node");

    let listed = page.api("GET", "/api/trash", "");
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert!(
        listed.body.contains("\"a.txt\"") && listed.body.contains("\"shared\""),
        "the account's trash shows nothing: {}",
        listed.body
    );

    let refused = page.api("POST", "/api/trash", "action=delete&paths=b.txt");
    assert_ne!(
        refused.status, 200,
        "the page deleted from every machine a file nobody put in the trash: {}",
        refused.body
    );
    assert!(
        node().store.tombstone("b.txt").expect("read").is_none(),
        "a file outside the trash was deleted for good"
    );

    let restored = page.api("POST", "/api/trash", "action=restore&paths=a.txt");
    assert_eq!(restored.status, 200, "{}", restored.body);
    assert!(
        deletion::shared(&node().store, &node().vault)
            .expect("shared")
            .is_empty(),
        "Restore left the file in the account's trash: it is deleted when the delay runs out"
    );

    deletion::trash_shared(&node().store, "a.txt", itsanas_store::now_unix(), 30).expect("again");
    let deleted = page.api("POST", "/api/trash", "action=delete&paths=a.txt");
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert!(
        node().store.tombstone("a.txt").expect("read").is_some(),
        "Delete for good left the file in the account"
    );
}

/// The page's file list shows a file's versions, brings an earlier one back on
/// every machine, and renames it (§8 15: the desktop page had neither).
/// Sabotage: make the `restore` action list instead of bringing back.
#[test]
fn red_team_the_page_brings_a_version_back_and_renames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (page, slot) = page_with_node(dir.path(), &["essay.txt"], |node| {
        node.store
            .write_file("essay.txt", b"the draft that broke it")
            .expect("edit");
        node.store.flush_segment().expect("seal");
    });
    let node = || slot.lock().expect("slot").clone().expect("node");

    let listed = page.api("POST", "/api/files", "action=versions&path=essay.txt");
    assert_eq!(listed.status, 200, "{}", listed.body);
    let reply: serde_json::Value = serde_json::from_str(&listed.body).expect("json");
    let versions = reply["versions"]["list"]
        .as_array()
        .expect("versions listed");
    assert_eq!(
        versions.len(),
        2,
        "both versions are not listed: {}",
        listed.body
    );
    let earlier = versions
        .iter()
        .position(|version| version["current"] == false)
        .expect("an earlier version");

    let back = page.api(
        "POST",
        "/api/files",
        &format!("action=restore&path=essay.txt&at={earlier}"),
    );
    assert_eq!(back.status, 200, "{}", back.body);
    assert_ne!(
        node().store.read_file("essay.txt").expect("read"),
        Some(b"the draft that broke it".to_vec()),
        "Bring back left the newest version as the file"
    );

    let moved = page.api(
        "POST",
        "/api/files",
        "action=rename&path=essay.txt&to=final.txt",
    );
    assert_eq!(moved.status, 200, "{}", moved.body);
    assert!(
        node().store.stat("final.txt").expect("stat").is_some()
            && node().store.stat("essay.txt").expect("stat").is_none(),
        "Rename did not move the file"
    );
    let refused = page.api("POST", "/api/files", "action=rename&path=final.txt&to=");
    assert_eq!(
        refused.status, 400,
        "an empty new name was accepted: {}",
        refused.body
    );
}

/// A rule that keeps less waits for the person to prove who they are, in the
/// engine's own window; one that keeps more is set at once. Sabotage: make
/// `deleting::needs_identity` answer false.
#[test]
fn red_team_a_rule_that_keeps_less_waits_for_who_you_are() {
    use itsanas_store::deletion;
    let dir = tempfile::tempdir().expect("tempdir");
    let (page, slot) = page_with_node(dir.path(), &[], |_| {});
    let rules = || {
        let node = slot.lock().expect("slot").clone().expect("node");
        deletion::rules_here(&node.store, &node.vault).expect("rules")
    };

    let more = page.api(
        "POST",
        "/api/deletion",
        "scope=here&mode=trash&receive=protect&days=60&ask_auth=true&confirm=true",
    );
    assert_eq!(more.status, 200, "{}", more.body);
    assert_eq!(rules().days, 60, "a longer wait was not kept");

    let less = page.api(
        "POST",
        "/api/deletion",
        "scope=here&mode=direct&receive=apply&days=60&ask_auth=true&confirm=true",
    );
    assert_eq!(
        less.status, 202,
        "a delete at once was set with nobody asked who they are: {}",
        less.body
    );
    assert!(less.body.contains("started"), "{}", less.body);
    let ended = job_ended(&page);
    assert!(ended.contains("\"phase\":\"done\""), "{ended}");
    assert_eq!(
        rules().mode,
        deletion::Mode::Direct,
        "the rule proved by the passphrase was not set"
    );
}

/// How long the traces of deletes are kept is set for the whole account from
/// Settings: a year until anybody chooses, keeping them longer (or for ever)
/// at once, keeping them for less time only once the person said who they
/// are, and nothing else but a duration or `never` taken. Kept less with
/// nobody asked, anybody at an unlocked screen could set ten days, switch a
/// machine off, and bring a deleted file back through it. Sabotage: answer
/// `traces_need_identity` false.
#[test]
fn red_team_traces_kept_for_less_time_wait_for_who_you_are() {
    use itsanas_store::traces::{self, Keep};
    let dir = tempfile::tempdir().expect("tempdir");
    let (page, slot) = page_with_node(dir.path(), &[], |_| {});
    let kept = || {
        let node = slot.lock().expect("slot").clone().expect("node");
        traces::account(&node.store, &node.vault).expect("traces")
    };
    let shown = page.api("GET", "/api/traces", "");
    assert_eq!(shown.status, 200, "{}", shown.body);
    assert!(
        shown.body.contains("\"days\":365"),
        "the default is not a year: {}",
        shown.body
    );
    assert!(
        shown.body.contains("1825"),
        "the durations offered are not sent: {}",
        shown.body
    );

    let refused = page.api("POST", "/api/traces", "keep=0");
    assert_eq!(
        refused.status, 400,
        "zero days was taken: every trace would go at once"
    );

    let more = page.api("POST", "/api/traces", "keep=never");
    assert_eq!(more.status, 200, "{}", more.body);
    assert_eq!(
        kept(),
        Keep::Forever,
        "keeping the traces for ever was not set"
    );

    let less = page.api("POST", "/api/traces", "keep=10");
    assert_eq!(
        less.status, 202,
        "the traces were kept for less time with nobody asked who they are: {}",
        less.body
    );
    assert_eq!(
        kept(),
        Keep::Forever,
        "the shorter choice was written before anybody proved who they are"
    );
    let ended = job_ended(&page);
    assert!(ended.contains("\"phase\":\"done\""), "{ended}");
    assert_eq!(
        kept(),
        Keep::For(10),
        "the choice proved by the passphrase was not set"
    );
}
