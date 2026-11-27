//! The settings of this machine: the shell's own settings, files, start at
//! login, the machines to sync with, hosting, steering and disk work.

use super::{
    super::ServiceControl,
    Context,
    features::Features,
    hosting,
    http::{Request, Response},
    json::{self, array, field, object, quote},
    node_call, ok_json,
};
use crate::{failure::Failure, node::Node};

/// A setting only the shell can keep (the fingerprint, mobile data): the page
/// says it, the shell is told. Refused where the shell has no such thing.
pub(super) fn host_setting(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let features = ctx.backends.features;
    // Not a setting: a phone's shell asks GitHub, a computer runs `itsanas update`.
    if field(&form, "name") == Some("update") {
        if let Some(refusal) = Features::refuse(features.update_check, "update check") {
            return refusal;
        }
        ctx.backends.host.event("update", "check");
        return ok_json(object(&[("name", quote("update"))]));
    }
    let (name, enabled, what) = match field(&form, "name") {
        Some("biometric") => (
            "biometric",
            features.biometric,
            "fingerprint or face unlock",
        ),
        Some("metered") => ("metered", features.metered, "choice about mobile data"),
        // A phone whose system rations background work (Android's standby
        // buckets put an app not opened for hours in RARE, where its worker
        // runs a few times a day): the shell asks the system to let it off.
        Some("battery") => ("battery", features.metered, "choice about battery limits"),
        _ => return Response::refuse(400, "no such setting"),
    };
    if let Some(refusal) = Features::refuse(enabled, what) {
        return refusal;
    }
    let Some(value @ ("on" | "off")) = field(&form, "value") else {
        return Response::refuse(400, "the value is on or off");
    };
    ctx.backends.host.event(name, value);
    ok_json(object(&[("name", quote(name)), ("value", quote(value))]))
}

/// Free a file of the synced folder here, or keep it here again
/// ([`crate::freeing`]); answers the list as it now stands.
pub(super) fn files_change(ctx: &Context, request: &Request) -> Response {
    const ACTIONS: &str = "the action is free, keep, everywhere, versions, restore or rename";
    let form = json::parse_form(&request.body).unwrap_or_default();
    let (Some(action), Some(path)) = (field(&form, "action"), field(&form, "path")) else {
        return Response::refuse(400, &format!("{ACTIONS}, with a path"));
    };
    let at = field(&form, "at").and_then(|at| at.parse::<usize>().ok());
    let to = field(&form, "to").map(str::to_owned);
    match action {
        "restore" if at.is_none() => return Response::refuse(400, "restore needs at"),
        "rename" if to.as_deref().is_none_or(str::is_empty) => {
            return Response::refuse(400, "rename needs to");
        }
        "free" | "keep" | "everywhere" | "versions" | "restore" | "rename" => {}
        _ => return Response::refuse(400, ACTIONS),
    }
    node_call(ctx, |node| {
        let mut versions = None;
        let said = match action {
            "keep" => crate::freeing::keep(node, path)?,
            "everywhere" => crate::freeing::delete_everywhere(node, path)?,
            // The versions of one file, newest first, beside the list: the page
            // shows them under it, each with Bring back (§8 15).
            "versions" => {
                versions = Some(
                    serde_json::from_str::<serde_json::Value>(&crate::phone::versions_of(
                        node, path,
                    )?)
                    .map_err(|error| Failure::Usage(error.to_string()))?,
                );
                String::new()
            }
            "restore" => {
                let at = at.unwrap_or_default();
                crate::phone::bring_back(node, path, at)?;
                format!("{path} is version {} again, on every machine", at + 1)
            }
            "rename" => {
                let to = to.clone().unwrap_or_default();
                crate::phone::rename(node, path, &to)?;
                format!("{path} is now {to}, on every machine")
            }
            _ => crate::freeing::free(node, path)?,
        };
        let mut listed: serde_json::Value = serde_json::from_str(&crate::freeing::files(node)?)
            .map_err(|error| Failure::Usage(error.to_string()))?;
        listed["said"] = serde_json::Value::String(said);
        if let Some(versions) = versions {
            listed["versions"] = serde_json::json!({ "path": path, "list": versions["versions"] });
        }
        Ok(listed.to_string())
    })
}

/// Whether this account's background service starts at login, and the switch.
/// The service is this instance's own, so one account's choice leaves the others'.
pub(super) fn autostart_change(ctx: &Context, request: &Request) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let on = match field(&form, "on") {
        Some("true") => true,
        Some("false") => false,
        _ => return Response::refuse(400, "on is true or false"),
    };
    let service = (ctx.backends.service)();
    if !service.installed() {
        return Response::refuse(
            409,
            "no background service runs this account here: the setup offers one",
        );
    }
    match service.set_autostart(on) {
        Ok(()) => ok_json(autostart_json(
            service.as_ref(),
            if on {
                "It starts when you log in."
            } else {
                "It no longer starts when you log in. It runs until it is stopped."
            },
        )),
        Err(error) => Response::refuse(500, &error.to_string()),
    }
}

pub(super) fn autostart_json(service: &dyn ServiceControl, said: &str) -> String {
    let installed = service.installed();
    let on = if installed { service.autostart() } else { None };
    object(&[
        ("installed", installed.to_string()),
        (
            "on",
            on.map_or_else(|| "null".to_owned(), |on| on.to_string()),
        ),
        ("said", quote(said)),
    ])
}

pub(super) fn peers_list(ctx: &Context) -> Response {
    node_call(ctx, |node| Ok(peers_json(node, "")))
}

/// Add or forget a typed address: what `itsanas peer add` and `peer remove` do.
pub(super) fn peers_change(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let (Some(action), Some(address)) = (field(&form, "action"), field(&form, "address")) else {
        return Response::refuse(400, "an action (add or forget) and an address are needed");
    };
    node_call(ctx, |node| {
        let said = match action {
            "add" => crate::add_peer(node, address)?,
            "forget" => crate::forget_peer(node, address)?,
            _ => return Err(Failure::Usage("the action is add or forget".to_owned())),
        };
        Ok(peers_json(node, &said))
    })
}

pub(super) fn peers_json(node: &Node, said: &str) -> String {
    object(&[
        (
            "peers",
            array(node.config.peers.iter().map(|peer| quote(peer))),
        ),
        ("said", quote(said)),
    ])
}

/// Pause, resume, sync now, interval: the control file, through the same
/// function the commands use, so the page says the commands' words.
/// The switch and its settings, read from the file: asking must not disturb a
/// loop that is running.
pub(super) fn hosting_values(ctx: &Context) -> Response {
    if let Some(refusal) = Features::refuse(ctx.backends.features.hosting, "hosting") {
        return refusal;
    }
    match itsanas_node::config::Config::load(&Node::config_path(&ctx.home)) {
        Ok(config) => ok_json(hosting::to_json(&config)),
        Err(error) => Response::refuse(409, &error.to_string()),
    }
}

/// Save them on the open node, so the next look of the policy reads them.
pub(super) fn hosting_save(ctx: &Context, request: &Request) -> Response {
    if let Some(refusal) = Features::refuse(ctx.backends.features.hosting, "hosting") {
        return refusal;
    }
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    node_call(ctx, |node| {
        hosting::apply(&mut node.config, &form).map_err(Failure::Usage)?;
        node.save_config()?;
        Ok(hosting::to_json(&node.config))
    })
}

pub(super) fn steer(ctx: &Context, request: &Request) -> Response {
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
pub(super) fn disk_job(ctx: &Context, request: &Request) -> Response {
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
