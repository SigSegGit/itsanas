//! The page's server, spoken to over real sockets by a plain `TcpStream`.
//!
//! Each test starts the server in this process on a throwaway home, with a
//! scripted person in place of the native windows and no service manager.
//! What is proved is the door, not the furniture: who is refused (another
//! Host, no token, another origin, an oversized request), what every answer
//! carries, and that nothing that comes back ever holds a secret -- a whole
//! setup driven through the API, every byte read.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    io::{Read as _, Write as _},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use zeroize::Zeroizing;

use super::{
    Backends, Context, LANGUAGE_FILE, Mode, Server, browser_command, has_desktop, http,
    language_of, suggest_pledge,
};
use crate::{
    error::{CliError, Result},
    setup::{Secret, SecretPrompt, ServiceControl},
};

const PASSPHRASE: &str = "web-page-test-passphrase";

/// Every phrase the scripted person was shown, as the window would show it.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<String>>>);

/// A person in front of the native window: types back the right words and
/// `PASSPHRASE` whenever asked.
struct Person {
    seen: Seen,
}

impl SecretPrompt for Person {
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        positions: [usize; 3],
    ) -> Result<Option<Vec<Secret>>> {
        self.seen.0.lock().expect("seen").push(phrase.to_owned());
        let words: Vec<&str> = phrase.split_whitespace().collect();
        Ok(Some(
            positions
                .iter()
                .map(|&position| Zeroizing::new(words[position - 1].to_owned()))
                .collect(),
        ))
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(PASSPHRASE.to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        self.new_passphrase()
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        Err(CliError::Usage(
            "no account is joined in these tests".to_owned(),
        ))
    }
    fn where_asked(&self) -> &'static str {
        "a test window"
    }
}

/// A machine with no service manager; installing one is a failure said as one.
struct NoService {
    file: PathBuf,
}

impl ServiceControl for NoService {
    fn passphrase_file(&self) -> PathBuf {
        self.file.clone()
    }
    fn installed(&self) -> bool {
        false
    }
    fn install(&self, _: bool) -> Result<String> {
        Err(CliError::Usage(
            "the page installed a service it was told not to".to_owned(),
        ))
    }
    fn start(&self) -> Result<()> {
        Err(CliError::Usage(
            "the page started a service it was told not to".to_owned(),
        ))
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }
    fn set_autostart(&self, _: bool) -> Result<()> {
        Err(CliError::Usage(
            "the page changed a service it was told not to".to_owned(),
        ))
    }
    fn log_hint(&self) -> String {
        "the test".to_owned()
    }
}

fn backends(seen: &Seen, home: &Path) -> Backends {
    let seen = seen.clone();
    let file = home.with_extension("passphrase");
    Backends {
        prompt: Box::new(move || {
            let person: Box<dyn SecretPrompt> = Box::new(Person { seen: seen.clone() });
            Ok(person)
        }),
        service: Box::new(move || {
            let service: Box<dyn ServiceControl> = Box::new(NoService { file: file.clone() });
            service
        }),
        name: |_, name| Ok(name == "taken"),
        another: |_| Ok(()),
        drives: Box::new(Vec::new),
        programs: || Some(1024),
        copy: Box::new(|_, _| Err("no copy is started in these tests".to_owned())),
    }
}

/// A server running on its own thread, stopped when dropped.
struct Page {
    ctx: Arc<Context>,
    server: Option<JoinHandle<bool>>,
}

impl Page {
    fn start(home: &Path, base: &Path, mode: Mode, seen: &Seen) -> Self {
        Self::start_with(home, base, mode, seen, |_| {})
    }

    /// [`Page::start`], with some backends replaced.
    fn start_with(
        home: &Path,
        base: &Path,
        mode: Mode,
        seen: &Seen,
        change: impl FnOnce(&mut Backends),
    ) -> Self {
        let mut backends = backends(seen, home);
        change(&mut backends);
        let server = Server::bind(home, None, mode, base, backends).expect("bind");
        let ctx = Arc::clone(&server.ctx);
        let handle = std::thread::spawn(move || server.serve(Duration::from_secs(300)));
        Self {
            ctx,
            server: Some(handle),
        }
    }

