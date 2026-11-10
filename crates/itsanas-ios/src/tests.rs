use super::*;
use std::sync::PoisonError;

/// Read and free an answer, as the Swift wrapper does.
fn take(answer: *mut c_char) -> serde_json::Value {
    assert!(!answer.is_null(), "an entry point answered null");
    let text = c::text(answer, "the answer").expect("an answer is UTF-8");
    itsanas_free(answer);
    serde_json::from_str(&text).expect("an answer is JSON")
}

#[test]
fn an_answer_is_an_envelope_the_app_can_read() {
    let rule = take(itsanas_passphrase_rule());
    let inner: serde_json::Value =
        serde_json::from_str(rule["ok"].as_str().expect("ok holds the answer")).expect("json");
    assert_eq!(
        inner["min"].as_u64(),
        Some(itsanas_node::node::MIN_PASSPHRASE_CHARS as u64),
        "the app would state a passphrase rule the core does not apply: {rule}"
    );
    let closed = take(itsanas_status());
    assert!(
        closed["error"]
            .as_str()
            .is_some_and(|said| !said.is_empty()),
        "a call with no account open did not say why it failed: {closed}"
    );
    assert!(closed.get("ok").is_none(), "{closed}");
}

#[test]
fn red_team_a_missing_string_is_refused_not_read() {
    // NULL for the passphrase must never become "": that would try the store
    // with the empty passphrase, and read as "wrong passphrase" to a person who
    // typed the right one.
    let home = c"/nonexistent/itsanas";
    let said = take(itsanas_open(home.as_ptr(), std::ptr::null()));
    assert!(
        said["error"]
            .as_str()
            .is_some_and(|error| error.contains("the passphrase was not given")),
        "a null passphrase was not refused for what it is: {said}"
    );
    assert!(
        !itsanas_exists(std::ptr::null()),
        "a null home was taken as a home"
    );
    itsanas_free(std::ptr::null_mut());
}

#[test]
fn a_page_with_no_such_name_is_refused() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let said = take(itsanas_page_start(
        c"/nonexistent/itsanas".as_ptr(),
        c"files".as_ptr(),
        false,
    ));
    assert!(
        said["error"]
            .as_str()
            .is_some_and(|error| error.contains("no page is called files")),
        "{said}"
    );
}

#[test]
fn red_team_a_left_pages_news_does_not_close_the_next_page() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    while host::next_request() != "{}" {}
    host::tell("event", &["closed", "left"]);
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().to_str().expect("a UTF-8 path");
    let home = std::ffi::CString::new(home).expect("a path");
    let said = take(itsanas_page_start(
        home.as_ptr(),
        c"settings".as_ptr(),
        false,
    ));
    assert!(said.get("ok").is_some(), "the page did not start: {said}");
    let news = host::next_request();
    take(itsanas_page_stop());
    assert_eq!(
        news, "{}",
        "the last page's closing was read as this one's: Settings closes as it opens"
    );
}

#[test]
fn a_trashed_file_leaves_the_list_comes_back_and_can_be_deleted() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    open_into(&NODE, &home, "a passphrase", &nowhere(), NOW).expect("open");
    let source = dir.path().join("photo");
    std::fs::write(&source, b"content").expect("source");
    let c_source = std::ffi::CString::new(source.to_str().expect("utf-8")).expect("path");
    let c_home = std::ffi::CString::new(home.to_str().expect("utf-8")).expect("path");
    let file = c"photo";
    take(itsanas_put(file.as_ptr(), c_source.as_ptr()));

    let listed = |needle: &str| {
        let answer = take(itsanas_list_at(c"".as_ptr()));
        answer["ok"].as_str().expect("ok").contains(needle)
    };
    assert!(listed("photo"), "the file added is not listed");
    take(itsanas_trash_put(file.as_ptr(), 30));
    assert!(!listed("photo"), "a file in the trash is still listed");
    let trash = take(itsanas_trash_list());
    assert!(
        trash["ok"].as_str().expect("ok").contains("photo"),
        "the trash does not show what was put in it: {trash}"
    );
    let node = NODE.lock().expect("node").clone().expect("open");
    assert!(
        trash_list(&node).expect("the trash").contains("photo"),
        "the trash both phones read does not show what was put in it"
    );
    drop(node);
    take(itsanas_trash_restore(c_home.as_ptr(), file.as_ptr()));
    assert!(listed("photo"), "a restored file did not come back");
    take(itsanas_trash_put(file.as_ptr(), 30));
    take(itsanas_trash_delete(file.as_ptr()));
    take(itsanas_trash_restore(c_home.as_ptr(), file.as_ptr()));
    assert!(
        !listed("photo"),
        "a file deleted from the trash came back when its record was taken out"
    );
    take(itsanas_close());
}

