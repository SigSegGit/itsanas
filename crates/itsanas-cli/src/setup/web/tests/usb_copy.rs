//! Copying this account to a USB key from Settings.

use super::*;

// ---------------------------------------------------------------------------
// Copy this account to a USB key
// ---------------------------------------------------------------------------

/// An account on this machine, and drives beside it.
struct KeyWorld {
    dir: tempfile::TempDir,
    home: PathBuf,
}

impl KeyWorld {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("node");
        crate::node::Node::create(&home, PASSPHRASE, "camille").expect("account");
        Self { dir, home }
    }

    /// A drive named `name`, made, absolute.
    fn drive(&self, name: &str) -> PathBuf {
        let drive = std::path::absolute(self.dir.path().join(name)).expect("absolute");
        std::fs::create_dir_all(&drive).expect("drive");
        drive
    }

    /// A drive that already holds a copied account.
    fn taken_drive(&self, name: &str) -> PathBuf {
        let drive = self.drive(name);
        let key_home = drive.join(crate::portable::DIR).join("home");
        std::fs::create_dir_all(&key_home).expect("key home");
        std::fs::write(key_home.join("keystore.bin"), b"x").expect("keystore");
        drive
    }
}

fn candidate(
    path: &Path,
    free: Option<u64>,
    format: Option<crate::drives::Format>,
) -> crate::drives::Candidate {
    crate::drives::Candidate {
        path: path.to_owned(),
        label: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        total: free.map(|free| free * 2),
        free,
        format,
        external: false,
    }
}

/// The row `/api/keys` wrote for `drive`, up to the next row.
fn key_row<'a>(body: &'a str, drive: &Path) -> &'a str {
    let quoted = super::json::quote(&drive.display().to_string());
    body.split("{\"path\":")
        .find(|row| row.starts_with(&quoted))
        .unwrap_or_else(|| panic!("no row for {}: {body}", drive.display()))
}

