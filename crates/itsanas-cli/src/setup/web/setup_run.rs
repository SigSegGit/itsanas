//! The setup run: the form turned into answers, the engine started, and the
//! jobs that ask in a native window (I forgot, the words, the passphrase).

use std::{path::Path, sync::Arc};

use super::{
    super::{Account, Answers, Event, Keep, Setup, answers},
    Context, Mode, Phase, Run,
    backends::Backends,
    http::{Request, Response},
    json::{self, field, object},
    lock,
    preferences::{save_language, save_theme},
    start_job,
};
use crate::config::parse_size;

/// The answers a form holds. In Settings, only what the person changed is
/// sent, and the account is never touched.
pub(super) fn answers_from_form(
    form: &[(String, String)],
    mode: Mode,
    instance: Option<&str>,
    base: &Path,
    service_installed: bool,
    shell: &Backends,
) -> std::result::Result<Answers, String> {
    let mut answers = Answers {
        instance: instance.map(str::to_owned),
        phone: shell.phone,
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
        // A shell that offers no choice runs in the background, always.
        answers.service =
            !shell.features.background_choice || field(form, "background") != Some("no");
    } else {
        // A node run by a daemon started by hand has no service to restart;
        // the engine then says to stop it, rather than install one unasked.
        answers.service = service_installed;
    }
    answers.tray = answers.service;
    answers.folder = field(form, "folder")
        .filter(|_| shell.features.folder)
        .map(|folder| answers::expand(folder, base));
    match &shell.fixed_coordinator {
        // What the form says is not read: the address and the pin are the shell's.
        Some((address, device)) => {
            answers.coordinator = Some(address.clone());
            answers.coordinator_device.clone_from(device);
        }
        None => answers.coordinator = field(form, "coordinator").map(str::to_owned),
    }
    answers.invite = field(form, "invite").map(str::to_owned);
    answers.pledge = field(form, "pledge")
        .map(|text| parse_size(text).map_err(|error| error.to_string()))
        .transpose()?;
    answers.vault = field(form, "vault")
        .filter(|_| shell.features.folder)
        .map(|vault| answers::expand(vault, base));
    // Present and empty is "everything"; absent is "not asked".
    answers.keep = match form
        .iter()
        .find(|(name, _)| name == "keep")
        .filter(|_| shell.features.keep_question)
    {
        None => Keep::Unchanged,
        Some((_, text)) if text.trim().is_empty() => Keep::Everything,
        Some((_, text)) => {
            Keep::Budget(parse_size(text.trim()).map_err(|error| error.to_string())?)
        }
    };
    answers.updates = field(form, "updates")
        .filter(|_| shell.features.advanced)
        .map(|text| {
            crate::config::Updates::parse(text)
                .ok_or_else(|| "updates is auto, notify or off".to_owned())
        })
        .transpose()?;
    Ok(answers)
}

pub(super) fn start_run(ctx: &Arc<Context>, request: &Request) -> Response {
    let installed = (ctx.backends.service)().installed();
    let answers = match json::parse_form(&request.body).and_then(|form| {
        answers_from_form(
            &form,
            ctx.mode,
            ctx.instance.as_deref(),
            &ctx.base,
            installed,
            &ctx.backends,
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

pub(super) fn run_engine(ctx: &Context, answers: Answers) {
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
            .with_owner_check(ctx.backends.owner)
            .run())
        }
        Err(error) => Err(error.to_string()),
    };
    // A language or a theme chosen before the home existed is kept now that it does.
    if let Some(language) = *lock(&ctx.language) {
        save_language(&ctx.home, language);
    }
    if let Some(theme) = *lock(&ctx.theme) {
        save_theme(&ctx.home, theme);
    }
    ctx.finish(outcome);
}

/// "I forgot my passphrase": the 24 words replace the keys on this machine
/// ([`super::super::forgot::forgot`]). Run on a thread of its own as a setup run is, the
/// words and the new passphrase asked in the native windows; the page follows it
/// through `/api/state` and never sees either.
pub(super) fn start_forgot(ctx: &Arc<Context>) -> Response {
    start_job(
        ctx,
        "forgot",
        "the 24 recovery words, then a new passphrase",
        |worker, prompt, service| {
            super::super::forgot::forgot(&worker.home, prompt, service, worker.backends.owner)
        },
    )
}

/// "Show my 24 words": the passphrase is asked in a native window, and so are the
/// words shown ([`super::super::keys::show_words`]). The page is told only that it was done.
pub(super) fn start_words(ctx: &Arc<Context>) -> Response {
    start_job(
        ctx,
        "words",
        "your passphrase, to show the 24 words",
        |worker, prompt, _| super::super::keys::show_words(&worker.home, prompt),
    )
}

/// "Change my passphrase": the current and the new one are asked in native windows
/// ([`super::super::keys::change_passphrase`]).
pub(super) fn start_passphrase(ctx: &Arc<Context>) -> Response {
    start_job(
        ctx,
        "passphrase",
        "your passphrase, then a new one",
        |worker, prompt, service| {
            super::super::keys::change_passphrase(&worker.home, prompt, service)
        },
    )
}
