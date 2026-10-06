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
}

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

/// Read one request, answer it, close.
fn handle(ctx: &Arc<Context>, mut stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let response = match http::read_request(&mut stream) {
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
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = std::io::copy(
        &mut (&mut stream).take(http::MAX_BODY as u64),
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
        ("GET", "/" | "/index.html") => Response::new(200, "text/html; charset=utf-8", INDEX_HTML),
        ("GET", "/app.css") => Response::new(200, "text/css; charset=utf-8", APP_CSS),
        ("GET", "/app.js") => Response::new(200, "text/javascript; charset=utf-8", APP_JS),
        (_, path) if path.starts_with("/api/") => api(ctx, request),
        ("GET", _) => Response::refuse(404, "no such page"),
        _ => Response::refuse(405, "not here"),
    }
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
    if (method, path) != ("GET", "/api/state") {
        *lock(&ctx.last_action) = Instant::now();
    }
    match (method, path) {
        ("GET", "/api/state") => ok_json(state_json(ctx)),
        ("GET", "/api/plan") => ok_json(plan_json(ctx)),
        ("POST", "/api/run") => start_run(ctx, request),
        ("POST", "/api/control") if ctx.mode == Mode::Settings => steer(ctx, request),
        ("POST", "/api/signout") if ctx.mode == Mode::Settings => sign_out(ctx),
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
        ("free", optional(free.map(format_size).as_deref())),
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
            .run())
        }
        Err(error) => Err(error.to_string()),
    };
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
        // TODO(0w (5)): a pause with an end ("for 1 hour", "for 8 hours")
        // needs the control file's `until` key, built with `itsanas pause
        // --for`; until then the page offers only "until I resume", and a
        // duration sent anyway is refused rather than silently ignored.
        Some("pause") if field(&form, "for").is_none_or(|f| f == "resume") => crate::Command::Pause,
        Some("pause") => {
            return Response::refuse(400, "a pause with an end is not available yet");
        }
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

/// `itsanas setup` on a desktop.
pub(crate) fn wizard(home: &Path, instance: Option<&str>) -> Result<()> {
    if open(home, instance, Mode::Setup)? {
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

/// `itsanas settings`.
pub(crate) fn settings(home: &Path, instance: Option<&str>) -> Result<()> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    open(home, instance, Mode::Settings).map(|_| ())
}

fn open(home: &Path, instance: Option<&str>, mode: Mode) -> Result<bool> {
    let server = Server::bind(
        home,
        instance,
        mode,
        &crate::config::user_home(),
        Backends::of_this_machine(home, instance),
    )?;
    let url = server.url();
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
