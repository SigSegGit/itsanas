//! What a shell has: each feature named in the page and refused behind its
//! route when missing, another account, the disks, start at login and updates.

use super::*;

#[test]
fn red_team_another_account_never_takes_the_home_of_one_already_here() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join(".itsanas");
    let taken = dir.path().join(".itsanas-two");
    for existing in [&home, &taken] {
        std::fs::create_dir_all(existing).expect("home");
        std::fs::write(existing.join("keystore.bin"), b"x").expect("keystore");
    }
    let page = Page::start(&home, dir.path(), Mode::Setup, &Seen::default());
    for (name, status, why) in [
        ("two", 409, "the home of another account on this computer"),
        ("Two+Words", 400, "a name that is no instance name"),
        ("passphrase", 400, "the default node's passphrase file"),
    ] {
        assert_eq!(
            page.api("POST", "/api/another", &format!("name={name}"))
                .status,
            status,
            "setting up another account was let into {why}: two accounts would share one \
             home, or one would open the other's keys"
        );
    }
    assert_eq!(
        page.api("POST", "/api/another", "name=three").status,
        200,
        "a free name was refused, so a second account could not be set up from the page"
    );
}

#[test]
fn the_disks_section_hands_work_to_the_daemon_and_writes_a_portable_copy_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let seen = Seen::default();
    let setup = Page::start(&home, dir.path(), Mode::Setup, &seen);
    assert_eq!(
        setup
            .api("POST", "/api/disk", "action=export&path=/x")
            .status,
        404,
        "the setup page answered a Settings action"
    );
    drop(setup);
    let page = Page::start(&home, dir.path(), Mode::Settings, &seen);
    let backup = std::path::absolute(dir.path().join("backup")).expect("absolute");
    let body = format!("action=export&path={}", backup.display()).replace('\\', "%5C");
    let refused = page.api("POST", "/api/disk", &body);
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert!(
        refused.body.contains("not running"),
        "no daemon, and the page did not say so: {}",
        refused.body
    );
    assert!(
        !home.join(crate::jobs::JOB).exists(),
        "a job was left for a daemon that is not there"
    );
    assert_eq!(page.api("POST", "/api/disk", "action=format").status, 400);

    let drive = std::path::absolute(dir.path().join("drive")).expect("absolute");
    let body = format!("action=portable&path={}", drive.display()).replace('\\', "%5C");
    let made = page.api("POST", "/api/disk", &body);
    assert_eq!(made.status, 200, "{}", made.body);
    assert!(crate::portable::is_portable(
        &drive.join(crate::portable::DIR).join("home")
    ));
    assert!(
        page.api("GET", "/api/state", "")
            .body
            .contains("\"job\":null")
    );
}

#[test]
fn red_team_every_feature_of_a_shell_is_named_in_the_page() {
    // The page hides what a shell lacks by `data-feature`. A feature no element
    // names is a feature the page cannot hide: a phone would be shown it.
    let html = super::INDEX_HTML;
    for (name, _) in Features::DESKTOP.all() {
        assert!(
            html.contains(&format!("data-feature=\"{name}\"")),
            "`{name}` is a feature of the shell and no element of index.html carries \
             data-feature=\"{name}\": a shell without it would still be shown it"
        );
    }
    let named: BTreeSet<&str> = html
        .split("data-feature=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let known: BTreeSet<&str> = Features::DESKTOP
        .all()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        named, known,
        "the page names a feature the shell does not have: it would never be shown"
    );
}

#[test]
fn red_team_an_iphone_is_refused_hosting_whatever_the_page_hides() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.features = Features::IPHONE;
        b.phone = true;
    });
    for (method, body) in [("GET", ""), ("POST", "contribute=on")] {
        let reply = page.api(method, "/api/hosting", body);
        assert_eq!(
            reply.status, 403,
            "{method} /api/hosting was answered on an iPhone, which ends an app it does not \
             show: a member would lend space nobody can reach ({})",
            reply.body
        );
    }
}

