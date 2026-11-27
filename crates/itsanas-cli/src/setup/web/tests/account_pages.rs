//! The account from Settings: its devices and machines, signing out, the keys
//! on a phone, removing or deleting it, and the stop handle.

use super::*;

#[test]
fn forgetting_on_a_page_with_no_node_changes_nothing_and_says_so() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let started = page.api("POST", "/api/forgot", "");
    assert_eq!(started.status, 202, "{}", started.body);
    let mut state = String::new();
    for _ in 0..200 {
        state = page.api("GET", "/api/state", "").body;
        if !state.contains("\"phase\":\"running\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        state.contains("\"phase\":\"failed\"") && state.contains("no node"),
        "forgetting with nothing to replace did not fail, or did not say why: {state}"
    );
    let setup = Page::start(&home, dir.path(), Mode::Setup, &Seen::default());
    assert_eq!(
        setup.api("POST", "/api/forgot", "").status,
        404,
        "the setup page offers to replace the keys of an account that is not there yet"
    );
}

/// A node of account `username` at `home`, pointed at the coordinator and
/// enrolled; its phrase when it was made new, and its device.
fn enrolled(
    home: &Path,
    coordinator: std::net::SocketAddr,
    username: &str,
    phrase: Option<&str>,
) -> (String, itsanas_crypto::DeviceId) {
    let (mut node, phrase) = match phrase {
        None => {
            let (node, phrase) =
                itsanas_node::Node::create(home, PASSPHRASE, username).expect("create");
            (node, phrase.0.to_string())
        }
        Some(phrase) => (
            itsanas_node::Node::restore(home, PASSPHRASE, username, phrase).expect("restore"),
            phrase.to_owned(),
        ),
    };
    node.config.coordinator = Some(coordinator.to_string());
    node.save_config().expect("config");
    crate::coordinator::register_with(&node, None, crate::testing::NOW).expect("enrol");
    (phrase, node.store.device_id())
}

#[test]
fn red_team_the_page_withdraws_a_listed_full_id_and_never_this_machine_nor_a_stray_one() {
    use crate::testing::{live, with_directory};
    with_directory(|address, directory| {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("node");
        let (phrase, mine) = enrolled(&home, address, "camille", None);
        let (_, other) = enrolled(&dir.path().join("other"), address, "camille", Some(&phrase));
        let (_, stranger) = enrolled(&dir.path().join("voisin"), address, "voisin", None);

        let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
        let listed = page.api("GET", "/api/devices", "");
        assert_eq!(listed.status, 200, "{}", listed.body);
        assert!(
            listed.body.contains(&other.to_string()),
            "the account's other machine is not offered: {}",
            listed.body
        );
        assert!(
            !listed.body.contains(&stranger.to_string()),
            "another account's machine is listed"
        );

        for (what, id) in [
            ("an abbreviated id", other.short()),
            ("this machine", mine.to_string()),
            ("a device of another account", stranger.to_string()),
        ] {
            let refused = page.api("POST", "/api/withdraw", &format!("id={id}"));
            assert_eq!(
                refused.status, 409,
                "{what} was withdrawn: {}",
                refused.body
            );
        }
        assert!(
            live(directory, mine),
            "the page withdrew the machine it runs on"
        );
        assert!(
            live(directory, stranger),
            "the page withdrew another account's device"
        );
        assert!(
            live(directory, other),
            "an abbreviated id withdrew a device"
        );

        let done = page.api("POST", "/api/withdraw", &format!("id={other}"));
        assert_eq!(done.status, 200, "{}", done.body);
        assert!(
            !live(directory, other),
            "the listed full id was refused its withdrawal: the slot is never freed"
        );
    });
}