/// `/api/keys` once the page's thread has measured the programs: before
/// that, every row is measured against the guess.
fn keys_once_measured(page: &Page) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let body = page.api("GET", "/api/keys", "").body;
        if body.contains("\"programs_known\":true") {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "the programs were never measured: {body}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The paths the stand-in copy was started for, and whether it still runs.
#[derive(Clone, Default)]
struct Copies {
    started: Arc<Mutex<Vec<PathBuf>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl Copies {
    fn backend(&self) -> super::StartCopy {
        let copies = self.clone();
        Box::new(move |_, dir| {
            copies.started.lock().expect("started").push(dir.to_owned());
            copies
                .running
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let running = Arc::clone(&copies.running);
            let alive: super::Running =
                Box::new(move || running.load(std::sync::atomic::Ordering::SeqCst));
            Ok(alive)
        })
    }

    fn started(&self) -> Vec<PathBuf> {
        self.started.lock().expect("started").clone()
    }
}

/// `path=...` for a form, as `URLSearchParams` writes it.
fn path_form(path: &Path) -> String {
    let mut out = String::from("path=");
    for byte in path.display().to_string().bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[test]
fn red_team_the_usb_copy_answers_only_the_page_s_key_and_only_in_settings() {
    let world = KeyWorld::new();
    let key = world.drive("KEY");
    let copies = Copies::default();
    let seen = Seen::default();
    let calls = [
        ("GET", "/api/keys", String::new()),
        ("POST", "/api/keys", path_form(&key)),
        ("GET", "/api/copy-to-key", String::new()),
        ("POST", "/api/copy-to-key", path_form(&key)),
    ];
    let page = Page::start_with(&world.home, world.dir.path(), Mode::Settings, &seen, |b| {
        let listed = key.clone();
        b.drives = Box::new(move || vec![candidate(&listed, Some(1 << 40), None)]);
        b.copy = copies.backend();
    });
    let host = page.host();
    for (method, path, body) in &calls {
        for token in [None, Some("0".repeat(32))] {
            let mut headers = vec![("Host", host.as_str())];
            if let Some(token) = &token {
                headers.push(("X-Itsanas-Token", token.as_str()));
            }
            let reply = page.send(method, path, &headers, body);
            assert_eq!(
                reply.status, 403,
                "{method} {path} answered without the page's key ({token:?}): any page open in \
                 the browser could list the drives or start a copy: {}",
                reply.body
            );
            assert!(
                !reply.body.contains("KEY"),
                "{method} {path} told a stranger about a drive"
            );
        }
    }
    assert!(
        copies.started().is_empty(),
        "a copy started for a request without the page's key"
    );
    drop(page);

    let setup = Page::start_with(&world.home, world.dir.path(), Mode::Setup, &seen, |b| {
        b.copy = copies.backend();
    });
    for (method, path, body) in &calls {
        assert_eq!(
            setup.api(method, path, body).status,
            404,
            "the setup page answered {method} {path}, a Settings action"
        );
    }
    assert!(copies.started().is_empty(), "the setup page started a copy");
}

#[test]
fn a_key_without_room_or_already_holding_an_account_does_not_fit_and_says_why() {
    let world = KeyWorld::new();
    let roomy = world.drive("ROOMY");
    let small = world.drive("SMALL");
    let taken = world.taken_drive("TAKEN");
    let windows = world.drive("WINDOWS");
    let inside = world.home.clone();
    let listed = [
        candidate(&roomy, Some(64 << 30), Some(crate::drives::Format::ExFat)),
        // Smaller than the programs whichever size the page reads: the
        // guess until its thread has measured them, the stand-in's 1 KiB
        // after. A 1 MiB key was refused or offered on whichever thread won
        // (it lost once, on ARM, 2026-10-29).
        candidate(&small, Some(512), Some(crate::drives::Format::Fat32)),
        candidate(&taken, Some(64 << 30), None),
        candidate(&windows, Some(64 << 30), Some(crate::drives::Format::Ntfs)),
        candidate(&inside, Some(64 << 30), None),
    ];
    let page = Page::start_with(
        &world.home,
        world.dir.path(),
        Mode::Settings,
        &Seen::default(),
        |b| b.drives = Box::new(move || listed.to_vec()),
    );
    let reply = page.api("GET", "/api/keys", "");
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        reply.body.matches("{\"path\":").count(),
        5,
        "{}",
        reply.body
    );
    let row = |drive: &Path| key_row(&reply.body, drive);
    assert!(
        row(&roomy).contains("\"fits\":true,\"why\":null"),
        "a key with room was refused: {}",
        row(&roomy)
    );
    let small_row = row(&small);
    assert!(
        small_row.contains("\"fits\":false,\"why\":\"room\""),
        "a 512-byte key was offered for a copy of the programs alone: {small_row}"
    );
    assert!(
        json_field(small_row, "reason").contains("free"),
        "{small_row}"
    );
    assert!(
        json_field(small_row, "warning").contains("FAT32"),
        "{small_row}"
    );
    let taken_row = row(&taken);
    assert!(
        taken_row.contains("\"fits\":false,\"why\":\"account\""),
        "a key holding an account was offered: the copy would land on top of it: {taken_row}"
    );
    assert!(
        json_field(taken_row, "reason").contains("already holds an account"),
        "{taken_row}"
    );
    let windows_row = row(&windows);
    assert!(
        windows_row.contains("\"fits\":true") && json_field(windows_row, "warning").contains("Mac"),
        "an NTFS key fits, with the warning that a Mac cannot write to it: {windows_row}"
    );
    assert!(
        row(&inside).contains("\"fits\":false,\"why\":\"inside\""),
        "the account's own home was offered as a key: {}",
        row(&inside)
    );
    for sizes in ["\"programs_bytes\":", "\"needed\":", "\"copying\":false"] {
        assert!(
            reply.body.contains(sizes),
            "{sizes} missing: {}",
            reply.body
        );
    }

    // Once the programs are measured, the same key is still too small: the
    // refusal does not depend on the guess.
    let measured = keys_once_measured(&page);
    let small_measured = key_row(&measured, &small);
    assert!(
        small_measured.contains("\"fits\":false,\"why\":\"room\""),
        "a 512-byte key was offered once the programs were measured at 1 KiB: {small_measured}"
    );

    // "Another folder…": the same row for a path the person chose.
    let chosen = page.api("POST", "/api/keys", &path_form(&taken));
    assert_eq!(chosen.status, 200, "{}", chosen.body);
    assert!(
        chosen.body.contains("\"fits\":false,\"why\":\"account\""),
        "{}",
        chosen.body
    );
    let relative = page.api("POST", "/api/keys", "path=KEY");
    assert!(
        relative.body.contains("\"fits\":false,\"why\":\"path\""),
        "{}",
        relative.body
    );
}

#[test]
fn red_team_the_copy_refuses_a_path_the_jobs_would_refuse_and_starts_nothing() {
    let world = KeyWorld::new();
    let taken = world.taken_drive("TAKEN");
    let copies = Copies::default();
    let page = Page::start_with(
        &world.home,
        world.dir.path(),
        Mode::Settings,
        &Seen::default(),
        |b| b.copy = copies.backend(),
    );
    for (body, why) in [
        ("path=".to_owned(), "no path"),
        ("path=KEY".to_owned(), "a relative path"),
        ("path=%2Fmedia%2Fkey%0A%2Fetc".to_owned(), "a line break"),
        ("path=%2Fmedia%2Fkey%00".to_owned(), "a NUL"),
        (path_form(&taken), "a key that already holds an account"),
        (path_form(&world.home), "this account's own home"),
    ] {
        let reply = page.api("POST", "/api/copy-to-key", &body);
        assert_eq!(
            reply.status, 400,
            "{why} was accepted as a key: {}",
            reply.body
        );
        assert!(
            copies.started().is_empty(),
            "a copy was started for {why}: the passphrase windows open for a copy that cannot \
             happen, or one that writes where it must not"
        );
    }
    let key = world.drive("KEY");
    let started = page.api("POST", "/api/copy-to-key", &path_form(&key));
    assert_eq!(started.status, 202, "{}", started.body);
    assert_eq!(copies.started(), [key]);
}

#[test]
fn a_second_copy_while_one_runs_is_refused_and_the_page_follows_the_first() {
    let world = KeyWorld::new();
    let key = world.drive("KEY");
    let copies = Copies::default();
    let page = Arc::new(Page::start_with(
        &world.home,
        world.dir.path(),
        Mode::Settings,
        &Seen::default(),
        |b| b.copy = copies.backend(),
    ));
    // An old run's end, which must not read as this run's.
    std::fs::write(
        world.home.join(crate::copy_to_key::STATUS),
        "{\"step\":\"done\",\"done\":true,\"said\":\"old\",\"error\":null}\n",
    )
    .expect("old status");
    let body = path_form(&key);
    let racers: Vec<_> = (0..2)
        .map(|_| {
            let page = Arc::clone(&page);
            let body = body.clone();
            std::thread::spawn(move || page.api("POST", "/api/copy-to-key", &body).status)
        })
        .collect();
    let mut statuses: Vec<u16> = racers
        .into_iter()
        .map(|racer| racer.join().expect("racer"))
        .collect();
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        [202, 409],
        "two copies at once: two pairs of passphrase windows, two writers on one key"
    );
    assert_eq!(copies.started().len(), 1);
    let state = page.api("GET", "/api/copy-to-key", "");
    assert_eq!(
        state.body, "{\"running\":true,\"progress\":null}",
        "the old run's end was shown as this one's"
    );

    // The windows answered: the first process is gone, its background run
    // writes the steps. Still running, still refused.
    copies
        .running
        .store(false, std::sync::atomic::Ordering::SeqCst);
    std::fs::write(
        world.home.join(crate::copy_to_key::STATUS),
        "{\"step\":\"programs\",\"done\":false,\"said\":\"writing the programs\",\"error\":null}\n",
    )
    .expect("status");
    assert_eq!(page.api("POST", "/api/copy-to-key", &body).status, 409);
    let state = page.api("GET", "/api/copy-to-key", "");
    assert!(state.body.contains("\"running\":true"), "{}", state.body);
    assert!(
        state.body.contains("\"step\":\"programs\""),
        "{}",
        state.body
    );
    assert!(
        page.api("GET", "/api/keys", "")
            .body
            .contains("\"copying\":true")
    );

    // Done: the page reads the end, and another copy may start.
    std::fs::write(
        world.home.join(crate::copy_to_key::STATUS),
        "{\"step\":\"done\",\"done\":true,\"said\":\"The key is ready\",\"error\":null}\n",
    )
    .expect("status");
    let state = page.api("GET", "/api/copy-to-key", "");
    assert!(state.body.contains("\"running\":false"), "{}", state.body);
    assert!(state.body.contains("The key is ready"), "{}", state.body);
    assert_eq!(page.api("POST", "/api/copy-to-key", &body).status, 202);
    assert_eq!(copies.started().len(), 2);
}

/// A CHILD THAT VANISHED BEFORE IT SAID ANYTHING: the page's own child
/// exited and no status was written yet (its background run is starting,
/// or it died before its first step). For two minutes another copy is
/// refused: two writers would race on one key.
#[test]
fn red_team_a_copy_is_refused_while_the_last_one_started_has_not_spoken() {
    let world = KeyWorld::new();
    let key = world.drive("KEY");
    let copies = Copies::default();
    let page = Page::start_with(
        &world.home,
        world.dir.path(),
        Mode::Settings,
        &Seen::default(),
        |b| b.copy = copies.backend(),
    );
    let body = path_form(&key);
    assert_eq!(page.api("POST", "/api/copy-to-key", &body).status, 202);
    copies
        .running
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        page.api("POST", "/api/copy-to-key", &body).status,
        409,
        "a second copy started while the first had not written a word"
    );
    assert_eq!(copies.started().len(), 1);
    // It spoke and ended: another may start.
    std::fs::write(
        world.home.join(crate::copy_to_key::STATUS),
        "{\"step\":\"failed\",\"done\":true,\"said\":\"\",\"error\":\"cancelled\"}\n",
    )
    .expect("status");
    assert_eq!(page.api("POST", "/api/copy-to-key", &body).status, 202);
}