    fn port(&self) -> u16 {
        self.ctx.port
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port())
    }

    /// A request with this server's Host, the given headers and body.
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut raw = format!("{method} {path} HTTP/1.1\r\n");
        for (name, value) in headers {
            let _ = write!(raw, "{name}: {value}\r\n");
        }
        if !body.is_empty() {
            let _ = write!(
                raw,
                "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
                body.len()
            );
        }
        raw.push_str("\r\n");
        raw.push_str(body);
        exchange(self.port(), raw.as_bytes())
    }

    /// An API call as the page makes it: right Host, right token.
    fn api(&self, method: &str, path: &str, body: &str) -> Reply {
        let host = self.host();
        let token = self.ctx.token.clone();
        self.send(
            method,
            path,
            &[("Host", &host), ("X-Itsanas-Token", &token)],
            body,
        )
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.ctx
            .quit
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

/// One answer: status, the head as text, the body as text.
#[derive(Debug)]
struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn all(&self) -> String {
        format!("{}\r\n\r\n{}", self.head, self.body)
    }

    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    }
}

fn exchange(port: u16, raw: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    // A server that refuses early may close before all of it is written.
    let _ = stream.write_all(raw);
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    Reply {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

#[test]
fn a_foreign_host_is_refused_before_anything_is_served() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let port = page.port();
    for (host, why) in [
        (format!("evil.example:{port}"), "a rebinding name"),
        (
            format!("127.0.0.1:{}", port.wrapping_add(1)),
            "another port",
        ),
        ("127.0.0.1".to_owned(), "no port"),
        (
            format!("localhost.evil.example:{port}"),
            "a name that starts like localhost",
        ),
    ] {
        let reply = page.send("GET", "/", &[("Host", &host)], "");
        assert_eq!(
            reply.status, 403,
            "{why} ({host}) was served the page: a site that points its own name at 127.0.0.1 \
             would read this machine's setup as its own"
        );
    }
    assert_eq!(
        page.send("GET", "/", &[], "").status,
        403,
        "a request with no Host was served"
    );
    let ours = page.host();
    assert_eq!(
        page.send("GET", "/", &[("Host", &ours), ("Host", "evil.example")], "")
            .status,
        403,
        "two Host headers were served: a proxy and this server could each read a different one"
    );
    for host in [ours, format!("localhost:{port}")] {
        assert_eq!(
            page.send("GET", "/", &[("Host", &host)], "").status,
            200,
            "the page refused its own address {host}: the person would see an error, not setup"
        );
    }
}

#[test]
fn red_team_a_rebinding_host_with_the_right_token_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let evil = format!("evil.example:{}", page.port());
    let token = page.ctx.token.clone();
    for (method, path) in [("GET", "/api/state"), ("POST", "/api/quit")] {
        let reply = page.send(
            method,
            path,
            &[("Host", &evil), ("X-Itsanas-Token", &token)],
            "",
        );
        assert_eq!(
            reply.status, 403,
            "{method} {path} under Host {evil} was answered although the token was right: a \
             token that leaked once (a screenshot, a shared terminal) would let any web page \
             that rebinds its name drive this machine's setup"
        );
    }
    assert_eq!(
        page.api("GET", "/api/state", "").status,
        200,
        "the refused quit stopped the server anyway"
    );
}

