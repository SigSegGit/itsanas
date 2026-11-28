//! Binding an address, and what a verified one changes for a recovery.

use super::*;
use crate::mail::{Mail, Outbox};
use crate::protocol::EmailStatus;

/// An enrolled member with an escrow container, its name and its device.
fn member_with_escrow(directory: &Directory, seed: u8) -> (String, DeviceId) {
    let owner = user(seed);
    let keys = device(seed);
    enrol(directory, &owner, &keys);
    let service = CoordService::new(directory);
    assert_eq!(
        ask(
            &service,
            &Request::PutEscrow {
                blob: Some(vec![seed; 96])
            },
            keys.device_id()
        ),
        Response::Done
    );
    (
        format!("member{}", keys.device_id().short()),
        keys.device_id(),
    )
}

/// The digits of the one mail waiting.
fn code_in(receiver: &std::sync::mpsc::Receiver<Mail>) -> String {
    digits_of(&receiver.try_recv().expect("a mail was queued"))
}

/// The eight digits after "code is" in `mail`.
fn digits_of(mail: &Mail) -> String {
    let at = mail.text.find("code is ").expect("a code in the mail") + "code is ".len();
    mail.text[at..at + 8].to_owned()
}

fn bind_and_verify(
    service: &CoordService<'_>,
    caller: DeviceId,
    receiver: &std::sync::mpsc::Receiver<Mail>,
) {
    bind_and_verify_at(service, caller, receiver, NOW);
}

fn bind_and_verify_at(
    service: &CoordService<'_>,
    caller: DeviceId,
    receiver: &std::sync::mpsc::Receiver<Mail>,
    at: u64,
) {
    let bind = Request::BindEmail {
        address: Some("Owner@Mail.example".to_owned()),
    };
    assert_eq!(ask_at(service, &bind, caller, at), Response::Done);
    let code = code_in(receiver);
    assert_eq!(
        ask_at(service, &Request::ConfirmEmail { code }, caller, at),
        Response::Done
    );
}

fn escrow(service: &CoordService<'_>, request: &Request) -> Vec<u8> {
    match ask(service, request, device(99).device_id()) {
        Response::Escrow(blob) => blob,
        other => panic!("not a container: {other:?}"),
    }
}

#[test]
fn red_team_a_verified_address_keeps_the_container_from_a_passphrase_alone() {
    let directory = directory();
    let (name, caller) = member_with_escrow(&directory, 61);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    let real = vec![61; 96];
    let old_ask = Request::GetEscrow {
        username: name.clone(),
    };
    assert_eq!(
        escrow(&service, &old_ask),
        real,
        "before an address, the passphrase alone"
    );

    bind_and_verify(&service, caller, &receiver);
    assert_ne!(
        escrow(&service, &old_ask),
        real,
        "a client that sends no code still got the container of an account whose address asks for one"
    );
    let guess = Request::RecoverEscrow {
        username: name.clone(),
        code: "12345678".to_owned(),
    };
    assert_ne!(
        escrow(&service, &guess),
        real,
        "a code nobody mailed opened it"
    );

    let stranger = device(98).device_id();
    assert_eq!(
        ask_at(
            &service,
            &Request::AskRecovery {
                username: name.clone()
            },
            stranger,
            NOW + 61
        ),
        Response::Done
    );
    let mail = receiver.try_recv().expect("the recovery code was mailed");
    assert_eq!(mail.to, "Owner@Mail.example");
    let code = digits_of(&mail);
    let with_code = Request::RecoverEscrow {
        username: name.clone(),
        code,
    };
    assert_eq!(
        escrow(&service, &with_code),
        real,
        "the mailed code and the name"
    );
    assert_ne!(escrow(&service, &with_code), real, "a code opened twice");
}

