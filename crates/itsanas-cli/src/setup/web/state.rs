//! What the page reads: the defaults, the run's state, the settings and the
//! plan, the space suggested, and the username and free-space checks.

use std::path::Path;

use super::{
    super::{Answers, answers, verify},
    Context, Mode, Run,
    features::Features,
    http::{Request, Response},
    json::{self, array, field, object, optional, quote},
    lock, ok_json, step_id,
};
use crate::{
    config::{Config, format_size},
    node::Node,
};

/// What to offer and keep here, from the one rule every platform shares
/// ([`itsanas_node::pledges::suggest`]). `largest_other` is the largest pledge of
/// the account's other machines when the person is joining one, else 0. Their sum
/// is not known before an account is joined; the largest stands in for it, which
/// can only suggest a smaller keep, never one the account could not earn.
pub(super) fn suggestion(ctx: &Context, largest_other: u64) -> itsanas_node::pledges::Suggestion {
    let disk = itsanas_node::pledges::disk_space(&ctx.base);
    let config = Config::load(&Node::config_path(&ctx.home)).unwrap_or_default();
    itsanas_node::pledges::suggest(
        &config,
        disk.total.unwrap_or(0),
        disk.free.unwrap_or(0),
        0,
        largest_other,
        largest_other,
    )
}

/// `POST /api/suggest`: the suggestion once the person says what their other
/// machines offer.
pub(super) fn suggest(ctx: &Context, request: &Request) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let other = field(&form, "other")
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(0);
    let found = suggestion(ctx, other);
    ok_json(object(&[
        ("pledge_gb", (found.pledge >> 30).to_string()),
        ("keep_gb", (found.keep >> 30).to_string()),
        (
            "same_as_other_machines",
            found.same_as_other_machines.to_string(),
        ),
    ]))
}

/// `POST /api/space-default`: what the account's machines offer, and whether
/// this phone has room for the same. Asked on the button only: it is a request
/// to the coordinator.
pub(super) fn space_default(ctx: &Context) -> Response {
    if let Some(refusal) = Features::refuse(ctx.backends.features.metered, "space slider") {
        return refusal;
    }
    let config = Config::load(&Node::config_path(&ctx.home)).unwrap_or_default();
    let fixed = ctx
        .backends
        .fixed_coordinator
        .as_ref()
        .map(|(address, _)| address.as_str());
    let other = fixed
        .or(config.coordinator.as_deref())
        .and_then(|address| (ctx.backends.pledge)(address, &config.username))
        .unwrap_or(0);
    let free = fs4::available_space(&ctx.base).unwrap_or(0);
    let same = itsanas_node::pledges::same_as_others_gb(other, free);
    ok_json(object(&[
        ("other_gb", (other >> 30).to_string()),
        ("same_gb", same.unwrap_or(0).to_string()),
    ]))
}

/// What the pages start from: the folder, the suggestion, the coordinator.
pub(super) fn defaults_json(ctx: &Context) -> String {
    let free = fs4::available_space(&ctx.base).ok();
    let suggested = suggestion(ctx, 0);
    object(&[
        (
            "folder",
            quote(
                &answers::default_folder(&ctx.base, ctx.instance.as_deref())
                    .display()
                    .to_string(),
            ),
        ),
        ("pledge_gb", (suggested.pledge >> 30).to_string()),
        ("keep_gb", (suggested.keep >> 30).to_string()),
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
            "slider_max_gb",
            itsanas_node::pledges::slider_max_gb(free.unwrap_or(0)).to_string(),
        ),
        (
            "coordinator",
            quote(&ctx.backends.fixed_coordinator.as_ref().map_or_else(
                || {
                    Config::load(&Node::config_path(&ctx.home))
                        .ok()
                        .and_then(|config| config.coordinator)
                        .unwrap_or_else(|| super::super::DEFAULT_COORDINATOR.to_owned())
                },
                |(address, _)| address.clone(),
            )),
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
    ])
}