#[test]
fn the_api_refuses_a_missing_or_wrong_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let host = page.host();
    let right = page.ctx.token.clone();
    let mut wrong = right.clone().into_bytes();
    wrong[31] = if wrong[31] == b'0' { b'1' } else { b'0' };
    let wrong = String::from_utf8(wrong).expect("hex");
    for (headers, why) in [
        (vec![("Host", host.as_str())], "no token"),
        (
            vec![("Host", host.as_str()), ("X-Itsanas-Token", wrong.as_str())],
            "a token one digit off",
        ),
        (
            vec![
                ("Host", host.as_str()),
                ("X-Itsanas-Token", wrong.as_str()),
                ("X-Itsanas-Token", right.as_str()),
            ],
            "the right token beside a wrong one",
        ),
    ] {
        for (method, path) in [
            ("GET", "/api/state"),
            ("GET", "/api/plan"),
            ("POST", "/api/run"),
        ] {
            assert_eq!(
                page.send(method, path, &headers, "").status,
                403,
                "{method} {path} with {why} was answered: any local process or page could read \
                 this machine's state or start its setup"
            );
        }
    }
    assert_eq!(
        page.api("GET", "/api/state", "").status,
        200,
        "the right token was refused: the page could never work"
    );
}

#[test]
fn a_cross_origin_request_is_refused_and_no_cors_header_is_sent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let host = page.host();
    let token = page.ctx.token.clone();
    let port = page.port();
    let other_port = format!("http://127.0.0.1:{}", port.wrapping_add(1));
    for (name, value, why) in [
        ("Origin", "http://evil.example", "another site"),
        ("Origin", "null", "a sandboxed frame or a file"),
        (
            "Origin",
            other_port.as_str(),
            "another port of this machine",
        ),
        (
            "Sec-Fetch-Site",
            "cross-site",
            "a fetch the browser marks cross-site",
        ),
    ] {
        let reply = page.send(
            "POST",
            "/api/quit",
            &[("Host", &host), ("X-Itsanas-Token", &token), (name, value)],
            "",
        );
        assert_eq!(
            reply.status, 403,
            "a POST from {why} ({name}: {value}) was obeyed: a page the person happens to have \
             open could change their settings or sign them out"
        );
    }
    let ours = format!("http://localhost:{port}");
    let reply = page.send(
        "GET",
        "/api/state",
        &[
            ("Host", &host),
            ("X-Itsanas-Token", &token),
            ("Origin", &ours),
        ],
        "",
    );
    assert_eq!(reply.status, 200, "the page's own origin was refused");
    assert!(
        !reply.head.to_ascii_lowercase().contains("access-control-"),
        "a CORS header was sent: it is the only way another origin could read an answer"
    );
}

#[test]
fn oversized_requests_are_refused_without_being_read_whole() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    // A head one byte over the limit and never finished: only a server that
    // stops reading at the limit can answer at all.
    let mut head = format!("GET / HTTP/1.1\r\nHost: {}\r\nX-Pad: ", page.host()).into_bytes();
    head.resize(http::MAX_HEAD + 1, b'a');
    let started = Instant::now();
    let reply = exchange(page.port(), &head);
    assert_eq!(
        reply.status,
        431,
        "a head over {} bytes was not refused: a local process could make this server hold \
         as much memory as it sends",
        http::MAX_HEAD
    );
    assert!(
        started.elapsed() < super::READ_TIMEOUT,
        "the oversized head was answered only at the read timeout: the server waited for the \
         rest instead of refusing at the limit"
    );
    // A body announced at 10 MB and never sent: refused on the announcement.
    for length in [http::MAX_BODY + 1, 10_000_000] {
        let raw = format!(
            "POST /api/run HTTP/1.1\r\nHost: {}\r\nX-Itsanas-Token: {}\r\nContent-Length: \
             {length}\r\n\r\n",
            page.host(),
            page.ctx.token
        );
        let started = Instant::now();
        let reply = exchange(page.port(), raw.as_bytes());
        assert_eq!(
            reply.status, 413,
            "a body announced at {length} bytes was not refused on its announcement"
        );
        assert!(
            started.elapsed() < super::READ_TIMEOUT,
            "a body announced at {length} bytes was waited for before being refused"
        );
    }
}

