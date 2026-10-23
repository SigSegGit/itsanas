//! `itsanas setup` and `itsanas settings` as a page in the person's browser.
//!
//! HANDOVER §8 0w (4). The page is served by this binary on 127.0.0.1, on a
//! random port, for as long as the command runs: one interface for Windows,
//! macOS and Linux desktops, no GUI crate, and every request testable by a
//! plain `TcpStream` in CI, which has no display.
//!
//! # What the browser never sees
//!
//! The 24 words and the passphrase. Every extension allowed to "read all
//! sites" reads this page too, so the engine asks for secrets in the native
//! windows of [`super::secrets`], and the page only says that a window opened
//! and polls until it is done. Nothing in this module receives, holds or
//! sends a secret: it hands the engine [`Answers`] (none of which is one) and
//! relays its [`Event`]s, which carry none (the engine's own red-team test).
//! `red_team_no_response_ever_carries_a_recovery_word_or_the_passphrase`
//! drives a whole setup through the API and reads every byte that came back.
//!
//! # Who may talk to it
//!
//! Any local process, and any web page the person has open, can reach a port
//! on 127.0.0.1. So: a `Host` other than `127.0.0.1:<port>` or
//! `localhost:<port>` is refused (a page that rebinds its own name to
//! 127.0.0.1 would otherwise be same-origin with this one); every `/api/`
//! request carries a 128-bit token, compared in constant time, that only the
//! URL this command printed holds -- in the fragment, which a browser never
//! sends in a request line, so it reaches no log and no Referer; an `Origin`
//! that is there and not this server is refused (CSRF), and no CORS header is
//! ever sent. The token also passes through the argv of the program that
//! opens the browser for a moment; a process of the same user that can read
//! that can read the service's passphrase file too, so it opens nothing new.
//!
//! # Who serves Settings
//!
//! This same process, started by `itsanas settings`, not the daemon: the
//! daemon is a long-lived process and a listener in it is more to defend for
//! every hour it runs. A change the daemon reads only at start (space
//! offered, folder, coordinator) goes through the setup engine, which stops
//! the service, applies it with the functions the CLI uses, and starts it
//! again; pause, resume, sync now and the interval go through the control
//! file (`control.rs`) as the tray's do.

mod http;
mod json;

use std::{
    fmt::Write as _,
    io::Read as _,
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use self::{
    http::{Request, Response},
    json::{array, field, object, optional, quote},
};
use super::{
    Account, Answers, Event, SecretPrompt, ServiceControl, Setup, Step, answers, secrets, service,
    sign, verify,
};
use crate::{
    config::{Config, format_size, parse_size},
    error::{CliError, Result},
    node::Node,
};

const INDEX_HTML: &str = include_str!("index.html");
const APP_CSS: &str = include_str!("app.css");
const APP_JS: &str = include_str!("app.js");
/// The page's other languages, keyed by its English text.
const I18N_JS: &str = include_str!("i18n.js");
/// The project's icon (docs/assets), in the header and as the favicon.
const ICON_PNG: &[u8] = include_bytes!("../../../../../docs/assets/icon.png");

/// The languages the page speaks: English, written in the page, and those of
/// `i18n.js`.
const LANGUAGES: [&str; 2] = ["en", "fr"];
/// In a node's home, the language chosen in the page's list. Each page is a
/// new port, so a new origin with empty browser storage: a choice kept there
/// was gone at the next `itsanas settings` (hand test, 2026-10-22: French
/// chosen in setup, Settings in English). Kept beside the node, the next page
/// and the tray read the same word.
pub(crate) const LANGUAGE_FILE: &str = "language";

/// The language chosen in a page of this node, when one was.
pub(crate) fn saved_language(home: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(home.join(LANGUAGE_FILE)).ok()?;
    let word = text.trim();
    LANGUAGES.into_iter().find(|language| *language == word)
}

/// Keep `language` for the next page, once the node's home exists: before the
/// first step of a setup it does not, and the choice waits in memory until the
/// run ends.
fn save_language(home: &Path, language: &str) -> bool {
    home.is_dir() && std::fs::write(home.join(LANGUAGE_FILE), format!("{language}\n")).is_ok()
}

/// The language of the page a browser asks for in `Accept-Language`: the one
/// it prefers most (by `q`, then by order) among those the page speaks, by
/// primary tag, so `fr-CA` is French. `None` when it asks for none of them.
pub(crate) fn language_of(accept: &str) -> Option<&'static str> {
    let mut best: Option<(u16, &'static str)> = None;
    for item in accept.split(',') {
        let mut parts = item.split(';');
        let tag = parts.next().unwrap_or("").trim();
        let primary = tag.split('-').next().unwrap_or("").to_ascii_lowercase();
        let quality = parts
            .find_map(|param| param.trim().strip_prefix("q="))
            .map_or(Some(1000), quality_of);
        let (Some(quality), Some(language)) = (
            quality,
            LANGUAGES.into_iter().find(|language| *language == primary),
        ) else {
            continue;
        };
        if quality > 0 && best.is_none_or(|(had, _)| quality > had) {
            best = Some((quality, language));
        }
    }
    best.map(|(_, language)| language)
}

/// A `q` value in thousandths, without a float: `1`, `0.8`, `0.125`.
fn quality_of(text: &str) -> Option<u16> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let thousandths = format!("{fraction:0<3}").parse::<u16>().ok()?;
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(1000),
        _ => None,
    }
}

/// What the page is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// `itsanas setup`: the steps, one at a time.
    Setup,
    /// `itsanas settings`: a node that is set up, changed.
    Settings,
}

/// With no click, typed field or button for this long, the server stops: a
/// page forgotten in a tab is not a door left open for days. Polling the
/// state does not count -- a forgotten tab polls too.
pub(crate) const IDLE: Duration = Duration::from_secs(30 * 60);
/// After a setup that finished, how long the page stays answerable (a
/// reload, the final report read again) before the command ends by itself.
const FINISHED_GRACE: Duration = Duration::from_secs(2 * 60);
/// A slow or silent client is dropped after this, so it cannot hold a thread.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// The whole life of one connection, at most. `READ_TIMEOUT` alone is per
/// read: a local process dripping one byte every few seconds would hold a
/// thread for ever, and sixteen of them would leave the person's page
/// unanswered until the idle exit.
const CONNECTION_DEADLINE: Duration = Duration::from_secs(10);
/// Connections served at once; one person's browser opens a handful.
const MAX_CONNECTIONS: usize = 16;
/// Lines of progress kept for the page; a run says about twenty.
const MAX_LINES: usize = 400;