pub(super) fn state_json(ctx: &Context) -> String {
    let defaults = defaults_json(ctx);
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
        ("phone", ctx.backends.phone.to_string()),
        ("version", quote(env!("CARGO_PKG_VERSION"))),
        ("theme", quote((*lock(&ctx.theme)).unwrap_or("system"))),
        (
            "host",
            object(&[
                (
                    "biometric",
                    optional(ctx.backends.host.value("biometric").as_deref()),
                ),
                (
                    "metered",
                    optional(ctx.backends.host.value("metered").as_deref()),
                ),
                (
                    "battery",
                    optional(ctx.backends.host.value("battery").as_deref()),
                ),
            ]),
        ),
        (
            "features",
            object(
                &ctx.backends
                    .features
                    .all()
                    .map(|(name, on)| (name, on.to_string())),
            ),
        ),
        ("run", run_json(&lock(&ctx.run))),
    ];
    if ctx.mode == Mode::Settings {
        fields.push(("settings", settings_json(&ctx.home)));
    }
    object(&fields)
}

pub(super) fn run_json(run: &Run) -> String {
    let failed = run.failed.as_ref().map_or_else(
        || "null".to_owned(),
        |(step, error, remedy)| {
            object(&[
                ("step", quote(step_id(*step))),
                ("title", quote(step.title())),
                ("error", quote(error)),
                ("remedy", quote(remedy)),
                // The account is at its device limit: the page opens the
                // devices to withdraw one ([`crate::coordinator::cap_named`]).
                (
                    "atCap",
                    error.contains("live devices and the limit is").to_string(),
                ),
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
pub(super) fn settings_json(home: &Path) -> String {
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
            "pledge_gb",
            config
                .as_ref()
                .map_or(0, |c| c.pledge_bytes >> 30)
                .to_string(),
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
pub(super) fn plan_json(ctx: &Context) -> String {
    let found = Answers {
        instance: ctx.instance.clone(),
        phone: ctx.backends.phone,
        ..Answers::default()
    };
    let service = (ctx.backends.service)();
    array(
        super::super::plan(&ctx.home, &found, service.as_ref())
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

/// Is the username typed taken at the coordinator typed? Answers "unknown"
/// rather than failing when the coordinator cannot be asked: the person may
/// be offline, and the engine asks again before any key is written. Answers
/// "invalid", with the reason, for a name no coordinator would register: on a
/// lookup it reads as free, and `marie_1` was told so, made her keys, and was
/// refused at registration with a remedy about something else.
pub(super) fn check_name(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let fixed = ctx
        .backends
        .fixed_coordinator
        .as_ref()
        .map(|(address, _)| address.as_str());
    let (Some(username), Some(coordinator)) = (
        field(&form, "username"),
        fixed.or(field(&form, "coordinator")),
    ) else {
        return ok_json(name_json("unknown", None, None));
    };
    if let Some(why) = crate::coordinator::username_refusal(username) {
        return ok_json(name_json("unknown", Some(&why), None));
    }
    let answer = match (ctx.backends.name)(coordinator, username) {
        Ok(true) => "yes",
        Ok(false) => "no",
        Err(_) => "unknown",
    };
    // What the account's machines offer is only worth a request to someone who
    // is joining that account.
    let other = (answer == "yes" && field(&form, "account") == Some("join"))
        .then(|| (ctx.backends.pledge)(coordinator, username))
        .flatten();
    ok_json(name_json(answer, None, other))
}

/// `taken` is `yes`, `no` or `unknown`; `invalid` says why no coordinator would
/// take the name (and then `taken` is `unknown`: the network was not asked);
/// `other_pledge_bytes` is the largest pledge of the account's machines.
pub(super) fn name_json(
    taken: &str,
    invalid: Option<&str>,
    other_pledge_bytes: Option<u64>,
) -> String {
    object(&[
        ("taken", quote(taken)),
        ("invalid", optional(invalid)),
        (
            "other_pledge_bytes",
            other_pledge_bytes.map_or_else(|| "null".to_owned(), |bytes| bytes.to_string()),
        ),
    ])
}

/// The nearest directory that exists at or above `path`: free space is a
/// property of a disk, and the folder chosen may not exist yet.
pub(super) fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|dir| dir.is_dir())
}

/// Free space on the disk that would hold `path` (the folder chosen), for
/// the slider's upper end.
pub(super) fn free_space(ctx: &Context, request: &Request) -> Response {
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