#[test]
fn red_team_connections_dripping_bytes_cannot_starve_the_page() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let port = page.port();
    // Every slot taken by a client that sends one byte every 2 s: under the
    // 5 s per-read timeout, so only a total deadline can drop it.
    let drippers: Vec<_> = (0..super::MAX_CONNECTIONS)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
                    return;
                };
                for byte in b"GET / HTTP/1.1\r\nX-Slow: aaaaaaaaaaaaaaaaaaaaaa"
                    .iter()
                    .cycle()
                {
                    if stop.load(Ordering::SeqCst) || stream.write_all(&[*byte]).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
        })
        .collect();
    std::thread::sleep(super::CONNECTION_DEADLINE + Duration::from_secs(2));
    let asked = Instant::now();
    let reply = page.api("GET", "/api/state", "");
    stop.store(true, Ordering::SeqCst);
    for dripper in drippers {
        let _ = dripper.join();
    }
    assert_eq!(
        reply.status, 200,
        "with every slot held by a client dripping bytes, the person's page got no answer: a \
         local process could stall setup until the idle exit"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(5),
        "the page was answered only after {:?}",
        asked.elapsed()
    );
}

#[test]
fn every_response_says_no_store_and_forbids_framing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let host = page.host();
    let mut oversized = format!("GET / HTTP/1.1\r\nHost: {host}\r\nX-Pad: ").into_bytes();
    oversized.resize(http::MAX_HEAD + 1, b'a');
    let replies = [
        page.send("GET", "/", &[("Host", &host)], ""),
        page.send("GET", "/app.js", &[("Host", &host)], ""),
        page.send("GET", "/app.css", &[("Host", &host)], ""),
        page.send("GET", "/nothing", &[("Host", &host)], ""),
        page.send("GET", "/", &[("Host", "evil.example")], ""),
        page.send("GET", "/api/state", &[("Host", &host)], ""),
        page.api("GET", "/api/state", ""),
        exchange(page.port(), &oversized),
    ];
    for reply in &replies {
        let says = |name: &str| reply.header(name).unwrap_or_default();
        assert_eq!(
            says("Cache-Control"),
            "no-store",
            "a {} answer may be cached: the state of a person's machine would stay on disk in \
             the browser's cache",
            reply.status
        );
        let csp = says("Content-Security-Policy");
        assert!(
            csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'self'"),
            "a {} answer's CSP is {csp:?}: another page could frame this one, or a script it \
             did not serve could run in it",
            reply.status
        );
        assert_eq!(
            says("X-Frame-Options"),
            "DENY",
            "a {} answer may be framed",
            reply.status
        );
        assert_eq!(says("X-Content-Type-Options"), "nosniff");
        assert_eq!(says("Referrer-Policy"), "no-referrer");
        assert_eq!(says("Connection"), "close");
    }
}

