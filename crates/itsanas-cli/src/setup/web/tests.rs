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
    Backends, Context, Features, LANGUAGE_FILE, Mode, NoHost, Server, browser_command, has_desktop,
    http, language_of,
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

/// The node behind the account calls, opened as a computer does: the passphrase
/// from the scripted person, no service to stop.
fn test_nodes(seen: &Seen, home: &Path) -> Arc<dyn super::Nodes> {
    let seen = seen.clone();
    let file = home.with_extension("passphrase");
    Arc::new(super::nodes::Opened::new(
        home.to_owned(),
        Box::new(move || {
            let service: Box<dyn ServiceControl> = Box::new(NoService { file: file.clone() });
            service
        }),
        Box::new(move || {
            let person: Box<dyn SecretPrompt> = Box::new(Person { seen: seen.clone() });
            Ok(person)
        }),
    ))
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
        owner: |_, _| Ok(None),
        pledge: |_, name| (name == "taken").then_some(40 << 30),
        another: |_| Ok(()),
        drives: Box::new(Vec::new),
        programs: || Some(1024),
        copy: Box::new(|_, _| Err("no copy is started in these tests".to_owned())),
        features: Features::DESKTOP,
        phone: false,
        fixed_coordinator: None,
        host: Arc::new(NoHost),
        nodes: test_nodes(&Seen::default(), home),
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
    drop(page);
    // Settings, on the account just made: the words shown again and the
    // passphrase changed. Both are asked and shown in the "window"; what comes
    // back to the page must be a sentence.
    let settings = Page::start(
        home,
        folder.parent().expect("parent"),
        Mode::Settings,
        &seen,
    );
    for route in ["/api/words", "/api/passphrase"] {
        let started = settings.api("POST", route, "");
        assert_eq!(
            started.status, 202,
            "{route} was not started: {}",
            started.body
        );
        heard.push_str(&started.all());
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let state = settings.api("GET", "/api/state", "");
            heard.push_str(&state.all());
            if !state.body.contains("\"phase\":\"running\"") {
                assert!(
                    state.body.contains("\"phase\":\"done\""),
                    "{route} did not finish: {}",
                    state.body
                );
                break;
            }
            assert!(Instant::now() < deadline, "{route} never ended");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    {
        let shown = seen.0.lock().expect("seen");
        assert_eq!(
            shown.get(1),
            shown.first(),
            "Show my 24 words did not show this account's words in the window"
        );
    }
    drop(settings);
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

/// What `/api/name` says of a name is shown in the page's language.
#[test]
fn every_reason_a_username_is_refused_has_a_french_translation() {
    let french = french_keys();
    for bad in ["", "marie_1", "-nope"] {
        let why = crate::coordinator::username_refusal(bad).expect("a refused name");
        assert!(
            french.contains(&why),
            "the page shows this in English whatever the language chosen: {why}"
        );
    }
}

/// The question has no answer until the person gives one, and the one they are
/// likelier to need comes first: a preselected "new account" made a second
/// account for a person who meant to join their own (Nicolas, 2026-10-14).
/// Sabotage: `checked` on either radio; "new" first; `accountKind` falling back
/// on 'new' (Next goes on with an answer nobody gave).
#[test]
fn red_team_the_page_answers_nothing_for_the_person_and_offers_the_existing_account_first() {
    let html = include_str!("index.html");
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
        slot.starts_with("id=\"st-usb\" data-feature=\"usb_key\" aria-labelledby=\"st-h-usb\">"),
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
    let other = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let desktop = other.api("POST", "/api/host", "name=biometric&value=on");
    assert_eq!(
        desktop.status, 403,
        "a computer was told to set a fingerprint it does not have: {}",
        desktop.body
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
    let disk = itsanas_node::pledges::disk_space(dir.path());
    let (total, free) = (disk.total.unwrap_or(0), disk.free.unwrap_or(0));
    for other in [0_u64, 1 << 30, 10 << 40] {
        let reply = page
            .api("POST", "/api/suggest", &format!("other={other}"))
            .body;
        let expected = itsanas_node::pledges::suggest(
            &crate::config::Config::default(),
            total,
            free,
            0,
            other,
            other,
        );
        assert!(
            reply.contains(&format!("\"pledge_gb\":{}", expected.pledge >> 30))
                && reply.contains(&format!("\"keep_gb\":{}", expected.keep >> 30))
                && reply.contains(&format!(
                    "\"same_as_other_machines\":{}",
                    expected.same_as_other_machines
                )),
            "the page was suggested something the rule does not (other = {other}): {reply}"
        );
    }
    let state = page.api("GET", "/api/state", "").body;
    let none =
        itsanas_node::pledges::suggest(&crate::config::Config::default(), total, free, 0, 0, 0);
    assert!(
        state.contains(&format!("\"pledge_gb\":{}", none.pledge >> 30)),
        "the first suggestion is not the rule's: {state}"
    );
}

/// Records what the page told the shell.
#[derive(Default)]
struct Told(Mutex<Vec<(String, String)>>);

impl super::Host for Told {
    fn event(&self, name: &str, value: &str) {
        self.0
            .lock()
            .expect("lock")
            .push((name.to_owned(), value.to_owned()));
    }
}

/// The theme is in the first byte of the page, kept in the node's home for the
/// next page and the phone's own screens, and told to the shell: a theme set
/// only by a script would flash the other one first, and be lost at the next page.
/// Sabotage: drop the `data-theme` insertion in `index_html`; drop `save_theme`;
/// drop the `host.event`.
#[test]
fn red_team_a_chosen_theme_is_in_the_first_byte_and_survives_the_next_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let told = Arc::new(Told::default());
    let page = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.host = told.clone();
    });
    let first = |page: &Page| {
        let host = page.host();
        page.send("GET", "/", &[("Host", &host)], "").body
    };
    assert!(
        !first(&page).contains("data-theme"),
        "a theme nobody chose is in the page"
    );
    let refused = page.api("POST", "/api/theme", "theme=purple");
    assert_eq!(refused.status, 400, "{}", refused.body);

    let dark = page.api("POST", "/api/theme", "theme=dark");
    assert_eq!(dark.status, 200, "{}", dark.body);
    assert!(
        first(&page).contains("<html data-theme=\"dark\" "),
        "the theme is not in the first byte of the page: the other one shows first"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(super::THEME_FILE)).expect("kept"),
        "dark\n",
        "the theme was not kept beside the node: the next page forgets it"
    );
    assert_eq!(
        *told.0.lock().expect("lock"),
        [("theme".to_owned(), "dark".to_owned())],
        "the shell was not told: a phone's own screens stay in the other theme"
    );
    drop(page);
    let next = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    assert!(
        first(&next).contains("<html data-theme=\"dark\" "),
        "the next page started from the system's theme"
    );
    next.api("POST", "/api/theme", "theme=system");
    assert!(!first(&next).contains("data-theme"));

    let css = super::APP_CSS;
    assert!(
        css.contains(":root:not([data-theme=\"light\"])")
            && css.contains(":root[data-theme=\"dark\"]"),
        "the dark colours do not follow the system unless light is chosen, or apply when dark is"
    );
}

