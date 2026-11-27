//! The secrets stay in the native windows: no answer carries a word or the
//! passphrase, a mistyped passphrase stores nothing, and the page is told only
//! that a window opened and how it closed.

use super::*;

/// Every alphabetic word in `text`, lowercased.
fn words_of(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphabetic())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[test]
fn red_team_no_response_ever_carries_a_recovery_word_or_the_passphrase() {
    // Two accounts, as the engine's own test does: "account" and "machine"
    // are recovery words too, so only a word of *this* run's phrase that the
    // *other* run never sent is a leak.
    let dir = tempfile::tempdir().expect("tempdir");
    let runs: Vec<(String, String)> = ["one", "two"]
        .iter()
        .map(|name| {
            set_up_through_the_page(
                &dir.path().join(name),
                &dir.path().join(format!("{name}-base")).join("folder"),
            )
        })
        .collect();
    for (index, (phrase, heard)) in runs.iter().enumerate() {
        assert!(
            !heard.contains(PASSPHRASE),
            "the passphrase came back in an HTTP answer: every browser extension that reads \
             all sites could unlock this machine's keys"
        );
        assert!(
            !heard.contains(phrase.as_str()),
            "the 24 recovery words came back in an HTTP answer: every browser extension that \
             reads all sites could take the account"
        );
        let mine = words_of(heard);
        let other = words_of(&runs[1 - index].1);
        for word in phrase.split_whitespace() {
            assert!(
                !mine.contains(word) || other.contains(word),
                "the recovery word {word:?} came back in an HTTP answer: the page must never \
                 see a word, or an extension reading it holds part of the account"
            );
        }
    }
}

/// A person who mistypes the current passphrase, and notes whether a new one
/// was ever asked of them.
struct Mistaken {
    seen: Seen,
    asked_new: Arc<std::sync::atomic::AtomicBool>,
}

impl SecretPrompt for Mistaken {
    fn show_and_confirm(&mut self, phrase: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
        self.seen.0.lock().expect("seen").push(phrase.to_owned());
        Ok(None)
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        self.asked_new
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Zeroizing::new("another-passphrase-entirely".to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        Ok(Zeroizing::new("not-the-passphrase".to_owned()))
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        Err(CliError::Usage("not asked here".to_owned()))
    }
    fn where_asked(&self) -> &'static str {
        "a test window"
    }
}

/// Run `route` on a Settings page whose person mistypes the passphrase; the
/// state it ended in.
fn mistaken_job(home: &Path, base: &Path, route: &str, seen: &Seen) -> (String, bool) {
    let asked_new = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&asked_new);
    let page = Page::start_with(home, base, Mode::Settings, seen, move |backends| {
        let seen = seen.clone();
        backends.prompt = Box::new(move || {
            let person: Box<dyn SecretPrompt> = Box::new(Mistaken {
                seen: seen.clone(),
                asked_new: Arc::clone(&flag),
            });
            Ok(person)
        });
    });
    assert_eq!(page.api("POST", route, "").status, 202);
    let mut state = String::new();
    for _ in 0..200 {
        state = page.api("GET", "/api/state", "").body;
        if !state.contains("\"phase\":\"running\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (state, asked_new.load(std::sync::atomic::Ordering::SeqCst))
}

#[test]
fn red_team_a_mistyped_passphrase_shows_no_words_asks_no_new_one_and_stores_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let base = dir.path().join("base");
    let _ = set_up_through_the_page(&home, &base.join("folder"));
    // The copy the service reads, as an installed service would have left it.
    let stored = home.with_extension("passphrase");
    std::fs::write(&stored, PASSPHRASE).expect("stored copy");
    let seen = Seen::default();

    let (state, _) = mistaken_job(&home, &base, "/api/words", &seen);
    assert!(
        state.contains("\"phase\":\"failed\""),
        "a mistyped passphrase did not stop Show my 24 words: {state}"
    );
    assert!(
        seen.0.lock().expect("seen").is_empty(),
        "the 24 words were shown to somebody who did not know the passphrase"
    );

    let (state, asked_new) = mistaken_job(&home, &base, "/api/passphrase", &seen);
    assert!(
        state.contains("\"phase\":\"failed\""),
        "a mistyped current passphrase did not stop the change: {state}"
    );
    assert!(
        !asked_new,
        "a new passphrase was asked of somebody who had not proved the current one"
    );
    assert_eq!(
        std::fs::read_to_string(&stored).expect("stored"),
        PASSPHRASE,
        concat!(
            "the service's copy was replaced by a change that was refused: it would start ",
            "with a passphrase that opens nothing"
        )
    );
    assert!(
        itsanas_node::node::Identity::secrets(&home, PASSPHRASE).is_ok(),
        "a refused change re-sealed the keystore"
    );
}