#[test]
fn the_usb_section_has_a_handler_for_each_of_its_controls() {
    let html = super::INDEX_HTML;
    let section = &html[html.find("id=\"st-usb\"").expect("section")..];
    let section = &section[..section.find("</section>").expect("end")];
    let mut ids = Vec::new();
    for piece in section
        .split("<button")
        .skip(1)
        .chain(section.split("<input").skip(1))
    {
        if let Some(start) = piece.find("id=\"") {
            let rest = &piece[start + 4..];
            ids.push(rest[..rest.find('"').expect("quote")].to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    for id in [
        "st-usb-browse",
        "st-usb-go",
        "st-usb-path",
        "st-usb-refresh",
    ] {
        assert!(ids.iter().any(|found| found == id), "no #{id} in {ids:?}");
    }
    for id in &ids {
        assert!(
            super::APP_JS.contains(&format!("$('{id}').addEventListener(")),
            "#{id} is in the USB section and app.js gives it no handler: a control that does \
             nothing"
        );
    }
    for path in ["'/api/keys'", "'/api/copy-to-key'", "'/api/pick'"] {
        assert!(
            super::APP_JS.contains(path),
            "the section never calls {path}"
        );
    }
    assert!(
        super::APP_JS.contains("section=([a-z]+)")
            && super::APP_JS.contains("location.hash === '#key'")
            && super::APP_JS.contains("if (section === 'key')"),
        "the tray's \"Copy this account to a USB key\" opens Settings at #key, and the page \
         does not go there"
    );
}
