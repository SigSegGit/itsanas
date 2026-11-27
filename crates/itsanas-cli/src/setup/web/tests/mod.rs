//! The page's server, spoken to over real sockets by a plain `TcpStream`.
//!
//! Each test starts the server in this process on a throwaway home, with a
//! scripted person in place of the native windows and no service manager.
//! What is proved is the door, not the furniture: who is refused (another
//! Host, no token, another origin, an oversized request), what every answer
//! carries, and that nothing that comes back ever holds a secret -- a whole
//! setup driven through the API, every byte read.
//!
//! This module holds the scripted person, the stand-in service and the page
//! every test talks to; each concern's tests are a module of their own.

mod account_pages;
mod desktop_programs;
mod door;
mod languages;
mod layout;
mod names;
mod native_windows;
mod shell_features;
mod trash_and_versions;
mod usb_copy;

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
    APP_CSS, APP_JS, Backends, CONNECTION_DEADLINE, Context, Features, Held, Host, I18N_JS,
    INDEX_HTML, LEGAL_NOTICE, LEGAL_PRIVACY, LEGAL_TERMS, MAX_CONNECTIONS, Mode, NoHost, Phase,
    READ_TIMEOUT, Run, Running, Server, Slot,
    backends::StartCopy,
    desktop::{browser_command, gtk_bookmark, has_desktop, picker_command},
    http, json,
    preferences::{LANGUAGE_FILE, LANGUAGES, THEME_FILE, language_of},
    state::run_json,
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