#[test]
fn machines_typed_on_the_page_are_kept_listed_and_forgotten() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let _ = set_up_through_the_page(&home, &dir.path().join("base").join("folder"));
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let added = page.api("POST", "/api/peers", "action=add&address=10.0.0.5%3A7777");
    assert_eq!(added.status, 200, "{}", added.body);
    assert!(added.body.contains("10.0.0.5:7777"), "{}", added.body);
    let config =
        crate::config::Config::load(&itsanas_node::Node::config_path(&home)).expect("config");
    assert!(
        config.peers.iter().any(|peer| peer == "10.0.0.5:7777"),
        "the address was listed and not kept: the daemon would never dial it"
    );
    let parked = page.api("POST", "/api/peers", "action=add&address=relay%3Aabc");
    assert_eq!(
        parked.status, 409,
        "a parked machine was typed in: {}",
        parked.body
    );
    let forgotten = page.api(
        "POST",
        "/api/peers",
        "action=forget&address=10.0.0.5%3A7777",
    );
    assert!(
        forgotten.body.contains("\"peers\":[]"),
        "the machine was forgotten and is still listed: the daemon would keep dialling it: {}",
        forgotten.body
    );
    assert!(
        crate::config::Config::load(&itsanas_node::Node::config_path(&home))
            .expect("config")
            .peers
            .is_empty(),
        "a forgotten address stayed in the file"
    );
    let phone = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.features = Features::PHONE;
    });
    assert_eq!(
        phone.api("GET", "/api/peers", "").status,
        403,
        "a shell without Advanced was offered the machines typed by address"
    );
}

#[test]
fn a_setup_that_stopped_at_the_device_limit_says_so_to_the_page() {
    use super::{Phase, Run, run_json};
    use crate::setup::Step;
    let run = Run {
        phase: Phase::Failed,
        failed: Some((
            Step::Registration,
            "this account has 20 live devices and the limit is 20".to_owned(),
            "withdraw one".to_owned(),
        )),
        ..Run::default()
    };
    assert!(
        run_json(&run).contains("\"atCap\":true"),
        "the page is not told to open the devices: the person is left with a refusal and no way out"
    );
    let other = Run {
        failed: Some((Step::Registration, "no route".to_owned(), "wait".to_owned())),
        ..Run::default()
    };
    assert!(run_json(&other).contains("\"atCap\":false"));
}

/// The app locks when the page tells it, and the node stays open until then.
/// Sabotage: tell it nothing, or wait for the store as a computer does.
#[test]
fn red_team_a_phone_that_signs_out_locks_at_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (node, _) = itsanas_node::Node::create(&home, PASSPHRASE, "camille").expect("create");
    let file = home.with_extension("passphrase");
    std::fs::write(&file, "stand-in\n").expect("stand-in passphrase");
    let slot: &'static super::Slot =
        Box::leak(Box::new(Mutex::new(Some(std::sync::Arc::new(node)))));
    let heard = Heard::default();
    let told = heard.clone();
    let kept = file.clone();
    let page = Page::start_with(
        &home,
        dir.path(),
        Mode::Settings,
        &Seen::default(),
        move |b| {
            b.features = Features::PHONE;
            b.nodes = Arc::new(super::Held(slot));
            b.host = Arc::new(told);
            b.service = Box::new(move || {
                let service: Box<dyn ServiceControl> = Box::new(TheApp { file: kept.clone() });
                service
            });
        },
    );

    let asked = std::time::Instant::now();
    let out = page.api("POST", "/api/signout", "");
    assert_eq!(out.status, 200, "sign out was refused: {}", out.body);
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(15),
        "signing out took {:?}: the phone waited for itself to let go of its own store, and \
         the person looks at an account that seems to stay open",
        asked.elapsed()
    );
    assert!(
        !file.exists(),
        "signed out, and the phone still remembers the passphrase: it opens again for anybody"
    );
    assert_eq!(
        heard.0.lock().expect("heard").as_slice(),
        [("signedOut".to_owned(), String::new())],
        "the app was not told: the account stays open on screen after the person signed out"
    );
}

/// What the shell was told, in order.
#[derive(Clone, Default)]
struct Heard(Arc<Mutex<Vec<(String, String)>>>);

impl super::Host for Heard {
    fn event(&self, name: &str, value: &str) {
        self.0
            .lock()
            .expect("heard")
            .push((name.to_owned(), value.to_owned()));
    }
}

/// The app as its own service: it holds the node, so nothing else can let go of it.
struct TheApp {
    file: PathBuf,
}