/// Every alphabetic word in `text`, lowercased.
fn words_of(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphabetic())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A whole setup through the API, as the page drives it; every answer that
/// came back, and the phrase the person was shown in the "window".
fn set_up_through_the_page(home: &Path, folder: &Path) -> (String, String) {
    let seen = Seen::default();
    let page = Page::start(home, folder.parent().expect("parent"), Mode::Setup, &seen);
    let mut heard = String::new();
    for path in ["/", "/app.js", "/app.css"] {
        heard.push_str(&page.send("GET", path, &[("Host", &page.host())], "").all());
    }
    heard.push_str(&page.api("GET", "/api/plan", "").all());
    let form = format!(
        "account=new&username=camille&pledge=1M&background=no&folder={}",
        folder
            .display()
            .to_string()
            .replace('\\', "%5C")
            .replace(':', "%3A")
    );
    let started = page.api("POST", "/api/run", &form);
    assert_eq!(
        started.status, 202,
        "the run was not started: {}",
        started.body
    );
    heard.push_str(&started.all());
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let state = page.api("GET", "/api/state", "");
        heard.push_str(&state.all());
        if !state.body.contains("\"phase\":\"running\"") {
            assert!(
                state.body.contains("\"phase\":\"done\""),
                "setup through the page did not finish: {}",
                state.body
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "setup through the page never ended"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    heard.push_str(&page.api("GET", "/api/plan", "").all());
    let phrase = seen
        .0
        .lock()
        .expect("seen")
        .first()
        .cloned()
        .expect("words shown");
    (phrase, heard)
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
        another: |_| Ok(()),
        drives: Box::new(Vec::new),
        programs: || None,
        copy: Box::new(|_, _| Err("no copy is started in this test".to_owned())),
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

#[test]
fn a_desktop_is_needed_for_the_page_and_ssh_never_counts_as_one() {
    for (os, ssh, display, expected, why) in [
        ("windows", false, false, true, "a Windows session"),
        ("macos", false, false, true, "a Mac"),
        ("linux", false, true, true, "a Linux desktop"),
        (
            "linux",
            false,
            false,
            false,
            "a Linux console with no display",
        ),
        ("windows", true, false, false, "Windows over SSH"),
        ("linux", true, true, false, "SSH with X forwarding"),
    ] {
        assert_eq!(
            has_desktop(os, ssh, display),
            expected,
            "{why}: a browser would open where nobody sees it, or the terminal would be skipped"
        );
    }
}

#[test]
fn the_browser_is_opened_by_the_systems_own_program_with_the_url_as_one_argument() {
    let url = "http://127.0.0.1:5000/#t=0123456789abcdef0123456789abcdef&x=1";
    for (os, program, before) in [
        ("windows", "rundll32", vec!["url.dll,FileProtocolHandler"]),
        ("macos", "open", vec![]),
        ("linux", "xdg-open", vec![]),
    ] {
        let command = browser_command(os, url);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let mut expected: Vec<String> = before.iter().map(|arg| (*arg).to_owned()).collect();
        expected.push(url.to_owned());
        assert_eq!(command.get_program().to_string_lossy(), program, "{os}");
        assert_eq!(
            args, expected,
            "on {os} the URL is not one whole argument: a `&` would cut it, and the page would \
             open without its key"
        );
    }
}

#[test]
fn the_suggested_pledge_is_a_fifth_in_whole_gib_and_capped() {
    const GIB: u64 = 1 << 30;
    assert_eq!(
        suggest_pledge(100 * GIB),
        20 * GIB,
        "a fifth of the free space"
    );
    assert_eq!(
        suggest_pledge(GIB * 7 + 123),
        GIB,
        "rounded down to whole GiB"
    );
    assert_eq!(
        suggest_pledge(GIB),
        0,
        "a nearly full disk is offered nothing"
    );
    assert_eq!(
        suggest_pledge(100_000 * GIB),
        500 * GIB,
        "a huge disk would be suggested more than anybody means to give"
    );
}

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
fn red_team_a_chosen_path_never_lands_inside_a_script() {
    let start = Path::new(r"C:\x'; Remove-Item -Recurse C:\ #");
    for os in ["windows", "macos", "linux"] {
        let command = super::picker_command(os, start);
        let text: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let in_script = text
            .iter()
            .any(|arg| arg.contains("Remove-Item") && !arg.starts_with("--filename="));
        assert!(
            !in_script,
            "{os}: the start folder was written into the chooser's script, so a folder name \
             runs as code: {text:?}"
        );
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "ITSANAS_PICK_START" && value == Some(start.as_os_str())),
            "{os}: the start folder does not reach the chooser"
        );
    }
}

#[test]
fn a_gtk_bookmark_escapes_what_a_url_cannot_hold() {
    assert_eq!(
        super::gtk_bookmark(Path::new("/home/sam/My Files/été")),
        "file:///home/sam/My%20Files/%C3%A9t%C3%A9",
        "a space or an accent in the folder makes the bookmark point nowhere"
    );
}

/// Every text a person reads in the page, as the English the translations
/// are keyed by: the text between tags (outside scripts and code), the
/// placeholders and labels, and what app.js passes to `t(...)`.
fn texts_of_the_page() -> Vec<String> {
    let html = super::INDEX_HTML;
    let body = &html[html.find("<body>").expect("body")..];
    let mut texts = Vec::new();
    let mut skip = 0_i32;
    for piece in body.split('<').skip(1) {
        let (tag, text) = piece.split_once('>').unwrap_or((piece, ""));
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if matches!(name.as_str(), "script" | "style" | "code") {
            skip += if tag.starts_with('/') { -1 } else { 1 };
        }
        for attribute in ["placeholder=\"", "aria-label=\""] {
            if let Some(start) = tag.find(attribute) {
                let rest = &tag[start + attribute.len()..];
                texts.push(rest[..rest.find('"').expect("quote")].to_owned());
            }
        }
        let text = text
            .replace("&mdash;", "—")
            .replace("&hellip;", "…")
            .replace("&amp;", "&");
        let text = text.trim();
        if skip == 0 && !text.is_empty() {
            texts.push(text.to_owned());
        }
    }
    // `t('` as a call, not the end of `createElement('`; `N('` marks a text
    // of a table translated where it is shown; `say('` translates its text.
    let js = super::APP_JS;
    for call in ["t('", "N('", "say('"] {
        let mut from = 0;
        while let Some(at) = js[from..].find(call) {
            let start = from + at;
            from = start + call.len();
            if js[..start].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            let rest = &js[from..];
            let text = rest[..rest.find("')").expect("end of the call")].replace("\\'", "'");
            // `say('')` clears the line: nothing to translate.
            if !text.is_empty() {
                texts.push(text);
            }
        }
    }
    for label in super::APP_JS.split("label: '").skip(1) {
        texts.push(label[..label.find('\'').expect("label")].to_owned());
    }
    texts
}

/// One JavaScript string literal at the start of `text`: its value and what
/// follows it.
fn js_string(text: &str) -> (String, &str) {
    let quote = text.chars().next().expect("a string");
    assert!(
        quote == '\'' || quote == '"',
        "not a string in i18n.js: {}",
        &text[..text.len().min(60)]
    );
    let mut value = String::new();
    let mut chars = text[1..].char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => match chars.next().expect("an escape").1 {
                'n' => value.push('\n'),
                other => value.push(other),
            },
            c if c == quote => return (value, &text[1 + at + 1..]),
            c => value.push(c),
        }
    }
    panic!("a string in i18n.js never ends");
}