/// Nothing leaves the page but the three links of the legal block, each safe to
/// follow and each to a place that exists: no placeholder, no "coming soon".
/// Sabotage: drop a `rel`; add a link elsewhere.
#[test]
fn red_team_the_page_links_only_the_licence_the_source_and_the_security_policy() {
    let html = super::INDEX_HTML;
    let links: Vec<&str> = html.split("<a ").skip(1).collect();
    let external: Vec<&str> = links
        .iter()
        .copied()
        .filter(|link| link.contains("href=\"http"))
        .collect();
    assert_eq!(
        external.len(),
        3,
        "the legal block is not the licence, the source and the policy"
    );
    for link in external {
        let link = &link[..link.find('>').expect("a tag")];
        assert!(
            link.contains("rel=\"noopener noreferrer\"") && link.contains("target=\"_blank\""),
            "a link out of the page can read it back or is followed in the same tab: {link}"
        );
        assert!(
            link.contains("href=\"https://github.com/SigSegGit/itsanas"),
            "a link out of the page goes somewhere that is not the project: {link}"
        );
    }
    for placeholder in ["coming soon", "TODO", "lorem"] {
        assert!(
            !html.contains(placeholder),
            "the page carries a placeholder ({placeholder}) where a text should be"
        );
    }
}

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
            "this account has 10 live devices and the limit is 10".to_owned(),
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

#[test]
fn a_computer_is_offered_no_way_to_remove_or_delete_the_account_from_the_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    for (method, path) in [
        ("GET", "/api/remove"),
        ("POST", "/api/remove"),
        ("POST", "/api/delete"),
    ] {
        assert_eq!(
            page.api(method, path, "confirm=x").status,
            403,
            "{path} answers on a computer, where the service would be left behind"
        );
    }
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

/// The legal texts are in the program, reachable from the first screen and from
/// Settings, with no network. Sabotage: drop a route; drop a block from a screen.
#[test]
fn the_legal_texts_are_served_and_linked_from_both_screens() {
    for text in [
        super::LEGAL_NOTICE,
        super::LEGAL_PRIVACY,
        super::LEGAL_TERMS,
    ] {
        assert!(text.len() > 200, "a legal page is empty");
    }
    assert!(super::LEGAL_NOTICE.contains("Nicolas GIRARD"));
    let html = include_str!("index.html");
    assert_eq!(
        html.matches("data-legal=\"/legal/privacy\"").count(),
        2,
        "the privacy policy must be one tap away on the first screen and in Settings"
    );
}
