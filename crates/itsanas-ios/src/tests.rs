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
        exported.len() >= 14,
        "the source was not read: {exported:?}"
    );
    assert_eq!(
        declared, exported,
        "include/itsanas.h and src/lib.rs disagree on what Swift can call"
    );
}