type MakePrompt = Box<dyn Fn() -> Result<Box<dyn SecretPrompt>> + Send + Sync>;
type MakeService = Box<dyn Fn() -> Box<dyn ServiceControl> + Send + Sync>;

/// Where the engine asks for secrets and which service it drives, made fresh
/// for each run (the engine runs on a thread of its own, and neither needs to
/// cross threads that way). Tests pass stand-ins.
pub(crate) struct Backends {
    pub(crate) prompt: MakePrompt,
    pub(crate) service: MakeService,
    /// Is a username taken at a coordinator ([`super::name_taken`]).
    pub(crate) name: super::NameCheck,
    /// Start the setup of another account, as instance `name`, in a page of
    /// its own ([`start_another`]).
    pub(crate) another: fn(&str) -> std::result::Result<(), String>,
    /// The USB keys and external disks plugged in ([`crate::drives::candidates`]).
    pub(crate) drives: ListDrives,
    /// What this release's programs take on a key, asked once, in the
    /// background: it is a download of the signed manifest.
    pub(crate) programs: fn() -> Option<u64>,
    /// Start `copy-to-key DIR --detach` for the home; what tells whether it
    /// still runs ([`crate::copy_to_key::start_detached`]).
    pub(crate) copy: StartCopy,
}

type ListDrives = Box<dyn Fn() -> Vec<crate::drives::Candidate> + Send + Sync>;
/// Whether a started copy's process still runs; asked under a lock.
pub(crate) type Running = Box<dyn FnMut() -> bool + Send>;
type StartCopy = Box<dyn Fn(&Path, &Path) -> std::result::Result<Running, String> + Send + Sync>;

/// A copy whose progress file has not changed for this long is a copy that
/// died without a word: the page offers to start another.
const COPY_STALE: Duration = Duration::from_secs(30 * 60);

