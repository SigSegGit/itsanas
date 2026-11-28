//! One connection, read under a deadline, and the table that sends each
//! request to its handler, behind the page key, the origin check and the
//! features the shell has.

use std::{
    io::Read as _,
    net::{Shutdown, TcpStream},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use super::{
    APP_CSS, APP_JS, CONNECTION_DEADLINE, Context, I18N_JS, ICON_PNG, LEGAL_NOTICE, LEGAL_PRIVACY,
    LEGAL_TERMS, Mode, READ_TIMEOUT, TEXT,
    account_actions::{
        another_account, delete_account, devices_list, remove_account, remove_check, sign_out,
        withdraw_device,
    },
    deletion,
    desktop::{open_folder, pick},
    features::Features,
    http::{self, Request, Response},
    json::{self, field, object, quote},
    lock, node_call, ok_json,
    preferences::{choose_language, choose_theme, index_html},
    settings_actions::{
        autostart_change, autostart_json, copies_save, copies_values, disk_job, files_change,
        host_setting, hosting_save, hosting_values, peers_change, peers_list, steer,
    },
    setup_run::{start_forgot, start_passphrase, start_run, start_words},
    state::{check_name, free_space, plan_json, space_default, state_json, suggest},
    usb::{copy_state, key_check, keys, start_copy},
};

/// Reads from a stream, refusing to go past an absolute instant: each read
/// waits at most what is left, so dripping bytes cannot extend the life of a
/// connection the way they extend a per-read timeout.
pub(super) struct Deadline<'a> {
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
pub(super) fn handle(ctx: &Arc<Context>, mut stream: TcpStream) {
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

pub(super) fn route(ctx: &Arc<Context>, request: &Request) -> Response {
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
        ("GET", "/legal/notice") => Response::new(200, TEXT, LEGAL_NOTICE),
        ("GET", "/legal/privacy") => Response::new(200, TEXT, LEGAL_PRIVACY),
        ("GET", "/legal/terms") => Response::new(200, TEXT, LEGAL_TERMS),
        (_, path) if path.starts_with("/api/") => api(ctx, request),
        ("GET", _) => Response::refuse(404, "no such page"),
        _ => Response::refuse(405, "not here"),
    }
}

pub(super) fn api(ctx: &Arc<Context>, request: &Request) -> Response {
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
    if let Some(refusal) = gate(ctx.backends.features, path, request) {
        return refusal;
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
        ("GET", "/api/hosting") if ctx.mode == Mode::Settings => hosting_values(ctx),
        ("POST", "/api/hosting") if ctx.mode == Mode::Settings => hosting_save(ctx, request),
        ("GET", "/api/copies") if ctx.mode == Mode::Settings => copies_values(ctx),
        ("POST", "/api/copies") if ctx.mode == Mode::Settings => copies_save(ctx, request),
        ("POST", "/api/signout") if ctx.mode == Mode::Settings => sign_out(ctx),
        ("POST", "/api/disk") if ctx.mode == Mode::Settings => disk_job(ctx, request),
        ("POST", "/api/name") if ctx.mode == Mode::Setup => check_name(ctx, request),
        ("POST", "/api/another") if ctx.mode == Mode::Setup => another_account(ctx, request),
        ("POST", "/api/language") => choose_language(ctx, request),
        ("POST", "/api/theme") => choose_theme(ctx, request),
        ("POST", "/api/host") => host_setting(ctx, request),
        ("POST", "/api/suggest") => suggest(ctx, request),
        ("POST", "/api/space-default") => space_default(ctx),
        ("POST", "/api/forgot") if ctx.mode == Mode::Settings => start_forgot(ctx),
        ("POST", "/api/words") if ctx.mode == Mode::Settings => start_words(ctx),
        ("POST", "/api/passphrase") if ctx.mode == Mode::Settings => start_passphrase(ctx),
        ("GET", "/api/devices") => devices_list(ctx),
        ("POST", "/api/withdraw") => withdraw_device(ctx, request),
        ("GET", "/api/remove") if ctx.mode == Mode::Settings => remove_check(ctx),
        ("POST", "/api/remove") if ctx.mode == Mode::Settings => remove_account(ctx, request),
        ("POST", "/api/delete") if ctx.mode == Mode::Settings => delete_account(ctx, request),
        ("GET", "/api/trash") if ctx.mode == Mode::Settings => deletion::trash_list(ctx),
        ("POST", "/api/trash") if ctx.mode == Mode::Settings => {
            deletion::trash_change(ctx, request)
        }
        ("GET", "/api/deletion") if ctx.mode == Mode::Settings => deletion::rules(ctx),
        ("POST", "/api/deletion") if ctx.mode == Mode::Settings => {
            deletion::rules_change(ctx, request)
        }
        ("GET", "/api/traces") if ctx.mode == Mode::Settings => deletion::traces(ctx),
        ("POST", "/api/traces") if ctx.mode == Mode::Settings => {
            deletion::traces_change(ctx, request)
        }
        ("GET", "/api/files") if ctx.mode == Mode::Settings => {
            node_call(ctx, |node| crate::freeing::files(node))
        }
        ("POST", "/api/files") if ctx.mode == Mode::Settings => files_change(ctx, request),
        ("GET", "/api/autostart") if ctx.mode == Mode::Settings => {
            ok_json(autostart_json(&*(ctx.backends.service)(), ""))
        }
        ("POST", "/api/autostart") if ctx.mode == Mode::Settings => autostart_change(ctx, request),
        ("GET", "/api/peers") if ctx.mode == Mode::Settings => peers_list(ctx),
        ("POST", "/api/peers") if ctx.mode == Mode::Settings => peers_change(ctx, request),
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

/// The refusal for a route whose feature this shell lacks, before the route
/// runs. The page hides what is missing; this is what makes that more than looks.
pub(super) fn gate(features: Features, path: &str, request: &Request) -> Option<Response> {
    let (enabled, what) = match path {
        "/api/pick" => (features.folder, "folder to browse for"),
        "/api/copies" => (features.advanced, "choice of the copies' hosts"),
        "/api/open" | "/api/pin" => (features.folder, "synced folder"),
        "/api/disk" => (features.disks, "disk to move or back up to"),
        "/api/keys" | "/api/copy-to-key" => (features.usb_key, "USB key to copy to"),
        "/api/another" => (features.another_account, "second account beside this one"),
        "/api/forgot" => (
            features.forgot_in_settings,
            "way to replace the keys while the account is open: lock it, then choose I forgot",
        ),
        "/api/trash" => (
            features.folder,
            "trash on this page: a phone has its own, on its Files screen",
        ),
        "/api/files" => (
            features.folder,
            "list of files to free or keep: a phone keeps by its limit",
        ),
        "/api/autostart" => (
            features.background_choice,
            "switch for starting at login: the system starts this app",
        ),
        "/api/peers" => (
            features.advanced,
            "machine to sync with, named by its address",
        ),
        "/api/remove" | "/api/delete" => (
            features.remove_account,
            "way to remove or delete the account from here",
        ),
        // Sync now stays: it is what a phone's own button does.
        "/api/control" => {
            let form = json::parse_form(&request.body).unwrap_or_default();
            let timed = matches!(
                field(&form, "action"),
                Some("pause" | "resume" | "interval")
            );
            (
                features.sync_controls || !timed,
                "pause or schedule for syncing",
            )
        }
        _ => return None,
    };
    Features::refuse(enabled, what)
}