/// The French table of i18n.js, entry by entry, as the browser reads it. A
/// line the reader does not understand is a failure, not a skipped entry.
fn french_table() -> Vec<(String, String)> {
    let js = super::I18N_JS;
    let mut rest = &js[js.find("fr: {").expect("the fr table") + "fr: {".len()..];
    let mut entries = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.starts_with('}') {
            return entries;
        }
        if let Some(comment) = rest.strip_prefix("//") {
            rest = comment.split_once('\n').map_or("", |(_, next)| next);
            continue;
        }
        let (key, after) = js_string(rest);
        let after = after
            .trim_start()
            .strip_prefix(':')
            .unwrap_or_else(|| panic!("no ':' after {key:?} in i18n.js"));
        let (value, after) = js_string(after.trim_start());
        entries.push((key, value));
        rest = after;
    }
}

fn french_keys() -> BTreeSet<String> {
    french_table().into_iter().map(|(key, _)| key).collect()
}

#[test]
fn the_french_table_has_one_entry_per_english_text_and_none_empty() {
    let table = french_table();
    assert!(
        table.len() > 100,
        "i18n.js was misread: {} entries",
        table.len()
    );
    let mut keys = BTreeSet::new();
    for (english, french) in &table {
        assert!(
            keys.insert(english.as_str()),
            "{english:?} is twice in the French table: the browser keeps the last one, and the \
             first, maybe the one corrected, is silently dropped"
        );
        assert!(
            !french.trim().is_empty(),
            "{english:?} has an empty French entry: the page would show nothing there"
        );
    }
}

#[test]
fn every_text_of_the_page_has_a_french_translation() {
    let french = french_keys();
    let untranslated: Vec<String> = texts_of_the_page()
        .into_iter()
        .filter(|text| !matches!(text.as_str(), "ITSaNAS" | "English" | "Français"))
        .filter(|text| !french.contains(text))
        .collect();
    assert!(
        untranslated.is_empty(),
        "a French speaker would read these in English, in the middle of French ones: \
         add them to i18n.js: {untranslated:#?}"
    );
}