impl Backends {
    fn of_this_machine(home: &Path, instance: Option<&str>) -> Self {
        let home = home.to_owned();
        let instance = instance.map(str::to_owned);
        Self {
            prompt: Box::new(|| {
                let prompt: Box<dyn SecretPrompt> =
                    Box::new(secrets::Native::new(secrets::choose()?));
                Ok(prompt)
            }),
            service: Box::new(move || {
                let platform: Box<dyn ServiceControl> = Box::new(
                    service::Platform::of_this_machine(&home, instance.as_deref()),
                );
                platform
            }),
            name: super::name_taken,
            another: start_another,
            drives: Box::new(crate::drives::candidates),
            programs: || {
                let curl = crate::update::Curl;
                crate::update::Updater::of_this_build(&curl)
                    .and_then(|updater| updater.release_bytes())
                    .ok()
            },
            copy: Box::new(|home, dir| {
                let mut child =
                    crate::copy_to_key::start_detached(home, dir).map_err(|e| e.to_string())?;
                let running: Running = Box::new(move || matches!(child.try_wait(), Ok(None)));
                Ok(running)
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
}

impl Phase {
    const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// One run of the engine, as the page sees it.
#[derive(Debug, Default)]
struct Run {
    phase: Phase,
    step: Option<Step>,
    /// What a native window is asking for now, while it is open.
    waiting: Option<&'static str>,
    lines: Vec<String>,
    /// The step that failed, what went wrong, and the one thing to do.
    failed: Option<(Step, String, String)>,
    report: Vec<verify::Finding>,
    finished_at: Option<Instant>,
}

/// Everything a request handler may read, shared by every connection thread.
struct Context {
    home: PathBuf,
    instance: Option<String>,
    mode: Mode,
    /// The person's home directory: `~` in a folder, and the default folder.
    base: PathBuf,
    token: String,
    port: u16,
    backends: Backends,
    run: Mutex<Run>,
    last_action: Mutex<Instant>,
    quit: AtomicBool,
    live: AtomicUsize,
    /// The language chosen in the page's list: read from the node's home,
    /// set by POST /api/language.
    language: Mutex<Option<&'static str>>,
    /// What the programs take on a key, once the background ask answered
    /// ([`Backends::programs`]); `None` before, or when it could not say.
    programs: Mutex<Option<u64>>,
    programs_asked: AtomicBool,
    /// The copy to a key this page started, while its first process runs.
    copying: Mutex<Option<Running>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A handler that panicked leaves the data as it was; the page can still
    // be told what it holds.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The page's server: bound, not yet serving.
pub(crate) struct Server {
    listener: TcpListener,
    ctx: Arc<Context>,
}

impl Server {
    /// Bind 127.0.0.1 on a port the system chooses, with a fresh token.
    pub(crate) fn bind(
        home: &Path,
        instance: Option<&str>,
        mode: Mode,
        base: &Path,
        backends: Backends,
    ) -> Result<Self> {
        let failed = |source| CliError::Io {
            path: PathBuf::from("127.0.0.1"),
            source,
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(failed)?;
        let port = listener.local_addr().map_err(failed)?.port();
        let mut secret = [0_u8; 16];
        getrandom::fill(&mut secret).map_err(|error| {
            CliError::Usage(format!("no randomness for the page's key: {error}"))
        })?;
        let token = secret.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        Ok(Self {
            listener,
            ctx: Arc::new(Context {
                home: home.to_owned(),
                instance: instance.map(str::to_owned),
                mode,
                base: base.to_owned(),
                token,
                port,
                backends,
                run: Mutex::new(Run::default()),
                last_action: Mutex::new(Instant::now()),
                quit: AtomicBool::new(false),
                live: AtomicUsize::new(0),
                language: Mutex::new(saved_language(home)),
                programs: Mutex::new(None),
                programs_asked: AtomicBool::new(false),
                copying: Mutex::new(None),
            }),
        })
    }

    /// The address to open, the token in its fragment.
    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}/#t={}", self.ctx.port, self.ctx.token)
    }

    /// Answer requests until the page says it is done, the setup finished,
    /// or nobody did anything for `idle`. True when a setup run finished well.
    pub(crate) fn serve(&self, idle: Duration) -> bool {
        if self.listener.set_nonblocking(true).is_err() {
            return false;
        }
        while !self.ctx.should_stop(idle) {
            match self.listener.accept() {
                Ok((stream, _)) => self.dispatch(stream),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        lock(&self.ctx.run).phase == Phase::Done
    }

    fn dispatch(&self, stream: TcpStream) {
        // Beyond the cap a connection is closed unanswered: a flood from a
        // local process gets nothing, the person's page retries its poll.
        if self.ctx.live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            self.ctx.live.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        let ctx = Arc::clone(&self.ctx);
        let spawned = std::thread::Builder::new()
            .name("setup-page".to_owned())
            .spawn(move || {
                handle(&ctx, stream);
                ctx.live.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.ctx.live.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Context {
    fn should_stop(&self, idle: Duration) -> bool {
        if self.quit.load(Ordering::SeqCst) {
            return true;
        }
        let run = lock(&self.run);
        match run.phase {
            Phase::Running => false,
            Phase::Done
                if self.mode == Mode::Setup
                    && run
                        .finished_at
                        .is_some_and(|at| at.elapsed() >= FINISHED_GRACE) =>
            {
                true
            }
            _ => lock(&self.last_action).elapsed() >= idle,
        }
    }

    /// One event of the engine, kept for the page.
    fn record(&self, event: &Event) {
        let mut run = lock(&self.run);
        match event {
            Event::Started(step) => {
                run.step = Some(*step);
                run.waiting = None;
            }
            Event::NeedsSecret { what, .. } => run.waiting = Some(what),
            Event::Failed {
                step,
                error,
                remedy,
            } => {
                run.failed = Some((*step, error.clone(), remedy.clone()));
                run.waiting = None;
            }
            _ => run.waiting = None,
        }
        if run.lines.len() < MAX_LINES {
            run.lines.push(event.line().trim().to_owned());
        }
    }

    fn finish(&self, outcome: std::result::Result<super::Outcome, String>) {
        let mut run = lock(&self.run);
        match outcome {
            Ok(outcome) => {
                run.report = outcome
                    .report
                    .map(|report| report.findings)
                    .unwrap_or_default();
                run.phase = if outcome.failed.is_none() {
                    Phase::Done
                } else {
                    Phase::Failed
                };
            }
            Err(why) => {
                let step = run.step.unwrap_or(Step::Machine);
                run.failed = Some((
                    step,
                    why,
                    "close this page and run `itsanas setup --text` in a terminal".to_owned(),
                ));
                run.phase = Phase::Failed;
            }
        }
        run.waiting = None;
        run.finished_at = Some(Instant::now());
    }
}

/// Reads from a stream, refusing to go past an absolute instant: each read
/// waits at most what is left, so dripping bytes cannot extend the life of a
/// connection the way they extend a per-read timeout.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl std::io::Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left.min(READ_TIMEOUT)))?;
        std::io::Read::read(&mut &*self.stream, buf)
    }
}

/// Read one request, answer it, close.
fn handle(ctx: &Arc<Context>, mut stream: TcpStream) {
    let until = Instant::now() + CONNECTION_DEADLINE;
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let read = http::read_request(&mut Deadline {
        stream: &stream,
        until,
    });
    let response = match read {
        Ok(request) => route(ctx, &request),
        Err(status) => Response::refuse(
            status,
            "refused: the request was too large, too slow or malformed",
        ),
    };
    let _ = http::write_response(&mut stream, &response);
    let _ = stream.shutdown(Shutdown::Write);
    // Read a little of what is left before closing, so the close is not a
    // reset that destroys the answer before the client reads it. Bounded in
    // bytes and time: an oversized body is still never read whole.
    let mut drain = Deadline {
        stream: &stream,
        until: until.min(Instant::now() + Duration::from_millis(200)),
    };
    let _ = std::io::copy(
        &mut (&mut drain).take(http::MAX_BODY as u64),
        &mut std::io::sink(),
    );
}

fn route(ctx: &Arc<Context>, request: &Request) -> Response {
    if !http::host_allowed(request, ctx.port) {
        return Response::refuse(
            403,
            &format!(
                "this page answers only at http://127.0.0.1:{}/ on this machine",
                ctx.port
            ),
        );
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "/index.html") => {
            Response::new(200, "text/html; charset=utf-8", index_html(ctx, request))
        }
        ("GET", "/app.css") => Response::new(200, "text/css; charset=utf-8", APP_CSS),
        ("GET", "/app.js") => Response::new(200, "text/javascript; charset=utf-8", APP_JS),
        ("GET", "/i18n.js") => Response::new(200, "text/javascript; charset=utf-8", I18N_JS),
        ("GET", "/icon.png") => Response::new(200, "image/png", ICON_PNG),
        (_, path) if path.starts_with("/api/") => api(ctx, request),
        ("GET", _) => Response::refuse(404, "no such page"),
        _ => Response::refuse(405, "not here"),
    }
}

/// The page, in the language chosen before, else the browser's: said in
/// `<html lang>` from the first byte, so nothing shows in English first and
/// the page needs no request to know it.
fn index_html(ctx: &Context, request: &Request) -> String {
    let chosen = *lock(&ctx.language);
    match chosen.or_else(|| request.header("accept-language").and_then(language_of)) {
        Some(language) => INDEX_HTML.replacen(
            "<html lang=\"en\">",
            &format!("<html lang=\"{language}\" data-lang=\"{language}\">"),
            1,
        ),
        None => INDEX_HTML.to_owned(),
    }
}

/// The language chosen in the page's list, kept for the next page.
fn choose_language(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let Some(language) = field(&form, "lang")
        .and_then(|asked| LANGUAGES.into_iter().find(|language| *language == asked))
    else {
        return Response::refuse(400, "this page speaks English (en) and French (fr)");
    };
    *lock(&ctx.language) = Some(language);
    let kept = save_language(&ctx.home, language);
    ok_json(object(&[
        ("language", quote(language)),
        ("kept", kept.to_string()),
    ]))
}

fn api(ctx: &Arc<Context>, request: &Request) -> Response {
    let token_ok = request.count("x-itsanas-token") == 1
        && request
            .header("x-itsanas-token")
            .is_some_and(|given| http::same_secret(given.as_bytes(), ctx.token.as_bytes()));
    if !token_ok {
        return Response::refuse(
            403,
            "this page's key is missing or wrong: open the address the command printed",
        );
    }
    if !http::origin_allowed(request, ctx.port) {
        return Response::refuse(403, "refused: this request came from another site");
    }
    let method = request.method.as_str();
    let path = request.path.as_str();
    // Polls are not actions: a forgotten tab polls too.
    if !matches!((method, path), ("GET", "/api/state" | "/api/copy-to-key")) {
        *lock(&ctx.last_action) = Instant::now();
    }
    match (method, path) {
        ("GET", "/api/state") => ok_json(state_json(ctx)),
        ("GET", "/api/keys") if ctx.mode == Mode::Settings => keys(ctx),
        ("POST", "/api/keys") if ctx.mode == Mode::Settings => key_check(ctx, request),
        ("GET", "/api/copy-to-key") if ctx.mode == Mode::Settings => copy_state(ctx),
        ("POST", "/api/copy-to-key") if ctx.mode == Mode::Settings => start_copy(ctx, request),
        ("GET", "/api/plan") => ok_json(plan_json(ctx)),
        ("POST", "/api/run") => start_run(ctx, request),
        ("POST", "/api/control") if ctx.mode == Mode::Settings => steer(ctx, request),
        ("POST", "/api/signout") if ctx.mode == Mode::Settings => sign_out(ctx),
        ("POST", "/api/disk") if ctx.mode == Mode::Settings => disk_job(ctx, request),
        ("POST", "/api/name") if ctx.mode == Mode::Setup => check_name(ctx, request),
        ("POST", "/api/another") if ctx.mode == Mode::Setup => another_account(ctx, request),
        ("POST", "/api/language") => choose_language(ctx, request),
        ("POST", "/api/free") => free_space(ctx, request),
        ("POST", "/api/pick") => pick(request),
        ("POST", "/api/open") => open_folder(ctx, request, false),
        ("POST", "/api/pin") => open_folder(ctx, request, true),
        ("POST", "/api/quit") => {
            ctx.quit.store(true, Ordering::SeqCst);
            ok_json(object(&[("said", quote("closed"))]))
        }
        _ => Response::refuse(404, "no such action"),
    }
}

fn ok_json(body: String) -> Response {
    Response::new(200, "application/json", body)
}

const fn step_id(step: Step) -> &'static str {
    match step {
        Step::Machine => "machine",
        Step::Account => "account",
        Step::Secret => "secret",
        Step::Registration => "registration",
        Step::Pledge => "pledge",
        Step::Folder => "folder",
        Step::Updates => "updates",
        Step::Connectivity => "connectivity",
        Step::Service => "service",
        Step::Verify => "verify",
    }
}

/// Space to suggest offering: a fifth of what is free, in whole GiB, at most
/// 500 GiB. A suggestion the person can lower; integers, never a float.
pub(crate) fn suggest_pledge(free: u64) -> u64 {
    const GIB: u64 = 1 << 30;
    (free / 5 / GIB * GIB).min(500 * GIB)
}

fn state_json(ctx: &Context) -> String {
    let free = fs4::available_space(&ctx.base).ok();
    let suggested_gib = free.map_or(0, suggest_pledge) >> 30;
    let defaults = object(&[
        (
            "folder",
            quote(
                &answers::default_folder(&ctx.base, ctx.instance.as_deref())
                    .display()
                    .to_string(),
            ),
        ),
        ("pledge_gb", suggested_gib.to_string()),
        // What this node has, so running setup again never resets a choice.
        (
            "updates",
            quote(
                Config::load(&Node::config_path(&ctx.home))
                    .map(|config| config.updates)
                    .unwrap_or_default()
                    .name(),
            ),
        ),
        ("free", optional(free.map(format_size).as_deref())),
        ("free_bytes", free.unwrap_or(0).to_string()),
        (
            "coordinator",
            quote(
                &Config::load(&Node::config_path(&ctx.home))
                    .ok()
                    .and_then(|config| config.coordinator)
                    .unwrap_or_else(|| super::DEFAULT_COORDINATOR.to_owned()),
            ),
        ),
        // The bargain the space page shows live, from the code's own split.
        (
            "split_own",
            itsanas_coord::accounting::Split::DEFAULT.own.to_string(),
        ),
        (
            "split_network",
            itsanas_coord::accounting::Split::DEFAULT
                .network
                .to_string(),
        ),
        (
            "folder_now",
            optional(
                Config::load(&Node::config_path(&ctx.home))
                    .ok()
                    .and_then(|config| config.folder)
                    .map(|folder| folder.display().to_string())
                    .as_deref(),
            ),
        ),
    ]);
    let mut fields = vec![
        (
            "mode",
            quote(match ctx.mode {
                Mode::Setup => "setup",
                Mode::Settings => "settings",
            }),
        ),
        ("home", quote(&ctx.home.display().to_string())),
        ("instance", optional(ctx.instance.as_deref())),
        ("exists", Node::exists(&ctx.home).to_string()),
        (
            "account_here",
            optional(
                Node::exists(&ctx.home)
                    .then(|| Config::load(&Node::config_path(&ctx.home)).ok())
                    .flatten()
                    .map(|config| config.username)
                    .as_deref(),
            ),
        ),
        ("defaults", defaults),
        ("run", run_json(&lock(&ctx.run))),
    ];
    if ctx.mode == Mode::Settings {
        fields.push(("settings", settings_json(&ctx.home)));
    }
    object(&fields)
}

fn run_json(run: &Run) -> String {
    let failed = run.failed.as_ref().map_or_else(
        || "null".to_owned(),
        |(step, error, remedy)| {
            object(&[
                ("step", quote(step_id(*step))),
                ("title", quote(step.title())),
                ("error", quote(error)),
                ("remedy", quote(remedy)),
            ])
        },
    );
    let report = array(run.report.iter().map(|finding| {
        object(&[
            ("what", quote(finding.what)),
            (
                "verdict",
                quote(match finding.verdict {
                    verify::Verdict::Passed => "passed",
                    verify::Verdict::Failed => "failed",
                    verify::Verdict::Skipped => "skipped",
                }),
            ),
            ("detail", quote(&finding.detail)),
            ("remedy", quote(&finding.remedy)),
        ])
    }));
    object(&[
        ("phase", quote(run.phase.name())),
        ("step", optional(run.step.map(step_id))),
        ("waiting", optional(run.waiting)),
        ("lines", array(run.lines.iter().map(|line| quote(line)))),
        ("failed", failed),
        ("report", report),
    ])
}

/// What Settings shows: the tray's one word, and what can be changed.
fn settings_json(home: &Path) -> String {
    let running = Node::exists(home) && itsanas_store::Store::is_locked(Node::store_path(home));
    let brief = crate::brief_status(home, running, itsanas_discover::now_unix());
    let control = crate::control::Control::read(home).ok();
    let config = Config::load(&Node::config_path(home)).ok();
    let interval = control
        .as_ref()
        .and_then(|control| control.interval)
        .map_or_else(|| "auto".to_owned(), crate::control::describe_every);
    object(&[
        (
            "status",
            quote(brief.split_whitespace().next().unwrap_or("unknown")),
        ),
        ("running", running.to_string()),
        (
            "paused",
            control
                .as_ref()
                .is_some_and(|control| control.paused_since.is_some())
                .to_string(),
        ),
        ("interval", quote(&interval)),
        (
            "pledge",
            quote(&format_size(config.as_ref().map_or(0, |c| c.pledge_bytes))),
        ),
        (
            "folder",
            optional(
                config
                    .as_ref()
                    .and_then(|c| c.folder.as_ref())
                    .map(|folder| folder.display().to_string())
                    .as_deref(),
            ),
        ),
        (
            "coordinator",
            optional(config.as_ref().and_then(|c| c.coordinator.as_deref())),
        ),
        (
            "updates",
            quote(
                config
                    .as_ref()
                    .map_or_else(Default::default, |c| c.updates)
                    .name(),
            ),
        ),
        (
            "update_notice",
            optional(crate::update::notice(home).as_deref()),
        ),
        (
            "vault",
            optional(
                config
                    .as_ref()
                    .and_then(|c| c.vault.as_ref())
                    .map(|vault| vault.display().to_string())
                    .as_deref(),
            ),
        ),
        (
            "job",
            crate::jobs::result(home).map_or_else(
                || "null".to_owned(),
                |report| {
                    object(&[
                        ("state", quote(report.state.name())),
                        ("said", quote(&report.said)),
                    ])
                },
            ),
        ),
    ])
}

/// Which steps are done, from the engine's own checks. Asked when the page
/// loads and after a run, not on every poll: a check may open the keys.
fn plan_json(ctx: &Context) -> String {
    let found = Answers {
        instance: ctx.instance.clone(),
        ..Answers::default()
    };
    let service = (ctx.backends.service)();
    array(
        super::plan(&ctx.home, &found, service.as_ref())
            .into_iter()
            .map(|(step, said)| {
                object(&[
                    ("step", quote(step_id(step))),
                    ("title", quote(step.title())),
                    ("done", said.starts_with("done").to_string()),
                    ("said", quote(&said)),
                ])
            }),
    )
}

/// The answers a form holds. In Settings, only what the person changed is
/// sent, and the account is never touched.
fn answers_from_form(
    form: &[(String, String)],
    mode: Mode,
    instance: Option<&str>,
    base: &Path,
    service_installed: bool,
) -> std::result::Result<Answers, String> {
    let mut answers = Answers {
        instance: instance.map(str::to_owned),
        ..Answers::default()
    };
    if mode == Mode::Setup {
        let username = field(form, "username");
        answers.account = match (field(form, "account"), username) {
            (None, _) => None,
            (Some("new"), Some(name)) => Some(Account::New {
                username: name.to_owned(),
            }),
            (Some("join"), Some(name)) => Some(Account::Join {
                username: name.to_owned(),
                from: field(form, "recover_from").map(str::to_owned),
            }),
            (Some("new" | "join"), None) => return Err("choose a username".to_owned()),
            (Some(_), _) => return Err("the account is either new or joined".to_owned()),
        };
        answers.service = field(form, "background") != Some("no");
    } else {
        // A node run by a daemon started by hand has no service to restart;
        // the engine then says to stop it, rather than install one unasked.
        answers.service = service_installed;
    }
    answers.tray = answers.service;
    answers.folder = field(form, "folder").map(|folder| answers::expand(folder, base));
    answers.coordinator = field(form, "coordinator").map(str::to_owned);
    answers.invite = field(form, "invite").map(str::to_owned);
    answers.pledge = field(form, "pledge")
        .map(|text| parse_size(text).map_err(|error| error.to_string()))
        .transpose()?;
    answers.updates = field(form, "updates")
        .map(|text| {
            crate::config::Updates::parse(text)
                .ok_or_else(|| "updates is auto, notify or off".to_owned())
        })
        .transpose()?;
    Ok(answers)
}

fn start_run(ctx: &Arc<Context>, request: &Request) -> Response {
    let installed = (ctx.backends.service)().installed();
    let answers = match json::parse_form(&request.body).and_then(|form| {
        answers_from_form(
            &form,
            ctx.mode,
            ctx.instance.as_deref(),
            &ctx.base,
            installed,
        )
    }) {
        Ok(answers) => answers,
        Err(why) => return Response::refuse(400, &why),
    };
    {
        let mut run = lock(&ctx.run);
        if run.phase == Phase::Running {
            return Response::refuse(409, "it is already running; this page follows it");
        }
        *run = Run {
            phase: Phase::Running,
            ..Run::default()
        };
    }
    let worker = Arc::clone(ctx);
    let spawned = std::thread::Builder::new()
        .name("setup-engine".to_owned())
        .spawn(move || run_engine(&worker, answers));
    if let Err(error) = spawned {
        ctx.finish(Err(format!("could not start: {error}")));
    }
    Response::new(
        202,
        "application/json",
        object(&[("started", "true".to_owned())]),
    )
}

fn run_engine(ctx: &Context, answers: Answers) {
    let service = (ctx.backends.service)();
    let outcome = match (ctx.backends.prompt)() {
        Ok(mut prompt) => {
            let mut record = |event: &Event| ctx.record(event);
            Ok(Setup::new(
                &ctx.home,
                answers,
                prompt.as_mut(),
                service.as_ref(),
                &mut record,
            )
            .with_name_check(ctx.backends.name)
            .run())
        }
        Err(error) => Err(error.to_string()),
    };
    // A language chosen before the home existed is kept now that it does.
    if let Some(language) = *lock(&ctx.language) {
        save_language(&ctx.home, language);
    }
    ctx.finish(outcome);
}

/// Pause, resume, sync now, interval: the control file, through the same
/// function the commands use, so the page says the commands' words.
fn steer(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let command = match field(&form, "action") {
        // A pause with an end ("for 1 hour", "for 8 hours") or until resumed;
        // the duration goes through the same parser as `itsanas pause --for`.
        Some("pause") => crate::Command::Pause {
            duration: field(&form, "for")
                .filter(|f| *f != "resume")
                .map(str::to_owned),
        },
        Some("resume") => crate::Command::Resume,
        Some("sync-now") => crate::Command::SyncNow,
        Some("interval") => crate::Command::Interval {
            every: Some(field(&form, "every").unwrap_or("auto").to_owned()),
        },
        _ => return Response::refuse(400, "no such action"),
    };
    match crate::steer_said(&ctx.home, &command) {
        Ok(said) => ok_json(object(&[("said", quote(&said))])),
        Err(error) => Response::refuse(409, &error.to_string()),
    }
}

/// Move the vault, write or read a backup, make a portable copy: the first
/// three are done by the running daemon (`jobs.rs`), which has the keys
/// open, so this page never asks for the passphrase; the page follows the
/// job through `/api/state`.
fn disk_job(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let path = std::path::PathBuf::from(field(&form, "path").unwrap_or_default().trim());
    let kind = match field(&form, "action") {
        Some("move-vault") => crate::jobs::Kind::MoveVault {
            same_disk: field(&form, "same_disk") == Some("true"),
        },
        Some("export") => crate::jobs::Kind::Export,
        Some("restore") => crate::jobs::Kind::Restore,
        Some("portable") => {
            return match crate::portable::make(&path) {
                Ok(said) => ok_json(object(&[("said", quote(&said))])),
                Err(error) => Response::refuse(409, &error.to_string()),
            };
        }
        _ => return Response::refuse(400, "no such action"),
    };
    if !crate::daemon_holds(&ctx.home) {
        return Response::refuse(
            409,
            "the ITSaNAS service is not running on this machine: start it (Start ITSaNAS, or \
             `itsanas daemon`), or do this in a terminal with `itsanas vault --move` or \
             `itsanas backup`",
        );
    }
    match crate::jobs::ask(&ctx.home, kind, &path) {
        Ok(_) => ok_json(object(&[(
            "said",
            quote("asked; the service starts within two seconds"),
        )])),
        Err(why) => Response::refuse(409, &why),
    }
}

fn sign_out(ctx: &Context) -> Response {
    let service = (ctx.backends.service)();
    match sign::sign_out(&ctx.home, service.as_ref()) {
        Ok(done) => ok_json(object(&[(
            "said",
            quote(&format!("{done}.\n{}", sign::SIGNED_OUT)),
        )])),
        Err(error) => Response::refuse(409, &error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Copy this account to a USB key
// ---------------------------------------------------------------------------

/// What the programs take on a key: known once the background ask answered,
/// `None` meanwhile (the room check then counts the guess).
fn programs_size(ctx: &Arc<Context>) -> Option<u64> {
    if !ctx.programs_asked.swap(true, Ordering::SeqCst) {
        let worker = Arc::clone(ctx);
        // Not asked again if no thread can be had: the guess stands.
        let _ = std::thread::Builder::new()
            .name("programs-size".to_owned())
            .spawn(move || {
                let size = (worker.backends.programs)();
                *lock(&worker.programs) = size;
            });
    }
    *lock(&ctx.programs)
}

/// The account's config, or the refusal to send.
fn config_for_copy(ctx: &Context) -> std::result::Result<Config, Response> {
    if !Node::exists(&ctx.home) {
        return Err(Response::refuse(
            409,
            "no account is set up on this machine",
        ));
    }
    Config::load(&Node::config_path(&ctx.home)).map_err(|error| {
        Response::refuse(
            409,
            &format!("this account's settings could not be read: {error}"),
        )
    })
}

fn number(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

/// One drive as the page lists it, with whether the copy fits there.
fn key_json(
    ctx: &Context,
    config: &Config,
    candidate: &crate::drives::Candidate,
    programs: Option<u64>,
) -> String {
    let (fits, warning) = crate::copy_to_key::assess(
        &ctx.home,
        config,
        &candidate.path,
        candidate.free,
        candidate.format.as_ref(),
        programs,
    );
    let refused = fits.err();
    object(&[
        ("path", quote(&candidate.path.display().to_string())),
        ("label", quote(&candidate.label)),
        ("total_bytes", number(candidate.total)),
        ("free_bytes", number(candidate.free)),
        (
            "total",
            optional(candidate.total.map(format_size).as_deref()),
        ),
        ("free", optional(candidate.free.map(format_size).as_deref())),
        (
            "format",
            optional(
                candidate
                    .format
                    .as_ref()
                    .map(crate::drives::format_name)
                    .as_deref(),
            ),
        ),
        ("external", candidate.external.to_string()),
        ("fits", refused.is_none().to_string()),
        ("why", optional(refused.as_ref().map(|r| r.why))),
        (
            "reason",
            optional(refused.as_ref().map(|r| r.said.as_str())),
        ),
        ("warning", optional(warning.as_deref())),
    ])
}

/// The sizes every row is measured against.
fn sizes_json(ctx: &Arc<Context>) -> (Option<u64>, Vec<(&'static str, String)>) {
    let programs = programs_size(ctx);
    let program_bytes = crate::copy_to_key::programs_bytes(programs);
    let account = crate::copy_to_key::content_of(&ctx.home);
    let needed = program_bytes.saturating_add(account.unwrap_or(0));
    (
        programs,
        vec![
            ("account_bytes", number(account)),
            ("account", optional(account.map(format_size).as_deref())),
            ("programs_bytes", program_bytes.to_string()),
            ("programs", quote(&format_size(program_bytes))),
            ("programs_known", programs.is_some().to_string()),
            ("needed_bytes", needed.to_string()),
            ("needed", quote(&format_size(needed))),
        ],
    )
}

/// `GET /api/keys`: the USB keys and external disks plugged in, each with
/// whether the copy fits on it and why not.
fn keys(ctx: &Arc<Context>) -> Response {
    let config = match config_for_copy(ctx) {
        Ok(config) => config,
        Err(refused) => return refused,
    };
    let (programs, mut fields) = sizes_json(ctx);
    let rows = (ctx.backends.drives)()
        .iter()
        .map(|candidate| key_json(ctx, &config, candidate, programs))
        .collect::<Vec<_>>();
    fields.push(("keys", array(rows)));
    fields.push((
        "copying",
        copy_running(ctx, &mut lock(&ctx.copying)).to_string(),
    ));
    ok_json(object(&fields))
}

/// The drive at `path`, asked of the system: "Another folder…".
fn candidate_at(path: &Path) -> crate::drives::Candidate {
    crate::drives::Candidate {
        path: path.to_owned(),
        label: path.display().to_string(),
        total: crate::drives::existing_ancestor(path).and_then(|dir| fs4::total_space(dir).ok()),
        free: crate::drives::free_bytes(path),
        format: crate::drives::format_of(path),
        external: false,
    }
}

/// `POST /api/keys {path}`: the same row for a folder the person chose.
fn key_check(ctx: &Arc<Context>, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let config = match config_for_copy(ctx) {
        Ok(config) => config,
        Err(refused) => return refused,
    };
    let path = PathBuf::from(field(&form, "path").unwrap_or_default());
    let (programs, mut fields) = sizes_json(ctx);
    let candidate = if crate::jobs::check_path(&path).is_ok() {
        candidate_at(&path)
    } else {
        // Not asked of the system: a relative path is nobody's drive.
        crate::drives::Candidate {
            label: path.display().to_string(),
            path,
            total: None,
            free: None,
            format: None,
            external: false,
        }
    };
    fields.push(("key", key_json(ctx, &config, &candidate, programs)));
    ok_json(object(&fields))
}

/// Whether a copy to a key runs: the process this page started, or a
/// progress file that says it is not done and was written lately (a copy
/// started from a terminal counts too).
fn copy_running(ctx: &Context, slot: &mut Option<Running>) -> bool {
    let child = slot.as_mut().is_some_and(|alive| alive());
    if !child {
        *slot = None;
    }
    child
        || crate::copy_to_key::progress(&ctx.home).is_some_and(|(progress, written)| {
            !progress.done && written.elapsed().ok().is_none_or(|age| age < COPY_STALE)
        })
}

/// `GET /api/copy-to-key`: whether it runs, and its last progress.
fn copy_state(ctx: &Context) -> Response {
    let running = copy_running(ctx, &mut lock(&ctx.copying));
    if running {
        // A copy under way keeps the page that follows it open.
        *lock(&ctx.last_action) = Instant::now();
    }
    let progress = crate::copy_to_key::progress(&ctx.home).map_or_else(
        || "null".to_owned(),
        |(progress, _)| {
            object(&[
                ("step", quote(&progress.step)),
                ("done", progress.done.to_string()),
                ("said", quote(&progress.said)),
                ("error", optional(progress.error.as_deref())),
            ])
        },
    );
    ok_json(object(&[
        ("running", running.to_string()),
        ("progress", progress),
    ]))
}

/// `POST /api/copy-to-key {path}`: the target checked as the command checks
/// it, then `itsanas copy-to-key PATH --detach` started, which asks both
/// passphrases in windows of its own. One at a time.
fn start_copy(ctx: &Arc<Context>, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let config = match config_for_copy(ctx) {
        Ok(config) => config,
        Err(refused) => return refused,
    };
    let path = PathBuf::from(field(&form, "path").unwrap_or_default());
    // Every refusal of the command itself (`check_path` first), and the
    // room, before a window opens for a copy that cannot happen.
    let candidate = candidate_at(&path);
    let programs = programs_size(ctx);
    if let (Err(refused), _) = crate::copy_to_key::assess(
        &ctx.home,
        &config,
        &path,
        candidate.free,
        candidate.format.as_ref(),
        programs,
    ) {
        return Response::refuse(400, &refused.said);
    }
    // Held from the check to the start: a second click, or a second tab,
    // finds the first one running.
    let mut slot = lock(&ctx.copying);
    if copy_running(ctx, &mut slot) {
        return Response::refuse(
            409,
            "a copy to a key is already running; this page follows it",
        );
    }
    crate::copy_to_key::forget_progress(&ctx.home);
    match (ctx.backends.copy)(&ctx.home, &path) {
        Ok(running) => {
            *slot = Some(running);
            Response::new(
                202,
                "application/json",
                object(&[("started", "true".to_owned())]),
            )
        }
        Err(why) => Response::refuse(500, &format!("the copy could not start: {why}")),
    }
}

// ---------------------------------------------------------------------------
// Starting it
// ---------------------------------------------------------------------------

/// Whether a browser window opened here would be in front of somebody.
///
/// Over SSH it would open on the remote machine's screen, or nowhere; a
/// Linux session with no `DISPLAY` or `WAYLAND_DISPLAY` has no screen.
pub(crate) fn has_desktop(os: &str, over_ssh: bool, display: bool) -> bool {
    if over_ssh {
        return false;
    }
    matches!(os, "windows" | "macos") || display
}

pub(crate) fn desktop_here() -> bool {
    let over_ssh =
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some();
    let display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    has_desktop(std::env::consts::OS, over_ssh, display)
}

/// The program that opens `url` in the default browser: what each system
/// ships, so nothing is installed for it. `rundll32` rather than `cmd /c
/// start`, because `cmd` would read the `&` of a URL as a second command.
pub(crate) fn browser_command(os: &str, url: &str) -> Command {
    let mut command = match os {
        "windows" => {
            let mut command = Command::new("rundll32");
            command.arg("url.dll,FileProtocolHandler");
            command
        }
        "macos" => Command::new("open"),
        _ => Command::new("xdg-open"),
    };
    command.arg(url);
    command
}

/// Is the username typed taken at the coordinator typed? Answers "unknown"
/// rather than failing when the coordinator cannot be asked: the person may
/// be offline, and the engine asks again before any key is written.
fn check_name(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let (Some(username), Some(coordinator)) =
        (field(&form, "username"), field(&form, "coordinator"))
    else {
        return ok_json(object(&[("taken", quote("unknown"))]));
    };
    let answer = match (ctx.backends.name)(coordinator, username) {
        Ok(true) => "yes",
        Ok(false) => "no",
        Err(_) => "unknown",
    };
    ok_json(object(&[("taken", quote(answer))]))
}

/// The nearest directory that exists at or above `path`: free space is a
/// property of a disk, and the folder chosen may not exist yet.
fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|dir| dir.is_dir())
}

/// Free space on the disk that would hold `path` (the folder chosen), for
/// the slider's upper end.
fn free_space(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let path =
        field(&form, "path").map_or_else(|| ctx.base.clone(), |p| answers::expand(p, &ctx.base));
    let free = existing_ancestor(&path).and_then(|dir| fs4::available_space(dir).ok());
    ok_json(object(&[
        ("free_bytes", free.unwrap_or(0).to_string()),
        ("free", optional(free.map(format_size).as_deref())),
    ]))
}

/// The system's own folder chooser, started from `start`. Returns the
/// program to run; the chosen path is its standard output. The start folder
/// goes in an environment variable, never inside the script's text.
pub(crate) fn picker_command(os: &str, start: &Path) -> Command {
    let mut command = match os {
        "windows" => {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-STA",
                "-Command",
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
                 Add-Type -AssemblyName System.Windows.Forms; \
                 $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
                 $d.Description = 'ITSaNAS'; $d.ShowNewFolderButton = $true; \
                 $d.SelectedPath = $env:ITSANAS_PICK_START; \
                 $owner = New-Object System.Windows.Forms.Form; $owner.TopMost = $true; \
                 if ($d.ShowDialog($owner) -eq 'OK') { [Console]::Out.Write($d.SelectedPath) }",
            ]);
            command
        }
        "macos" => {
            let mut command = Command::new("osascript");
            command.args([
                "-e",
                "set start to POSIX file (system attribute \"ITSANAS_PICK_START\")",
                "-e",
                "POSIX path of (choose folder with prompt \"ITSaNAS\" default location start)",
            ]);
            command
        }
        _ => {
            let mut command = Command::new("zenity");
            command.args(["--file-selection", "--directory", "--title=ITSaNAS"]);
            command.arg(format!("--filename={}/", start.display()));
            command
        }
    };
    command.env("ITSANAS_PICK_START", start);
    command
}

/// Ask the system's folder chooser; `{"path": ...}`, or no path when the
/// person cancelled.
fn pick(request: &Request) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let base = crate::config::user_home();
    let start = field(&form, "start").map_or_else(|| base.clone(), |p| answers::expand(p, &base));
    let start = existing_ancestor(&start).map_or(base, Path::to_path_buf);
    let output = picker_command(std::env::consts::OS, &start)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) => {
            let chosen = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            ok_json(object(&[(
                "path",
                optional((!chosen.is_empty()).then_some(chosen.as_str())),
            )]))
        }
        Err(error) => Response::refuse(
            500,
            &format!("this system's folder chooser could not open ({error}): type the path"),
        ),
    }
}

/// The program that shows `folder` in the file manager.
pub(crate) fn open_command(os: &str, folder: &Path) -> Command {
    let mut command = Command::new(match os {
        "windows" => "explorer.exe",
        "macos" => "open",
        _ => "xdg-open",
    });
    command.arg(folder);
    command
}

/// Open the synced folder, or pin it where the file manager keeps favourites:
/// Quick access on Windows, the GTK bookmarks on Linux. macOS has no command
/// for the Finder sidebar, and says how by hand.
/// A second account on this computer is a second instance, in a home of its
/// own: never the home of the account already here, nor of any other one.
/// Found on Windows: an old account's home made every step "Already done",
/// and the page offered no way to set up a new account beside it.
fn another_account(ctx: &Context, request: &Request) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let name = field(&form, "name").unwrap_or_default().trim().to_owned();
    let home = match itsanas_node::config::instance_home_in(&ctx.base, &name) {
        Ok(home) => home,
        Err(error) => return Response::refuse(400, &error.to_string()),
    };
    if home == ctx.home || Node::exists(&home) {
        return Response::refuse(
            409,
            "an account is already set up under that name on this computer; choose another name",
        );
    }
    match (ctx.backends.another)(&name) {
        Ok(()) => ok_json(object(&[(
            "said",
            quote(&format!("the setup of {name} opened in a new page")),
        )])),
        Err(why) => Response::refuse(500, &why),
    }
}

/// `itsanas --instance NAME setup`, detached: it opens its own page.
fn start_another(name: &str) -> std::result::Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = std::process::Command::new(exe)
        .args(["--instance", name, "setup"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start it: {error}"))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn open_folder(ctx: &Context, request: &Request, pin: bool) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let folder = field(&form, "path")
        .map(|p| answers::expand(p, &ctx.base))
        .or_else(|| {
            Config::load(&Node::config_path(&ctx.home))
                .ok()
                .and_then(|config| config.folder)
        });
    let Some(folder) = folder.filter(|folder| folder.is_dir()) else {
        return Response::refuse(400, "the folder does not exist yet");
    };
    let os = std::env::consts::OS;
    let said = if pin {
        match pin_folder(os, &folder) {
            Ok(said) => said,
            Err(why) => return Response::refuse(500, &why),
        }
    } else {
        match open_command(os, &folder)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
                format!("opened {}", folder.display())
            }
            Err(error) => return Response::refuse(500, &format!("could not open it: {error}")),
        }
    };
    ok_json(object(&[("said", quote(&said))]))
}

/// The line a GTK bookmarks file holds for `folder`.
pub(crate) fn gtk_bookmark(folder: &Path) -> String {
    let mut url = String::from("file://");
    for byte in folder.display().to_string().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

fn pin_folder(os: &str, folder: &Path) -> std::result::Result<String, String> {
    match os {
        "windows" => {
            let status = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    "(New-Object -ComObject Shell.Application).Namespace($env:ITSANAS_PIN).Self.InvokeVerb('pintohome')",
                ])
                .env("ITSANAS_PIN", folder)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(format!("{} is in Quick access", folder.display()))
            } else {
                Err(
                    "Windows refused to pin it: right-click the folder, Pin to Quick access"
                        .to_owned(),
                )
            }
        }
        "macos" => {
            Ok("macOS has no command for this: drag the folder into Finder's sidebar".to_owned())
        }
        _ => {
            let file = crate::config::user_home().join(".config/gtk-3.0/bookmarks");
            let line = gtk_bookmark(folder);
            let existing = std::fs::read_to_string(&file).unwrap_or_default();
            if !existing
                .lines()
                .any(|l| l.split(' ').next() == Some(line.as_str()))
            {
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                }
                let mut text = existing;
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&line);
                text.push_str(" ITSaNAS\n");
                std::fs::write(&file, text).map_err(|error| error.to_string())?;
            }
            Ok(format!(
                "{} is in your file manager's bookmarks",
                folder.display()
            ))
        }
    }
}