#[test]

fn settings_steer_through_the_control_file_and_change_the_pledge_through_the_engine() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let _ = set_up_through_the_page(&home, &dir.path().join("base").join("folder"));
    let seen = Seen::default();
    let setup = Page::start(&home, dir.path(), Mode::Setup, &seen);
    assert_eq!(
        setup.api("POST", "/api/control", "action=pause").status,
        404,
        "the setup page answered a Settings action"
    );
    drop(setup);
    let page = Page::start(&home, dir.path(), Mode::Settings, &seen);
    let paused = page.api("POST", "/api/control", "action=pause&for=resume");
    assert_eq!(paused.status, 200, "pause was refused: {}", paused.body);
    assert!(
        crate::control::paused_on_disk(&home),
        "the page said paused and the control file does not: the daemon would keep syncing"
    );
    let timed = page.api("POST", "/api/control", "action=pause&for=1h");
    assert_eq!(
        timed.status, 200,
        "a pause for 1 hour was refused: {}",
        timed.body
    );
    assert!(
        crate::control::Control::read(&home).is_ok_and(|c| c.until.is_some()),
        "the page's 'for 1 hour' wrote a pause with no end: it would never end by itself"
    );
    assert!(
        page.api("GET", "/api/state", "")
            .body
            .contains("\"paused\":true")
    );
    assert_eq!(
        page.api("POST", "/api/control", "action=resume").status,
        200
    );
    assert!(
        !crate::control::paused_on_disk(&home),
        "resume did not resume"
    );
    assert_eq!(page.api("POST", "/api/run", "pledge=2M").status, 202);
    let deadline = Instant::now() + Duration::from_secs(300);
    while page
        .api("GET", "/api/state", "")
        .body
        .contains("\"phase\":\"running\"")
    {
        assert!(Instant::now() < deadline, "the pledge change never ended");
        std::thread::sleep(Duration::from_millis(200));
    }
    let ended = page.api("GET", "/api/state", "").body;
    let config =
        crate::config::Config::load(&crate::node::Node::config_path(&home)).expect("config");
    assert_eq!(
        config.pledge_bytes,
        2 * 1024 * 1024,
        "the pledge changed in the page is not the node's: the person offers what they did not choose; the page ended with {ended}"
    );
    let file = home.with_extension("passphrase");
    std::fs::write(&file, "stand-in\n").expect("stand-in passphrase file");
    let out = page.api("POST", "/api/signout", "");
    assert_eq!(out.status, 200, "sign out was refused: {}", out.body);
    assert!(
        !file.exists(),
        "the page said signed out and the passphrase file is still there: the service would \
         start again at the next logon as if nothing was asked"
    );
}

/// A person who leaves the window open until the test closes it.
struct SlowPerson {
    closed: std::sync::mpsc::Receiver<()>,
}

