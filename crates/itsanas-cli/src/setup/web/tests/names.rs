//! The username and the space: a taken or invalid name said before anything
//! is made, the coordinator proposed, and the one rule for the space offered.

use super::*;

#[test]
fn the_page_learns_a_taken_username_before_anything_is_made() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let taken = page.api(
        "POST",
        "/api/name",
        "username=taken&coordinator=c.test%3A9898",
    );
    assert!(
        taken.status == 200 && taken.body.contains("\"taken\":\"yes\""),
        "a name the coordinator holds is not reported while it is typed: {}",
        taken.all()
    );
    let free = page.api(
        "POST",
        "/api/name",
        "username=camille&coordinator=c.test%3A9898",
    );
    assert!(
        free.body.contains("\"taken\":\"no\""),
        "a free name is reported as taken: {}",
        free.all()
    );
    assert!(
        !dir.path().join("node").join("keystore.bin").exists(),
        "asking about a name wrote a key"
    );
}

/// A name no coordinator would register reads as free on a lookup: the page
/// said so, keys were made, and registration refused it with an unrelated remedy.
/// `invalid` comes with the reason, the coordinator is not asked, and the page
/// blocks Next on it for a new account. Sabotage: drop the check in `check_name`
/// (`marie_1` is "free"); drop the branch in `validate`.
#[test]
fn red_team_the_page_says_when_no_coordinator_would_take_a_username() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    for (name, because) in [("marie_1", "lowercase"), ("-nope", "start or end")] {
        let reply = page.api(
            "POST",
            "/api/name",
            &format!("username={name}&coordinator=c.test%3A9898"),
        );
        assert!(
            reply.status == 200 && reply.body.contains("\"invalid\":\""),
            "{name} reads as a name the coordinator could give: {}",
            reply.all()
        );
        assert!(
            reply.body.contains(because),
            "{name} is refused with no reason a person can act on: {}",
            reply.all()
        );
    }
    let js = super::APP_JS;
    assert!(
        js.contains("if (nameInvalid) {\n        say(nameInvalid);"),
        "Next no longer stops an account on a name no coordinator would take"
    );
}

