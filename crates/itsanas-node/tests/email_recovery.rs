//! An account binds an email address through a real coordinator, and a
//! recovery from the passphrase then needs the code mailed there.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use itsanas_coord::Directory;
use itsanas_coord::mail::{Mail, Outbox};
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";

struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// A coordinator that mails into `Receiver` instead of Mailjet.
fn with_mailing_coordinator<T>(body: impl FnOnce(SocketAddr, &Receiver<Mail>) -> T) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let (outbox, mails) = Outbox::new();
    let outbox = outbox.with_spacing(0);
    let server = CoordServer::bind("127.0.0.1:0")
        .expect("bind")
        .with_outbox(outbox);
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let device = DeviceKeys::from_seed(&SecretBytes::new([0xC1; 32]));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &device, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address, &mails)
    })
}

fn code_of(mails: &Receiver<Mail>) -> String {
    let mail = mails
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("a mail");
    let at = mail.text.find("code is ").expect("a code") + "code is ".len();
    mail.text[at..at + 8].to_owned()
}

/// THE ATTACK: somebody who knows, or guessed, an account's passphrase
/// recovers it on their own machine. Once the account has verified an
/// address, they also need the code mailed there; the owner, who has the
/// mailbox, gets in. Sabotage: drop the gate in `escrow_given`.
#[test]
fn red_team_the_passphrase_alone_no_longer_recovers_an_account_with_a_verified_address() {
    with_mailing_coordinator(|address, mails| {
        let home = tempfile::tempdir().expect("home");
        let (mut node, _) = Node::create(home.path(), PASSPHRASE, "carol").expect("create");
        node.config.coordinator = Some(address.to_string());
        node.save_config().expect("save config");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        coordinator::register_with(&node, None, now).expect("enrol");
        coordinator::set_escrow(&node, Some(PASSPHRASE), &node.secrets).expect("escrow");
        let at = address.to_string();
        assert!(
            coordinator::fetch_escrow(&at, None, "carol", PASSPHRASE).is_ok(),
            "before an address, the passphrase alone opens it"
        );

        coordinator::bind_email(&node, Some("carol@mail.example")).expect("bind");
        coordinator::confirm_email(&node, &code_of(mails)).expect("confirm");
        let status = coordinator::email_status(&node)
            .expect("status")
            .expect("an address");
        assert!(status.verified);

        let alone = coordinator::fetch_escrow(&at, None, "carol", PASSPHRASE);
        assert!(
            alone.is_err(),
            "the passphrase alone opened an account whose address asks for a code"
        );

        coordinator::ask_recovery_code(&at, None, "carol").expect("ask");
        let code = code_of(mails);
        let opened = coordinator::fetch_escrow_with_code(&at, None, "carol", PASSPHRASE, &code);
        assert!(
            opened.is_ok(),
            "the passphrase and the mailed code did not open: {opened:?}"
        );
        let again = coordinator::fetch_escrow_with_code(&at, None, "carol", PASSPHRASE, &code);
        assert!(again.is_err(), "a mailed code opened twice");
    });
}