#[test]
fn red_team_a_recovery_code_dies_after_its_tries_and_its_quarter_hour() {
    let directory = directory();
    let (name, caller) = member_with_escrow(&directory, 62);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    bind_and_verify(&service, caller, &receiver);
    let real = vec![62; 96];

    ask_at(
        &service,
        &Request::AskRecovery {
            username: name.clone(),
        },
        caller,
        NOW + 61,
    );
    let code = code_in(&receiver);
    // Straight to the directory: the escrow limiter (five fetches a quarter-
    // hour) would stop the service's caller first, which is the outer wall.
    let owner = user(62).user_id();
    for wrong in 0..crate::directory::CODE_TRIES {
        let guess = format!("{wrong:08}x");
        assert_eq!(
            directory
                .take_recovery_code(owner, &guess, NOW + 61)
                .unwrap(),
            crate::directory::CodeCheck::Refused
        );
    }
    assert_eq!(
        directory
            .take_recovery_code(owner, &code, NOW + 61)
            .unwrap(),
        crate::directory::CodeCheck::Refused,
        "the right code after its tries were spent"
    );

    // A fresh one, a minute later (the spacing), and too late.
    let later = NOW + 122;
    ask_at(
        &service,
        &Request::AskRecovery {
            username: name.clone(),
        },
        caller,
        later,
    );
    let code = code_in(&receiver);
    let late = later + crate::directory::CODE_LIFETIME_SECS + 1;
    let answer = ask_at(
        &service,
        &Request::RecoverEscrow {
            username: name,
            code,
        },
        caller,
        late,
    );
    assert_ne!(
        answer,
        Response::Escrow(real),
        "a code past its quarter-hour"
    );
}

#[test]
fn red_team_asking_a_code_says_nothing_of_the_name_and_mails_nothing_without_an_address() {
    let directory = directory();
    let (name, _) = member_with_escrow(&directory, 63);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    let stranger = device(97).device_id();
    for username in [
        name.as_str(),
        "nobody-by-this-name",
        &"x".repeat(MAX_WIRE_USERNAME + 1),
    ] {
        assert_eq!(
            ask(
                &service,
                &Request::AskRecovery {
                    username: username.to_owned()
                },
                stranger
            ),
            Response::Done,
            "{username}"
        );
    }
    assert!(
        receiver.try_recv().is_err(),
        "a mail went out for an account with no address"
    );
    let plain = Request::RecoverEscrow {
        username: name,
        code: String::new(),
    };
    assert_eq!(
        escrow(&service, &plain),
        vec![63; 96],
        "no address: the passphrase alone, as before"
    );
}

#[test]
fn red_team_an_address_is_verified_by_its_code_only() {
    let directory = directory();
    let (_, caller) = member_with_escrow(&directory, 64);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    let bind = Request::BindEmail {
        address: Some("me@mail.example".to_owned()),
    };
    assert_eq!(ask(&service, &bind, caller), Response::Done);
    let code = code_in(&receiver);
    let wrong = Request::ConfirmEmail {
        code: "99999999".to_owned(),
    };
    if code != "99999999" {
        assert!(matches!(
            ask(&service, &wrong, caller),
            Response::Refused(_)
        ));
    }
    let status = ask(&service, &Request::Email, caller);
    assert_eq!(
        status,
        Response::Email(Some(EmailStatus {
            address: "me@mail.example".to_owned(),
            verified: false
        }))
    );
    // Another device, of another account, cannot give the code back.
    let other = device(65);
    enrol(&directory, &user(65), &other);
    let theirs = ask(
        &service,
        &Request::ConfirmEmail { code: code.clone() },
        other.device_id(),
    );
    assert!(matches!(theirs, Response::Refused(_)), "{theirs:?}");
    assert_eq!(
        ask(&service, &Request::ConfirmEmail { code }, caller),
        Response::Done
    );
    assert_eq!(
        ask(&service, &Request::Email, caller),
        Response::Email(Some(EmailStatus {
            address: "me@mail.example".to_owned(),
            verified: true
        }))
    );
    // A stranger, with no claim, cannot bind or read anything.
    let stranger = device(96).device_id();
    assert!(matches!(
        ask(&service, &bind, stranger),
        Response::Refused(_)
    ));
    assert!(matches!(
        ask(&service, &Request::Email, stranger),
        Response::Refused(_)
    ));
}