impl SecretPrompt for SlowPerson {
    fn show_and_confirm(&mut self, _: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
        let _ = self.closed.recv();
        Err(CliError::Usage("the window was closed".to_owned()))
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        Err(CliError::Usage("not asked in this test".to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        self.new_passphrase()
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        self.new_passphrase()
    }
    fn where_asked(&self) -> &'static str {
        "a test window"
    }
}

/// The state's `run` until `done` says so, polled as the page polls.
fn poll_until(page: &Page, done: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let body = page.api("GET", "/api/state", "").body;
        if done(&body) {
            return body;
        }
        assert!(Instant::now() < deadline, "the run never got there: {body}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn while_a_window_is_open_the_page_is_told_so_and_a_closed_window_says_what_to_do() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (close, closed) = std::sync::mpsc::channel();
    let closed = Mutex::new(Some(closed));
    let file = home.with_extension("passphrase");
    let backends = Backends {
        prompt: Box::new(move || {
            let closed = closed.lock().expect("lock").take().expect("one run");
            let person: Box<dyn SecretPrompt> = Box::new(SlowPerson { closed });
            Ok(person)
        }),
        service: Box::new(move || {
            let service: Box<dyn ServiceControl> = Box::new(NoService { file: file.clone() });
            service
        }),
        name: |_, _| Ok(false),
        owner: |_, _| Ok(None),
        pledge: |_, _| None,
        another: |_| Ok(()),
        drives: Box::new(Vec::new),
        programs: || None,
        copy: Box::new(|_, _| Err("no copy is started in this test".to_owned())),
        features: Features::DESKTOP,
        phone: false,
        fixed_coordinator: None,
        host: Arc::new(NoHost),
        nodes: test_nodes(&Seen::default(), &home),
    };
    let server = Server::bind(&home, None, Mode::Setup, dir.path(), backends).expect("bind");
    let ctx = Arc::clone(&server.ctx);
    let page = Page {
        ctx,
        server: Some(std::thread::spawn(move || {
            server.serve(Duration::from_secs(300))
        })),
    };
    let started = page.api(
        "POST",
        "/api/run",
        "account=new&username=camille&background=no",
    );
    assert_eq!(started.status, 202, "{}", started.body);
    let waiting = poll_until(&page, |body| !body.contains("\"waiting\":null"));
    assert!(
        waiting.contains("\"phase\":\"running\"") && waiting.contains("24 recovery words"),
        "while the window was open the page was not told so: the person would stare at a page \
         that seems stuck, with the window hidden behind it ({waiting})"
    );
    assert_eq!(
        page.api("POST", "/api/run", "account=new&username=camille")
            .status,
        409,
        "a second run started while the first waited on its window: two windows, two accounts"
    );
    close.send(()).expect("close the window");
    let failed = poll_until(&page, |body| body.contains("\"phase\":\"failed\""));
    assert!(
        failed.contains("\"waiting\":null") && failed.contains("\"remedy\":\""),
        "a closed window left the page waiting, or without the one thing to do: {failed}"
    );
    assert!(
        !home.join("keystore.bin").exists(),
        "an account was written although its words were never confirmed"
    );
}

#[test]
fn the_page_asks_no_secret_loads_nothing_from_elsewhere_and_sends_its_key_in_a_header() {
    let html = super::INDEX_HTML;
    let js = super::APP_JS;
    assert!(
        !html.contains("type=\"password\"") && !html.contains("<textarea"),
        "the page has a field a passphrase or the words could be typed into: the browser is the \
         one place they must never go"
    );
    for sentence in [
        "Never type your recovery words into a web page",
        "A separate ITSaNAS window has opened in front of this page.",
        "look for ITSaNAS in your taskbar / Dock",
    ] {
        assert!(
            html.contains(sentence),
            "the page lost {sentence:?}: a person would look for the secret on the page, or \
             type it there"
        );
    }
    for (name, text) in [
        ("index.html", html),
        ("app.js", js),
        ("app.css", super::APP_CSS),
    ] {
        // The three links of the legal block are followed by the person, in a tab of
        // their own; `red_team_the_page_links_only_...` holds them to that.
        let text = text.replace("href=\"https://github.com/SigSegGit/itsanas", "href=\"");
        assert!(
            !text.contains("http://") && !text.contains("https://") && !text.contains("//cdn"),
            "{name} names another address: the page would load code from elsewhere, which the \
             CSP then blocks, or worse"
        );
    }
    assert!(
        js.contains("'X-Itsanas-Token'") && js.contains("history.replaceState"),
        "app.js does not send the key as a header, or leaves it in the address bar"
    );
    assert!(
        !js.contains("innerHTML") && !js.contains("eval("),
        "app.js writes HTML from data: a value from the server could become script"
    );
}