#[test]
fn no_text_set_by_the_script_skips_the_translation() {
    // Emptying a line (`textContent = ''`) shows no text.
    let js = super::APP_JS
        .replace("textContent = ''", "")
        .replace("textContent = \"\"", "");
    for bypass in [
        "textContent = '",
        "textContent = \"",
        "confirm('",
        "confirm(\"",
        "textContent = reply.said",
        "settings.job.said;",
    ] {
        assert!(
            !js.contains(bypass),
            "app.js sets a visible text with {bypass:?}: it shows in English whatever the \
             language chosen (hand test, 2026-10-22); pass it through t(), N() or said()"
        );
    }
}

#[test]
fn the_page_speaks_the_browser_s_language_until_one_is_chosen() {
    for (accept, language) in [
        ("fr-FR,fr;q=0.9,en-US;q=0.8,en;q=0.7", Some("fr")),
        ("fr", Some("fr")),
        ("FR-ca", Some("fr")),
        ("en-GB,en;q=0.9,fr;q=0.8", Some("en")),
        ("de-DE,de;q=0.9,fr;q=0.8,en;q=0.5", Some("fr")),
        ("en;q=0.5, fr;q=0.6", Some("fr")),
        ("fr;q=0, en", Some("en")),
        ("de-DE, *;q=0.1", None),
        ("fr;q=bogus", None),
        ("", None),
    ] {
        assert_eq!(language_of(accept), language, "Accept-Language: {accept:?}");
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let host = page.host();
    let french = page.send(
        "GET",
        "/",
        &[
            ("Host", &host),
            ("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8"),
        ],
        "",
    );
    assert!(
        french.body.contains("<html lang=\"fr\" data-lang=\"fr\">"),
        "a browser asking for French got a page that starts in English (hand test, \
         2026-10-22): {}",
        &french.body[..french.body.len().min(200)]
    );
    let english = page.send(
        "GET",
        "/",
        &[("Host", &host), ("Accept-Language", "en-US")],
        "",
    );
    assert!(english.body.contains("<html lang=\"en\" data-lang=\"en\">"));
    let unsaid = page.send("GET", "/", &[("Host", &host)], "");
    assert!(
        unsaid.body.contains("<html lang=\"en\">"),
        "with no Accept-Language the page must leave the choice to the browser's own language"
    );
}

#[test]
fn a_language_chosen_in_the_page_is_kept_in_the_node_s_home_and_wins_over_the_browser() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let host = page.host();
    assert_eq!(
        page.send("POST", "/api/language", &[("Host", &host)], "lang=fr")
            .status,
        403,
        "a language was set without the page's key"
    );
    assert_eq!(page.api("POST", "/api/language", "lang=de").status, 400);
    assert_eq!(page.api("POST", "/api/language", "lang=..%2Fx").status, 400);
    let chosen = page.api("POST", "/api/language", "lang=fr");
    assert_eq!(chosen.status, 200, "{}", chosen.body);
    assert_eq!(
        std::fs::read_to_string(home.join(LANGUAGE_FILE)).expect("language file"),
        "fr\n",
        "the choice was not kept beside the node: the next page, on another port, forgets it"
    );
    drop(page);
    // The next `itsanas settings`: a new server, a browser that asks for English.
    let next = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let host = next.host();
    let reply = next.send(
        "GET",
        "/",
        &[("Host", &host), ("Accept-Language", "en-US")],
        "",
    );
    assert!(
        reply.body.contains("data-lang=\"fr\""),
        "French was chosen in a page of this node and the next page opened in English"
    );

    // During a setup, before the node's home exists: nothing is made for it.
    let fresh = dir.path().join("fresh");
    let setup = Page::start(&fresh, dir.path(), Mode::Setup, &Seen::default());
    let early = setup.api("POST", "/api/language", "lang=fr");
    assert_eq!(early.status, 200, "{}", early.body);
    assert!(early.body.contains("\"kept\":false"), "{}", early.body);
    assert!(
        !fresh.exists(),
        "choosing a language made the node's home before setup did"
    );
}