#[test]
fn red_team_a_route_behind_a_missing_feature_refuses_whatever_the_page_hides() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.features = Features::PHONE;
        b.phone = true;
    });
    for (method, path, body) in [
        ("POST", "/api/pick", ""),
        ("POST", "/api/open", ""),
        ("POST", "/api/pin", ""),
        ("POST", "/api/disk", "action=export&path=x"),
        ("GET", "/api/keys", ""),
        ("POST", "/api/copy-to-key", "path=x"),
        ("POST", "/api/another", "name=other"),
        ("POST", "/api/control", "action=pause"),
        ("POST", "/api/control", "action=interval&every=1h"),
    ] {
        let reply = page.api(method, path, body);
        assert_eq!(
            reply.status, 403,
            "{method} {path} was answered on a shell that has no such thing: hiding it in the \
             page is not the control ({})",
            reply.body
        );
        assert!(
            reply.body.contains("not offered here"),
            "{path} refused without saying why: {}",
            reply.body
        );
    }
    let sync = page.api("POST", "/api/control", "action=sync-now");
    assert_ne!(
        sync.status, 403,
        "syncing now is a phone's own button: {}",
        sync.body
    );

    // A computer has no fingerprint to set; a phone does, and is told.
    let phone = page.api("POST", "/api/host", "name=biometric&value=on");
    assert_eq!(phone.status, 200, "{}", phone.body);
    let battery = page.api("POST", "/api/host", "name=battery&value=on");
    assert_eq!(
        battery.status, 200,
        "a phone could not ask to work in the background without battery limits: {}",
        battery.body
    );
    let space = page.api("POST", "/api/space-default", "");
    assert_eq!(
        space.status, 200,
        "a phone's space button was refused: {}",
        space.body
    );
    let check = page.api("POST", "/api/host", "name=update");
    assert_eq!(
        check.status, 200,
        "a phone's update button was refused, so a failed offer has no way back: {}",
        check.body
    );
    let other = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let desktop = other.api("POST", "/api/host", "name=biometric&value=on");
    assert_eq!(
        desktop.status, 403,
        "a computer was told to set a fingerprint it does not have: {}",
        desktop.body
    );
    let unlimited = other.api("POST", "/api/host", "name=battery&value=on");
    assert_eq!(
        unlimited.status, 403,
        "a computer was told to lift battery limits it does not have: {}",
        unlimited.body
    );
}

/// An installed service that remembers whether it starts at logon.
struct Switched(Arc<Mutex<bool>>);

impl ServiceControl for Switched {
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
        Ok(())
    }
    fn set_autostart(&self, on: bool) -> Result<()> {
        *self.0.lock().expect("lock") = on;
        Ok(())
    }
    fn autostart(&self) -> Option<bool> {
        Some(*self.0.lock().expect("lock"))
    }
    fn log_hint(&self) -> String {
        "the test".to_owned()
    }
}

/// Start at login, per account, from Settings: the switch changes this
/// instance's service and says what the system now says. Without a service there
/// is nothing to switch, and a phone has no such switch. Sabotage: the switch
/// always turns it on.
#[test]
fn red_team_settings_turn_this_accounts_start_at_login_on_and_off() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let on = Arc::new(Mutex::new(true));
    let state = on.clone();
    let page = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.features = super::Features::DESKTOP;
        b.service = Box::new(move || {
            let service: Box<dyn ServiceControl> = Box::new(Switched(state.clone()));
            service
        });
    });
    assert!(
        page.api("GET", "/api/autostart", "")
            .body
            .contains("\"on\":true"),
        "the page did not read the service's state"
    );
    let off = page.api("POST", "/api/autostart", "on=false");
    assert_eq!(
        off.status, 200,
        "turning start at login off was refused: {}",
        off.body
    );
    assert!(
        !*on.lock().expect("lock"),
        "the account still starts at login after it was turned off"
    );
    assert!(
        off.body.contains("\"on\":false"),
        "the page says on after turning it off: {}",
        off.body
    );
    assert_eq!(
        page.api("POST", "/api/autostart", "on=maybe").status,
        400,
        "a switch neither on nor off was taken"
    );
    assert!(
        !*on.lock().expect("lock"),
        "a malformed answer turned start at login back on"
    );
    drop(page);

    let none = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    assert_eq!(
        none.api("POST", "/api/autostart", "on=true").status,
        409,
        "a switch was taken with no service to switch"
    );
    drop(none);
    let phone = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.features = super::Features::PHONE;
    });
    assert_eq!(
        phone.api("GET", "/api/autostart", "").status,
        403,
        "a phone offered a switch its system holds"
    );
}

/// Check for updates on a computer, not only on a phone: the page tells the
/// computer's shell, which runs `itsanas update` apart. An iPhone has nothing to
/// check. Sabotage: gate the update on `features.metered` again.
#[test]
fn red_team_a_computer_checks_for_updates_and_an_iphone_does_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let told = Arc::new(Told::default());
    let page = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.host = told.clone();
        b.features = super::Features::DESKTOP;
    });
    let asked = page.api("POST", "/api/host", "name=update");
    assert_eq!(
        asked.status, 200,
        "a computer's Check for updates was refused: {}",
        asked.body
    );
    assert_eq!(
        *told.0.lock().expect("lock"),
        [("update".to_owned(), "check".to_owned())],
        "the computer was never told to check: the button does nothing"
    );
    drop(page);
    let iphone = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.features = super::Features::IPHONE;
    });
    assert_eq!(
        iphone.api("POST", "/api/host", "name=update").status,
        403,
        "an iPhone, installed by whoever signs it, offered an update it cannot install"
    );
}
