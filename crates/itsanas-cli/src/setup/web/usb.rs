//! Copying this account to a USB key: the drives listed and checked, the
//! copy started, and its progress followed.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use super::{
    Context,
    backends::{COPY_STALE, Running, SILENT_CHILD},
    http::{Request, Response},
    json::{self, array, field, object, optional, quote},
    lock, ok_json,
};
use crate::{
    config::{Config, format_size},
    node::Node,
};

/// What the programs take on a key: known once the background ask answered,
/// `None` meanwhile (the room check then counts the guess).
pub(super) fn programs_size(ctx: &Arc<Context>) -> Option<u64> {
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
pub(super) fn config_for_copy(ctx: &Context) -> std::result::Result<Config, Response> {
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

pub(super) fn number(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

/// One drive as the page lists it, with whether the copy fits there.
pub(super) fn key_json(
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
pub(super) fn sizes_json(ctx: &Arc<Context>) -> (Option<u64>, Vec<(&'static str, String)>) {
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
pub(super) fn keys(ctx: &Arc<Context>) -> Response {
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
pub(super) fn candidate_at(path: &Path) -> crate::drives::Candidate {
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
pub(super) fn key_check(ctx: &Arc<Context>, request: &Request) -> Response {
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
pub(super) fn copy_running(ctx: &Context, slot: &mut Option<Running>) -> bool {
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
pub(super) fn copy_state(ctx: &Context) -> Response {
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
pub(super) fn start_copy(ctx: &Arc<Context>, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let config = match config_for_copy(ctx) {
        Ok(config) => config,
        Err(refused) => return refused,
    };
    let path = PathBuf::from(field(&form, "path").unwrap_or_default());
    // Every refusal of the command itself (`check_path` first, before the
    // system is asked anything about the path), and the room, before a
    // window opens for a copy that cannot happen.
    if let Err(said) = crate::jobs::check_path(&path) {
        return Response::refuse(400, &said);
    }
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
        Ok(mut child) => {
            // The child this page started counts as running for two minutes
            // after it ends, until a status written since says it spoke: its
            // background run may be starting, or it died before a word, and
            // a second copy then would race the first on the key.
            let started = std::time::SystemTime::now();
            let at = Instant::now();
            let home = ctx.home.clone();
            let running: Running = Box::new(move || {
                child()
                    || (at.elapsed() < SILENT_CHILD
                        && crate::copy_to_key::progress(&home)
                            .is_none_or(|(_, written)| written + Duration::from_secs(2) < started))
            });
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