/// `itsanas setup` on a desktop.
pub(crate) fn wizard(home: &Path, instance: Option<&str>) -> Result<()> {
    if open(home, instance, Mode::Setup, None)? {
        println!("This machine is set up. `itsanas settings` changes it later.");
        Ok(())
    } else {
        Err(CliError::Usage(
            "setup did not finish in the page. Run `itsanas setup` again: the steps already \
             done are kept and not asked again (`itsanas setup --text` asks in this terminal)."
                .to_owned(),
        ))
    }
}

/// `itsanas settings [--section NAME]`.
pub(crate) fn settings(home: &Path, instance: Option<&str>, section: Option<&str>) -> Result<()> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    open(home, instance, Mode::Settings, section).map(|_| ())
}

/// The page's address, opened at `section`: inside the one fragment an
/// address has, after the page key, which `app.js` reads wherever it is.
pub(crate) fn section_url(url: String, section: Option<&str>) -> String {
    match section {
        Some(section) => format!("{url}&section={section}"),
        None => url,
    }
}

fn open(home: &Path, instance: Option<&str>, mode: Mode, section: Option<&str>) -> Result<bool> {
    let server = Server::bind(
        home,
        instance,
        mode,
        &crate::config::user_home(),
        Backends::of_this_machine(home, instance),
    )?;
    let url = section_url(server.url(), section);
    if desktop_here() {
        println!("Opening ITSaNAS in your browser.");
        println!("If no page opened, paste this into your browser:");
        println!("  {url}");
        launch_browser(&url);
    } else {
        // `settings` over SSH: the page is still the way in, through a
        // forwarded port that keeps the same number, so the Host matches.
        let port = server.ctx.port;
        println!("No desktop here. From your own computer, run");
        println!("  ssh -L {port}:127.0.0.1:{port} <this machine>");
        println!("and open this in your browser there:");
        println!("  {url}");
    }
    println!(
        "The address works only on this machine and only while this command runs (Ctrl+C \
         stops it; it also stops by itself after 30 minutes without a click)."
    );
    Ok(server.serve(IDLE))
}

fn launch_browser(url: &str) {
    let spawned = browser_command(std::env::consts::OS, url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Reaped on the side, so no zombie is left while the page runs.
            std::thread::spawn(move || child.wait());
        }
        Err(error) => println!("(The browser could not be opened: {error}.)"),
    }
}

#[cfg(test)]
mod tests;