const NOW: u64 = 1_000_000;

fn nowhere() -> Network {
    Network {
        address: "127.0.0.1:1".to_owned(),
        device: None,
    }
}

#[test]
fn red_team_a_passphrase_that_does_not_open_is_not_remembered() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    keychain::forget();
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    let slot = Mutex::new(None);
    assert!(open_and_remember(&slot, &home, "not it", &nowhere(), NOW).is_err());
    assert!(
        keychain::unlock().is_none(),
        "a mistyped passphrase was remembered: the app would try it at every opening"
    );
    open_and_remember(&slot, &home, "a passphrase", &nowhere(), NOW).expect("open");
    assert_eq!(
        keychain::unlock().as_deref().map(String::as_str),
        Some("a passphrase"),
        "a passphrase that opened was not remembered: it is asked at every return to the app"
    );
    *slot.lock().expect("slot") = None;
    let said = open_remembered(&slot, &home, &nowhere(), NOW).expect("remembered");
    assert_eq!(
        said, r#"{"opened":true}"#,
        "the Keychain's passphrase opened nothing"
    );
    keychain::forget();
    let said = open_remembered(&slot, &home, &nowhere(), NOW).expect("nothing kept");
    assert_eq!(
        said, r#"{"opened":false}"#,
        "a forgotten passphrase still opened"
    );
}

#[test]
fn a_reply_to_nobody_is_not_taken() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let said = take(itsanas_reply(u64::MAX, c"words".as_ptr()));
    assert_eq!(
        said["ok"].as_str(),
        Some(r#"{"taken":false}"#),
        "the app was told a reply to no question was taken: {said}"
    );
}

/// The exported names, read from this crate's own source.
#[test]
fn a_background_pass_runs_with_touch_id_on() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    keychain::forget();
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    keychain::set("a passphrase").expect("kept");
    assert!(keychain::set_guarded(true), "Touch ID did not turn on");
    let slot = Mutex::new(None);
    let said = background_pass(&slot, &home, &nowhere(), NOW, true);
    let opened = slot.lock().expect("slot").is_some();
    keychain::forget();
    assert!(
        opened && said.is_err(),
        "no round with Touch ID on: whoever locks the screen stops syncing; said {said:?}"
    );
}

#[test]
fn red_team_touch_id_on_makes_the_unlock_screen_ask_for_a_finger() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    keychain::forget();
    assert!(!itsanas_finger_needed(), "a finger asked with the lock off");
    keychain::set("a passphrase").expect("kept");
    assert!(keychain::set_guarded(true), "Touch ID did not turn on");
    let needed = itsanas_finger_needed();
    keychain::forget();
    assert!(
        needed,
        "Touch ID on and no finger asked: the app opens for whoever holds the phone"
    );
}

