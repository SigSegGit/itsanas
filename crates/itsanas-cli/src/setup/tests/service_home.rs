//! The service and its passphrase: signing out and in, a home the service
//! would not run, and a service that keeps the passphrase its own way.

use super::*;

#[test]
fn red_team_sign_out_forgets_the_passphrase_and_sign_in_needs_the_right_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    Node::create(&home, PASSPHRASE, "camille").expect("create");
    let sealed = std::fs::read(keystore(&home)).expect("keystore");
    let service = Recording {
        file: dir.path().join("service").join("passphrase"),
        calls: RefCell::new(Vec::new()),
    };
    super::service::write_passphrase_file(&service.file, PASSPHRASE).expect("write");

    sign::sign_out(&home, &service).expect("sign out");
    assert!(
        !service.file.exists(),
        "signed out, and the passphrase file is still there: the machine handed to somebody \
         else still unlocks the account at the next logon"
    );
    assert_eq!(
        *service.calls.borrow(),
        ["autostart off", "stop"],
        "sign out left the service to start again at logon, with no passphrase to unlock"
    );
    assert_eq!(
        std::fs::read(keystore(&home)).expect("keystore"),
        sealed,
        "sign out touched the keys, which it promises to keep"
    );

    service.calls.borrow_mut().clear();
    let refused = sign::sign_in(&home, &service, &mut Typing("not the passphrase"));
    assert!(
        matches!(refused, Err(CliError::Unlock)),
        "a wrong passphrase was accepted at sign in: {refused:?}"
    );
    assert!(
        !service.file.exists() && service.calls.borrow().is_empty(),
        "a wrong passphrase was saved for the service, which would then fail at every logon"
    );

    sign::sign_in(&home, &service, &mut Typing(PASSPHRASE)).expect("sign in");
    assert_eq!(
        super::service::read_passphrase_file(&service.file)
            .as_deref()
            .map(String::as_str),
        Some(PASSPHRASE),
        "signed in, and the service has no passphrase to start with"
    );
    assert_eq!(
        *service.calls.borrow(),
        ["autostart on", "start"],
        "signed in, and the service was not started again"
    );
}

#[test]
fn red_team_the_service_is_never_installed_for_a_home_it_would_not_run() {
    // The task, the unit and the plist run the node `--instance` names, not
    // the `--home` setup was given: installed for a throwaway home, the
    // service would start some other node, or none, and setup would say done.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let service = Recording {
        file: dir.path().join("service").join("passphrase"),
        calls: RefCell::new(Vec::new()),
    };
    let mut wanted = answers(dir.path());
    wanted.service = true;
    let (mut person, _) = Scripted::new(false);
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record).run();
    assert_eq!(
        outcome.failed,
        Some(Step::Service),
        "setup on a home no service runs did not stop at the service: {events:#?}"
    );
    assert!(
        !service.calls.borrow().contains(&"install"),
        concat!(
            "a service was installed for a home it does not run: the person is told the ",
            "machine works while no daemon ever opens this node"
        )
    );
}

#[test]
fn red_team_joining_from_a_coordinator_refuses_a_passphrase_the_service_file_cannot_hold() {
    let base = tempfile::tempdir().expect("tempdir");
    let home = base.path().join("home");
    let service = Recording {
        file: base.path().join("passphrase.txt"),
        calls: RefCell::new(Vec::new()),
    };
    let mut answers = answers(base.path());
    answers.account = Some(Account::Join {
        username: "camille".to_owned(),
        // Nothing listens here: the refusal must come before any network.
        from: Some("127.0.0.1:1".to_owned()),
    });
    answers.service = true;
    // A line break is refused on every system, so this runs the same on all.
    let mut prompt = Typing("line\nbreak");
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, answers, &mut prompt, &service, &mut record).run();
    assert_eq!(outcome.failed, Some(Step::Account));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Failed { error, .. } if error.contains("line break"))
        ),
        "a passphrase the service file cannot hold reached the coordinator: the keystore is \
         restored under it and the Secret step then fails on every run"
    );
    assert!(
        !Node::exists(&home),
        "a keystore was written under a passphrase no service can use"
    );
}

#[test]
fn red_team_a_service_that_remembers_the_passphrase_its_own_way_is_never_given_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    Node::create(&home, PASSPHRASE, "camille").expect("create");
    let service = Remembering {
        file: dir.path().join("service").join("passphrase"),
        kept: RefCell::new(Some(PASSPHRASE.to_owned())),
    };

    sign::sign_out(&home, &service).expect("sign out");
    assert_eq!(
        service.stored_passphrase(),
        None,
        "signed out, and the remembered passphrase is still there: the phone lent to somebody \
         else unlocks the account"
    );

    sign::sign_in(&home, &service, &mut Typing(PASSPHRASE)).expect("sign in");
    assert_eq!(
        service.stored_passphrase().as_deref().map(String::as_str),
        Some(PASSPHRASE),
        "signed in, and nothing remembers the passphrase for the service"
    );
    assert!(
        !service.file.exists() && !service.file.with_extension("new").exists(),
        "sign in wrote a passphrase file for a service that keeps it elsewhere: a copy of the \
         secret outside the keystore, readable by whatever runs as the app"
    );
}

#[test]
fn the_default_passphrase_methods_read_write_and_forget_today_s_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let service = NoService {
        file: dir.path().join("service").join("passphrase"),
    };
    assert_eq!(service.stored_passphrase(), None);
    let secret = Zeroizing::new(PASSPHRASE.to_owned());
    service.store_passphrase(&secret).expect("store");
    assert_eq!(
        super::service::read_passphrase_file(&service.file)
            .as_deref()
            .map(String::as_str),
        Some(PASSPHRASE),
        "a desktop's remembered passphrase is not in the file the service reads"
    );
    assert_eq!(
        service.stored_passphrase().as_deref().map(String::as_str),
        Some(PASSPHRASE)
    );
    assert_eq!(
        service.forget_passphrase().expect("forget"),
        "the passphrase file is deleted"
    );
    assert_eq!(
        service.forget_passphrase().expect("forget again"),
        "there was no passphrase file to delete"
    );
}