#[test]
fn red_team_a_verified_address_names_one_account() {
    let directory = directory();
    let (_, first) = member_with_escrow(&directory, 66);
    let (_, second) = member_with_escrow(&directory, 67);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    bind_and_verify(&service, first, &receiver);
    let again = Request::BindEmail {
        address: Some("OWNER@mail.example".to_owned()),
    };
    let answer = ask_at(&service, &again, second, NOW + 120);
    assert!(
        matches!(answer, Response::Refused(_)),
        "a second account took a verified address: {answer:?}"
    );
}

#[test]
fn red_team_unbinding_and_deleting_let_the_address_go() {
    let directory = directory();
    let (name, caller) = member_with_escrow(&directory, 68);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    bind_and_verify(&service, caller, &receiver);
    assert!(
        directory
            .owner_of_email("owner@mail.example")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        ask(&service, &Request::BindEmail { address: None }, caller),
        Response::Done
    );
    assert_eq!(
        directory.owner_of_email("owner@mail.example").unwrap(),
        None
    );
    let plain = Request::GetEscrow {
        username: name.clone(),
    };
    assert_eq!(
        escrow(&service, &plain),
        vec![68; 96],
        "unbound: the passphrase alone again"
    );

    bind_and_verify_at(&service, caller, &receiver, NOW + 61);
    directory.forget_account(&name).unwrap();
    assert_eq!(
        directory.owner_of_email("owner@mail.example").unwrap(),
        None,
        "the address outlived its account"
    );
}

#[test]
fn red_team_a_reminder_goes_only_to_a_verified_address_and_says_nothing_back() {
    let directory = directory();
    let (name, caller) = member_with_escrow(&directory, 69);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    let stranger = device(95).device_id();
    let remind = Request::RemindUsername {
        address: "owner@MAIL.example".to_owned(),
    };
    assert_eq!(ask(&service, &remind, stranger), Response::Done);
    assert!(
        receiver.try_recv().is_err(),
        "a reminder before the address was verified"
    );

    bind_and_verify(&service, caller, &receiver);
    assert_eq!(
        ask_at(&service, &remind, stranger, NOW + 61),
        Response::Done
    );
    let mail = receiver.try_recv().expect("the reminder");
    assert!(mail.text.contains(&name), "{}", mail.text);
    let unknown = Request::RemindUsername {
        address: "nobody@mail.example".to_owned(),
    };
    assert_eq!(ask(&service, &unknown, stranger), Response::Done);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_coordinator_that_sends_no_mail_binds_no_address() {
    let directory = directory();
    let (_, caller) = member_with_escrow(&directory, 70);
    let service = CoordService::new(&directory);
    let bind = Request::BindEmail {
        address: Some("me@mail.example".to_owned()),
    };
    assert_eq!(
        ask(&service, &bind, caller),
        Response::Refused(crate::service::email::NO_MAIL.to_owned())
    );
    assert_eq!(
        ask(&service, &Request::Email, caller),
        Response::Email(None)
    );
}

#[test]
fn red_team_a_code_held_back_does_not_replace_the_one_already_mailed() {
    let directory = directory();
    let (_, caller) = member_with_escrow(&directory, 71);
    let (outbox, receiver) = Outbox::new();
    let service = CoordService::new(&directory).with_outbox(&outbox);
    let bind = Request::BindEmail {
        address: Some("me@mail.example".to_owned()),
    };
    assert_eq!(ask(&service, &bind, caller), Response::Done);
    let mailed = code_in(&receiver);
    // A second Bind within the minute is held back: no mail, and the code
    // in the inbox must still verify the address.
    assert!(matches!(ask(&service, &bind, caller), Response::Refused(_)));
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        ask(&service, &Request::ConfirmEmail { code: mailed }, caller),
        Response::Done,
        "the code already mailed stopped working"
    );
}