/// The text of a JSON string field in a flat answer: `said` or `error`.
fn json_field(body: &str, name: &str) -> String {
    let start = body
        .find(&format!("\"{name}\":\""))
        .unwrap_or_else(|| panic!("no {name} in {body}"))
        + name.len()
        + 4;
    let mut out = String::new();
    let mut chars = body[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return out,
            '\\' => match chars.next().expect("escape") {
                'n' => out.push('\n'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(
                        char::from_u32(u32::from_str_radix(&hex, 16).expect("hex")).expect("char"),
                    );
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    panic!("unterminated {name} in {body}");
}

#[test]
fn what_settings_hears_from_the_server_has_a_french_translation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    // Enough of a node for the control file; no daemon runs it.
    std::fs::write(home.join("keystore.bin"), b"x").expect("keystore");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let mut heard = Vec::new();
    for (path, body) in [
        ("/api/control", "action=pause&for=resume"),
        ("/api/control", "action=pause&for=resume"),
        ("/api/control", "action=sync-now"),
        ("/api/control", "action=resume"),
        ("/api/control", "action=resume"),
        ("/api/control", "action=sync-now"),
        ("/api/control", "action=interval&every=auto"),
        ("/api/disk", "action=export&path=%2Fx"),
    ] {
        let reply = page.api("POST", path, body);
        let field = if reply.status == 200 { "said" } else { "error" };
        heard.extend(json_field(&reply.body, field).lines().map(str::to_owned));
    }
    let french = french_keys();
    let untranslated: Vec<&String> = heard
        .iter()
        .filter(|line| !french.contains(*line))
        .collect();
    assert!(
        untranslated.is_empty(),
        "Settings shows what the server says line by line through i18n.js; these lines would \
         stay English in a French page: {untranslated:#?}"
    );
}

#[test]
fn settings_is_one_column_of_cards_with_the_usb_copy_shown() {
    assert!(
        super::APP_CSS.contains("nav[hidden] + main { grid-column: 1 / -1; }"),
        "with the steps' list hidden, Settings falls into the list's 260 px column (hand test, \
         2026-10-22)"
    );
    let html = super::INDEX_HTML;
    let settings = &html[html.find("data-page=\"settings\"").expect("settings")
        ..html.find("data-page=\"closed\"").expect("closed")];
    assert_eq!(
        settings.matches("<section class=\"card\"").count(),
        settings.matches("<h2").count(),
        "a heading of Settings is outside a card of its own"
    );
    let slot = &settings[settings
        .find("id=\"st-usb\"")
        .expect("the section \"Copy this account to a USB key\" is gone")..];
    let slot = &slot[..slot.find("</section>").expect("end of the section")];
    assert!(slot.contains("Copy this account to a USB key"));
    assert!(
        slot.starts_with("id=\"st-usb\" aria-labelledby=\"st-h-usb\">"),
        "the USB copy section is hidden: the tray's \"Copy this account to a USB key\" opens \
         a page without it"
    );
    assert!(
        slot.contains("IS this account"),
        "the section copies an account without saying that whoever opens the key is it"
    );
}

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
        candidate(&small, Some(1 << 20), Some(crate::drives::Format::Fat32)),
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
    let rows: Vec<&str> = reply.body.split("{\"path\":").skip(1).collect();
    assert_eq!(rows.len(), 5, "{}", reply.body);
    let row = |drive: &Path| {
        let quoted = super::json::quote(&drive.display().to_string());
        *rows
            .iter()
            .find(|row| row.starts_with(&quoted))
            .unwrap_or_else(|| panic!("no row for {}: {}", drive.display(), reply.body))
    };
    assert!(
        row(&roomy).contains("\"fits\":true,\"why\":null"),
        "a key with room was refused: {}",
        row(&roomy)
    );
    let small_row = row(&small);
    assert!(
        small_row.contains("\"fits\":false,\"why\":\"room\""),
        "a 1 MiB key was offered for a copy of the programs alone: {small_row}"
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
