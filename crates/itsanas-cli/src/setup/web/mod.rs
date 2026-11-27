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

mod deletion;
mod features;
mod hosting;
mod http;
mod json;
mod nodes;

pub use features::{Features, Host, NoHost};
pub use nodes::{Held, Nodes, Slot};

mod account_actions;
mod backends;
mod desktop;
mod preferences;
mod routing;
mod settings_actions;
mod setup_run;
mod state;
mod usb;

pub(crate) use self::backends::Running;
pub use self::backends::{Backends, MakePrompt, MakeService};
#[cfg(test)]
pub(crate) use self::desktop::section_url;
pub(crate) use self::desktop::{desktop_here, settings, wizard};
pub(crate) use self::preferences::{saved_language, saved_theme};

use std::{
    fmt::Write as _,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use self::{
    http::{Request, Response},
    json::object,
    routing::handle,
};
use super::{Event, SecretPrompt, ServiceControl, Step, verify};
use crate::{
    error::{CliError, Result},
    failure::Failure,
    node::Node,
};

const INDEX_HTML: &str = include_str!("index.html");
const APP_CSS: &str = include_str!("app.css");
const LEGAL_NOTICE: &str = include_str!("../../../../../docs/legal/NOTICE.md");
const LEGAL_PRIVACY: &str = include_str!("../../../../../docs/legal/PRIVACY.md");
const LEGAL_TERMS: &str = include_str!("../../../../../docs/legal/TERMS.md");
const TEXT: &str = "text/plain; charset=utf-8";
/// The page's script, one concern per file of `app/`, served as one `/app.js`:
/// the files hold one function between them, opened in the first and closed
/// in the last, so their order is the script's.
const APP_JS: &str = concat!(
    include_str!("app/page.js"),
    include_str!("app/setup.js"),
    include_str!("app/settings.js"),
    include_str!("app/trash.js"),
    include_str!("app/account.js"),
    include_str!("app/usb.js"),
    include_str!("app/init.js"),
);
/// The page's other languages, keyed by its English text, one table per file
/// of `i18n/`, served as one `/i18n.js`.
const I18N_JS: &str = concat!(include_str!("i18n/fr.js"), include_str!("i18n/zh.js"));
/// The project's icon (docs/assets), in the header and as the favicon.
const ICON_PNG: &[u8] = include_bytes!("../../../../../docs/assets/icon.png");

/// What the page is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
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
    /// The theme chosen in Settings, as [`Context::language`] is.
    theme: Mutex<Option<&'static str>>,
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
pub struct Server {
    listener: TcpListener,
    ctx: Arc<Context>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The port, never the key: a debug print is what ends up in a log.
        f.debug_struct("Server")
            .field("port", &self.ctx.port)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Bind 127.0.0.1 on a port the system chooses, with a fresh token.
    pub fn bind(
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
                theme: Mutex::new(saved_theme(home)),
                programs: Mutex::new(None),
                programs_asked: AtomicBool::new(false),
                copying: Mutex::new(None),
            }),
        })
    }

    /// The address to open, the token in its fragment.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/#t={}", self.ctx.port, self.ctx.token)
    }

    /// A handle that ends [`Server::serve`] from another thread: the phone's
    /// shell, whose person leaves with Back, has no page to say it is done and
    /// no wish to wait for the idle exit with the account still open.
    #[must_use]
    pub fn stopper(&self) -> Stop {
        Stop(Arc::clone(&self.ctx))
    }

    /// Answer requests until the page says it is done, the setup finished,
    /// or nobody did anything for `idle`. True when a setup run finished well.
    #[must_use]
    pub fn serve(&self, idle: Duration) -> bool {
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

/// See [`Server::stopper`].
pub struct Stop(Arc<Context>);

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stop").finish_non_exhaustive()
    }
}

impl Stop {
    /// Make `serve` return within a poll.
    pub fn stop(&self) {
        self.0.quit.store(true, Ordering::SeqCst);
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

/// Run `body` with the node behind the page ([`Nodes`]) and answer with the JSON
/// it makes, or with the sentence it was refused in. The node is given back
/// afterwards, whatever came of it.
fn node_call(
    ctx: &Context,
    body: impl FnOnce(&mut Node) -> std::result::Result<String, Failure>,
) -> Response {
    match with_node(ctx, body) {
        Ok(json) => ok_json(json),
        Err(failure) => Response::refuse(409, &failure.message()),
    }
}

/// [`node_call`]'s work, for a caller that answers in its own way (a job).
fn with_node(
    ctx: &Context,
    body: impl FnOnce(&mut Node) -> std::result::Result<String, Failure>,
) -> std::result::Result<String, Failure> {
    let nodes = &ctx.backends.nodes;
    nodes.open()?;
    // A change to the node needs it alone. The service's next look starts the
    // loop again, on the new values.
    crate::host_run::stop_current();
    let outcome = {
        let mut held = nodes.slot().lock().unwrap_or_else(PoisonError::into_inner);
        match held.as_mut().and_then(std::sync::Arc::get_mut) {
            Some(node) => body(node),
            None => Err(Failure::Closed),
        }
    };
    nodes.release();
    outcome
}

/// What a secret job does with the prompt and the service it is given.
type JobBody = fn(&Context, &mut dyn SecretPrompt, &dyn ServiceControl) -> Result<String>;

/// A job that carries what it was asked (the files to delete, the rules to set).
type BoxedJob =
    Box<dyn FnOnce(&Context, &mut dyn SecretPrompt, &dyn ServiceControl) -> Result<String> + Send>;

/// Start `body` on a thread of its own, as a setup run is: the page follows it
/// through `/api/state` (`waiting` says what the window wants) and never sees a
/// secret. One at a time with every run.
fn start_job(
    ctx: &Arc<Context>,
    name: &'static str,
    waiting: &'static str,
    body: JobBody,
) -> Response {
    start_boxed(ctx, name, waiting, Box::new(body))
}

/// [`start_job`] for a job that carries its request.
fn start_boxed(
    ctx: &Arc<Context>,
    name: &'static str,
    waiting: &'static str,
    body: BoxedJob,
) -> Response {
    {
        let mut run = lock(&ctx.run);
        if run.phase == Phase::Running {
            return Response::refuse(409, "it is already running; this page follows it");
        }
        *run = Run {
            phase: Phase::Running,
            step: Some(Step::Account),
            waiting: Some(waiting),
            ..Run::default()
        };
    }
    let worker = Arc::clone(ctx);
    let spawned = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let service = (worker.backends.service)();
            let result = (worker.backends.prompt)()
                .and_then(|mut prompt| body(&worker, prompt.as_mut(), service.as_ref()));
            let mut run = lock(&worker.run);
            match result {
                Ok(said) => {
                    run.lines.push(said);
                    run.phase = Phase::Done;
                }
                Err(error) => {
                    run.failed = Some((
                        Step::Account,
                        error.to_string(),
                        "nothing was changed on this machine unless it says otherwise; try again"
                            .to_owned(),
                    ));
                    run.phase = Phase::Failed;
                }
            }
            run.waiting = None;
            run.finished_at = Some(Instant::now());
        });
    if spawned.is_err() {
        ctx.finish(Err("could not start".to_owned()));
    }
    Response::new(
        202,
        "application/json",
        object(&[("started", "true".to_owned())]),
    )
}

#[cfg(test)]
mod tests;