#[test]
fn red_team_a_finger_enrolled_after_touch_id_was_turned_on_opens_nothing() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    keychain::forget();
    keychain::set("a passphrase").expect("kept");
    assert!(keychain::set_guarded(true), "Touch ID did not turn on");
    let first = std::ffi::CString::new("fingers A").expect("state");
    let later = std::ffi::CString::new("fingers A and B").expect("state");
    let seen = itsanas_finger_matches(first.as_ptr());
    let again = itsanas_finger_matches(first.as_ptr());
    let added = itsanas_finger_matches(later.as_ptr());
    let none = itsanas_finger_matches(c"".as_ptr());
    keychain::forget();
    assert!(
        seen && again,
        "the fingers Touch ID was turned on with no longer open the app"
    );
    assert!(
        !added && !none,
        "a finger enrolled later opens the app: whoever knows the phone's code adds theirs"
    );
}

#[test]
fn a_background_pass_opens_with_what_is_remembered_and_goes_for_a_round() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    keychain::forget();
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    let slot = Mutex::new(None);
    let said = background_pass(&slot, &home, &nowhere(), NOW, true).expect("nothing kept");
    assert_eq!(
        said, r#"{"ran":false}"#,
        "a phone with nothing kept claimed a round"
    );
    keychain::set("a passphrase").expect("kept");
    let failure = background_pass(&slot, &home, &nowhere(), NOW, true)
        .expect_err("a phone with no machine reached one");
    let opened = slot.lock().expect("slot").is_some();
    keychain::forget();
    assert!(
        matches!(failure, Failure::Usage(_)),
        "the pass stopped before its round: {failure:?}"
    );
    assert!(
        opened,
        "the pass closed the node itself: a person back on the screen would lose it"
    );
}

#[test]
fn a_background_pass_deletes_what_the_trash_has_held_long_enough() {
    let _turn = host::tests::SERIAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    keychain::forget();
    let dir = tempfile::tempdir().expect("scratch");
    let home = dir.path().join("phone");
    drop(Node::create(&home, "a passphrase", "camille").expect("create"));
    open_into(&NODE, &home, "a passphrase", &nowhere(), NOW).expect("open");
    let source = dir.path().join("photo");
    std::fs::write(&source, b"content").expect("source");
    let c_source = std::ffi::CString::new(source.to_str().expect("utf-8")).expect("path");
    let c_home = std::ffi::CString::new(home.to_str().expect("utf-8")).expect("path");
    let file = c"photo";
    take(itsanas_put(file.as_ptr(), c_source.as_ptr()));
    take(itsanas_trash_put(file.as_ptr(), 0));

    let _ = background_pass(&NODE, &home, &nowhere(), NOW, true);

    take(itsanas_trash_restore(c_home.as_ptr(), file.as_ptr()));
    let listing = take(itsanas_list_at(c"".as_ptr()));
    take(itsanas_close());
    assert!(
        !listing["ok"].as_str().expect("ok").contains("photo"),
        "a file past its days stayed: with the app closed the trash would never empty"
    );
}

fn exported() -> std::collections::BTreeSet<String> {
    include_str!("lib.rs")
        .lines()
        .filter_map(|line| line.strip_prefix("pub extern \"C\" fn "))
        .filter_map(|rest| rest.split('(').next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_header_declares_exactly_what_the_crate_exports() {
    // Swift sees only the header. A function exported and not declared is
    // dead; one declared and not exported links on the developer's Mac only if
    // it happens to be there, and fails at the first build elsewhere.
    let header = include_str!("../include/itsanas.h");
    let declared: std::collections::BTreeSet<String> = header
        .lines()
        .filter(|line| !line.starts_with(['/', ' ', '#']))
        .filter_map(|line| line.split('(').next())
        .filter_map(|head| head.rsplit([' ', '*']).next())
        .filter(|name| name.starts_with("itsanas_"))
        .map(str::to_owned)
        .collect();
    let exported = exported();
    assert!(
        exported.len() >= 19,
        "the source was not read: {exported:?}"
    );
    assert_eq!(
        declared, exported,
        "include/itsanas.h and src/lib.rs disagree on what Swift can call"
    );
}