impl ServiceControl for TheApp {
    fn passphrase_file(&self) -> PathBuf {
        self.file.clone()
    }
    fn installed(&self) -> bool {
        true
    }
    fn install(&self, _: bool) -> Result<String> {
        Ok("the app is its own service".to_owned())
    }
    fn start(&self) -> Result<()> {
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }
    fn set_autostart(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn log_hint(&self) -> String {
        "the test".to_owned()
    }
    fn holds_the_store_itself(&self) -> bool {
        true
    }
}

/// A phone's page over a node it holds open: the Settings of `home`, with the
/// node in a slot of its own and the shell's events recorded.
fn phone_page(home: &Path, base: &Path, node: itsanas_node::Node, heard: &Heard) -> Page {
    let slot: &'static super::Slot =
        Box::leak(Box::new(Mutex::new(Some(std::sync::Arc::new(node)))));
    let heard = heard.clone();
    Page::start_with(home, base, Mode::Settings, &Seen::default(), move |b| {
        b.features = Features::PHONE;
        b.nodes = Arc::new(super::Held(slot));
        b.host = Arc::new(heard);
    })
}

/// On a phone the node is open in this process, so a replacement of the keys
/// would wait twenty seconds for a store that never lets go, then fail: the
/// person who forgot is sent on to the Unlock screen instead. Sabotage: drop the
/// arm of `gate`.
#[test]
fn red_team_a_phone_refuses_to_replace_the_keys_under_an_open_node() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (node, _) = itsanas_node::Node::create(&home, PASSPHRASE, "camille").expect("create");
    let heard = Heard::default();
    let page = phone_page(&home, dir.path(), node, &heard);
    let said = page.api("POST", "/api/forgot", "");
    assert_eq!(
        said.status, 403,
        "a phone started replacing the keys under its own open node: {}",
        said.body
    );
    assert!(
        said.body.contains("choose I forgot"),
        "the person was not told where to go: {}",
        said.body
    );
}

#[test]
fn red_team_removing_the_account_through_the_page_needs_its_name_where_pieces_are_only_here() {
    use crate::testing::{live, with_directory};
    with_directory(|address, directory| {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("node");
        let (mut node, _) =
            itsanas_node::Node::create(&home, PASSPHRASE, "camille").expect("create");
        node.config.coordinator = Some(address.to_string());
        node.save_config().expect("config");
        crate::coordinator::register_with(&node, None, crate::testing::NOW).expect("enrol");
        let device = node.store.device_id();
        // A file no other machine has: its pieces exist only here.
        let source = dir.path().join("source");
        std::fs::write(&source, b"kept on this machine alone").expect("source");
        node.store
            .write_stream("one.txt", std::fs::File::open(&source).expect("open"))
            .expect("write");
        node.store.flush_segment().expect("flush");

        let heard = Heard::default();
        let page = phone_page(&home, dir.path(), node, &heard);
        let check = page.api("GET", "/api/remove", "");
        assert_eq!(check.status, 200, "{}", check.body);
        assert!(
            !check.body.contains("\"onlyHere\":0")
                && check.body.contains("\"username\":\"camille\""),
            "the check did not say the pieces exist only here: {}",
            check.body
        );

        for typed in ["", "confirm=camil", "confirm=someone-else"] {
            let refused = page.api("POST", "/api/remove", typed);
            assert_eq!(
                refused.status, 409,
                "the account was removed without its name ({typed:?}): its only copy is gone"
            );
        }
        assert!(home.exists(), "a refused removal still erased the home");
        assert!(
            live(directory, device),
            "a refused removal still freed the slot"
        );
        assert!(
            heard.0.lock().expect("heard").is_empty(),
            "the shell was told the account was gone when it was not"
        );

        let done = page.api("POST", "/api/remove", "confirm=camille");
        assert_eq!(done.status, 200, "{}", done.body);
        assert!(!home.exists(), "the home survived the removal");
        assert!(
            !live(directory, device),
            "the removal kept this device's place among the account's"
        );
        assert_eq!(
            heard.0.lock().expect("heard").as_slice(),
            [("account_removed".to_owned(), "removed".to_owned())],
            "the shell was not told: its screens would show an account that is gone"
        );
    });
}