/// The question has no answer until the person gives one, and the one they are
/// likelier to need comes first: a preselected "new account" made a second
/// account for a person who meant to join their own (Nicolas, 2026-10-14).
/// Sabotage: `checked` on either radio; "new" first; `accountKind` falling back
/// on 'new' (Next goes on with an answer nobody gave).
#[test]
fn red_team_the_page_answers_nothing_for_the_person_and_offers_the_existing_account_first() {
    let html = include_str!("../index.html");
    let join = html
        .find(r#"name="account" value="join""#)
        .expect("the existing-account answer");
    let new = html
        .find(r#"name="account" value="new""#)
        .expect("the new-account answer");
    assert!(
        join < new,
        "a new account is offered before the existing one"
    );
    for line in html
        .lines()
        .filter(|line| line.contains(r#"name="account""#))
    {
        assert!(
            !line.contains("checked"),
            "an answer is chosen before the person is asked: {line}"
        );
    }
    let js = super::APP_JS;
    assert!(
        js.contains("return chosen ? chosen.value : null;"),
        "no answer reads as an answer"
    );
    assert!(
        js.contains("page === 'account' && !done.account && !accountKind()"),
        "Next goes on with neither answer chosen"
    );
}

#[test]
fn the_page_proposes_the_built_in_coordinator_and_the_code_s_split() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let state = page.api("GET", "/api/state", "");
    assert!(
        state.body.contains(&format!(
            "\"coordinator\":\"{}\"",
            crate::setup::DEFAULT_COORDINATOR
        )),
        "a new member is asked for a coordinator address nobody gave them: {}",
        state.body
    );
    let split = itsanas_coord::accounting::Split::DEFAULT;
    assert!(
        state.body.contains(&format!("\"split_own\":{}", split.own))
            && state
                .body
                .contains(&format!("\"split_network\":{}", split.network)),
        "the space page's \"you get\" would not be the code's bargain: {}",
        state.body
    );
}

#[test]
fn red_team_a_fixed_coordinator_is_the_one_the_name_check_asks_whatever_the_form_says() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start_with(&home, dir.path(), Mode::Setup, &Seen::default(), |b| {
        b.name = |address, name| Ok(address == "official.example:1" && name == "taken");
        b.fixed_coordinator = Some(("official.example:1".to_owned(), Some("pin".to_owned())));
    });
    let reply = page.api(
        "POST",
        "/api/name",
        "username=taken&coordinator=my-own-network.example:9",
    );
    assert!(
        reply.body.contains("\"yes\""),
        "the name was asked of the address in the form, not of the shell's fixed coordinator: a \
         phone could be pointed at a network of somebody else's naming ({})",
        reply.body
    );
    let state = page.api("GET", "/api/state", "");
    assert!(
        state.body.contains("official.example:1") && !state.body.contains("my-own-network"),
        "the page was offered a coordinator that is not the fixed one: {}",
        state.body
    );
}

#[test]
fn the_name_check_tells_a_person_joining_what_the_account_s_machines_offer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let asked = |name: &str, account: &str| {
        page.api(
            "POST",
            "/api/name",
            &format!("username={name}&coordinator=c.test%3A9898&account={account}"),
        )
        .body
    };
    let joining = asked("taken", "join");
    assert!(
        joining.contains("\"taken\":\"yes\"")
            && joining.contains(&format!("\"other_pledge_bytes\":{}", 40_u64 << 30)),
        "a person joining an account is not told what its machines offer: {joining}"
    );
    assert!(
        asked("taken", "new").contains("\"other_pledge_bytes\":null"),
        "a person making a new account was told what somebody else's machines offer"
    );
    assert!(
        asked("camille", "join").contains("\"other_pledge_bytes\":null"),
        "a figure was offered for a name no account holds"
    );
    let invalid = asked("marie_1", "new");
    assert!(
        invalid.contains("\"taken\":\"unknown\"")
            && invalid.contains("\"invalid\":\"")
            && invalid.contains("lowercase"),
        "a name no coordinator would take is not said to be invalid, with its reason: {invalid}"
    );
}

/// The owner's rule for every platform: no next step without a positive check.
/// The page asks again when Next is pressed, blocks a new account on anything but
/// a free name, and asks a person joining under an unknown name to say it is theirs.
/// Sabotage: `nameTaken !== 'no'` into `nameTaken === 'yes'`; drop `await checkName()`;
/// drop the tick's condition.
#[test]
fn red_team_a_new_account_goes_on_only_after_the_network_says_the_name_is_free() {
    let js = super::APP_JS;
    for (needle, consequence) in [
        (
            "await checkName();\n      const asked",
            "Next goes on with the answer to a name since changed: a quick Next passes the check",
        ),
        (
            "accountKind() === 'new' && asked && nameTaken !== 'no'",
            "a new account goes on under a name the network could not be asked about",
        ),
        (
            "accountKind() === 'join' && asked && nameTaken === 'no' && !$('continue-name').checked",
            "a person joins under a name no account has without ever saying it is theirs",
        ),
        (
            "$('continue-name').checked = false;",
            "the tick given to one name carries over to the next one typed",
        ),
    ] {
        assert!(js.contains(needle), "{consequence}");
    }
    let html = super::INDEX_HTML;
    assert!(
        html.contains("id=\"username-retry\"") && html.contains("id=\"continue-name\""),
        "the page has nowhere to retry a check or to tick the name"
    );
}

#[test]
fn the_space_question_is_answered_by_the_one_rule_every_platform_shares() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    // The disk is read here and by the page a moment apart, and a shared CI
    // runner's free space moves in between (an ARM run, 2026-11-23): the
    // page's answer must be the rule's for a reading taken just before or
    // just after it, never for none.
    let reading = || {
        let disk = itsanas_node::pledges::disk_space(dir.path());
        (disk.total.unwrap_or(0), disk.free.unwrap_or(0))
    };
    let says = |reply: &str, (total, free): (u64, u64), other: u64| {
        let expected = itsanas_node::pledges::suggest(
            &crate::config::Config::default(),
            total,
            free,
            0,
            other,
            other,
        );
        reply.contains(&format!("\"pledge_gb\":{}", expected.pledge >> 30))
            && reply.contains(&format!("\"keep_gb\":{}", expected.keep >> 30))
            && reply.contains(&format!(
                "\"same_as_other_machines\":{}",
                expected.same_as_other_machines
            ))
    };
    for other in [0_u64, 1 << 30, 10 << 40] {
        let before = reading();
        let reply = page
            .api("POST", "/api/suggest", &format!("other={other}"))
            .body;
        let after = reading();
        assert!(
            says(&reply, before, other) || says(&reply, after, other),
            "the page was suggested something the rule does not (other = {other}): {reply}"
        );
    }
    let before = reading();
    let state = page.api("GET", "/api/state", "").body;
    let after = reading();
    let first = |(total, free): (u64, u64)| {
        let none =
            itsanas_node::pledges::suggest(&crate::config::Config::default(), total, free, 0, 0, 0);
        state.contains(&format!("\"pledge_gb\":{}", none.pledge >> 30))
    };
    assert!(
        first(before) || first(after),
        "the first suggestion is not the rule's: {state}"
    );
}