#[test]
fn red_team_deleting_the_account_through_the_page_takes_its_own_name_typed() {
    use crate::testing::with_directory;
    with_directory(|address, _| {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("node");
        let (mut node, _) =
            itsanas_node::Node::create(&home, PASSPHRASE, "camille").expect("create");
        node.config.coordinator = Some(address.to_string());
        node.save_config().expect("config");
        crate::coordinator::register_with(&node, None, crate::testing::NOW).expect("enrol");
        let heard = Heard::default();
        let page = phone_page(&home, dir.path(), node, &heard);

        for typed in ["", "confirm=camil", "confirm=voisin"] {
            let refused = page.api("POST", "/api/delete", typed);
            assert_eq!(
                refused.status, 409,
                "the account was deleted on a name that is not its own ({typed:?})"
            );
        }
        assert!(home.exists(), "a refused deletion erased the home");
        assert!(
            heard.0.lock().expect("heard").is_empty(),
            "the shell was told of a deletion that was refused"
        );

        let done = page.api("POST", "/api/delete", "confirm=camille");
        assert_eq!(done.status, 200, "{}", done.body);
        assert!(!home.exists(), "the home survived the deletion");
        assert_eq!(
            heard.0.lock().expect("heard").as_slice(),
            [("account_removed".to_owned(), "deleted".to_owned())]
        );
    });
}

/// An installed service that writes down what was done to it.
struct Recorded(Arc<Mutex<Vec<&'static str>>>);

impl ServiceControl for Recorded {
    fn passphrase_file(&self) -> PathBuf {
        PathBuf::new()
    }
    fn installed(&self) -> bool {
        true
    }
    fn install(&self, _: bool) -> Result<String> {
        Ok(String::new())
    }
    fn start(&self) -> Result<()> {
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        self.0.lock().expect("lock").push("stop");
        Ok(())
    }
    fn set_autostart(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn uninstall(&self) -> Result<()> {
        self.0.lock().expect("lock").push("uninstall");
        Ok(())
    }
    fn log_hint(&self) -> String {
        "the test".to_owned()
    }
}

/// A computer removes the account from its Settings too, and its background
/// service goes with it: a task left behind would start, at every logon, for
/// an account whose home is gone. Sabotage: drop the `uninstall` call.
#[test]
fn red_team_a_computer_that_removes_the_account_leaves_no_service_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (node, _) = itsanas_node::Node::create(&home, PASSPHRASE, "camille").expect("create");
    let slot: &'static super::Slot =
        Box::leak(Box::new(Mutex::new(Some(std::sync::Arc::new(node)))));
    let done = Arc::new(Mutex::new(Vec::new()));
    let seen = done.clone();
    let page = Page::start_with(
        &home,
        dir.path(),
        Mode::Settings,
        &Seen::default(),
        move |b| {
            b.features = Features::DESKTOP;
            b.nodes = Arc::new(super::Held(slot));
            b.host = Arc::new(Heard::default());
            b.service = Box::new(move || {
                let service: Box<dyn ServiceControl> = Box::new(Recorded(seen.clone()));
                service
            });
        },
    );
    let removed = page.api("POST", "/api/remove", "");
    assert_eq!(
        removed.status, 200,
        "a computer could not remove the account: {}",
        removed.body
    );
    assert!(!home.exists(), "the home survived the removal");
    assert!(
        done.lock().expect("lock").contains(&"uninstall"),
        "the service was left behind: it starts at every logon for an account that is gone"
    );
}

/// A phone person leaves with Back: the server must end then, not at the idle
/// exit, or the account stays reachable by a page nobody is looking at.
/// Sabotage: make `Stop::stop` do nothing.
#[test]
fn the_stop_handle_ends_a_server_that_would_otherwise_wait_for_the_idle_exit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let home = directory.path().join("home");
    let server = Server::bind(
        &home,
        None,
        Mode::Setup,
        directory.path(),
        backends(&Seen::default(), &home),
    )
    .expect("bind");
    let stop = server.stopper();
    let handle = std::thread::spawn(move || server.serve(Duration::from_secs(3600)));
    stop.stop();
    let started = std::time::Instant::now();
    while !handle.is_finished() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        handle.is_finished(),
        "the person left and the page kept answering for an hour"
    );
    let _ = handle.join();
}
