//! What Settings does to the account: the devices listed and withdrawn,
//! removing or deleting the account, signing out, and another account.

use std::{process::Stdio, sync::PoisonError};

use super::{
    super::sign,
    Context,
    http::{Request, Response},
    json::{self, field, object, quote},
    node_call, ok_json,
};
use crate::{failure::Failure, node::Node};

/// The account's devices, this machine marked, as the coordinator lists them.
pub(super) fn devices_list(ctx: &Context) -> Response {
    node_call(ctx, |node| {
        crate::devices::list(node).map(|listed| listed.to_string())
    })
}

/// Withdraw one of the account's devices by its full id: never this machine,
/// never an id the coordinator's list does not show ([`crate::devices::withdraw`]).
pub(super) fn withdraw_device(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let Some(id) = field(&form, "id") else {
        return Response::refuse(400, "which device? the full id is needed");
    };
    node_call(ctx, |node| {
        crate::devices::withdraw(node, id, itsanas_discover::now_unix())
            .map(|answer| answer.to_string())
    })
}

/// One best-effort round, then what removing the account from this machine
/// would lose ([`crate::account::logout_check`]). Closes nothing.
pub(super) fn remove_check(ctx: &Context) -> Response {
    // Only the person's own choice against mobile data is known here; asking
    // for less is the safe side: more is reported as only here.
    let metered = ctx.backends.host.value("metered").as_deref() == Some("off");
    node_call(ctx, |node| crate::account::logout_check(node, metered))
}

/// The service is not a thing to leave running over an account that is gone,
/// and the shell is told it is: its screens go back to the beginning.
pub(super) fn account_gone(ctx: &Context, how: &str) {
    let service = (ctx.backends.service)();
    if service.installed() {
        let _ = service.set_autostart(false);
        let _ = service.stop();
        // A computer's task, unit or agent would start again for an account
        // that is gone; a phone app is its own service and keeps nothing.
        let _ = service.uninstall();
    }
    let _ = service.forget_passphrase();
    ctx.backends.nodes.keep_stopped();
    ctx.backends.nodes.release();
    ctx.backends.host.event("account_removed", how);
}

/// Remove the account from this machine: the machines are told, this device is
/// withdrawn, the home erased ([`crate::account::logout_into`]). Where pieces of
/// the account's files exist only here, the account's name must be typed.
pub(super) fn remove_account(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let confirm = field(&form, "confirm").unwrap_or_default();
    let nodes = &ctx.backends.nodes;
    if let Err(failure) = nodes.open() {
        return Response::refuse(409, &failure.message());
    }
    let now = itsanas_discover::now_unix();
    let refusal = {
        let held = nodes.slot().lock().unwrap_or_else(PoisonError::into_inner);
        held.as_ref().map(|node| {
            let only_here = node.store.coverage(now).map_or(usize::MAX, |c| c.only_here);
            let username = node.config.username.as_str();
            if only_here > 0 && confirm != username {
                Some(format!(
                    "{only_here} pieces of your files exist only on this machine, so the \
                     account's name must be typed to remove it. Nothing was removed."
                ))
            } else {
                None
            }
        })
    };
    match refusal {
        None => {
            nodes.release();
            return Response::refuse(409, &Failure::Closed.message());
        }
        Some(Some(why)) => {
            nodes.release();
            return Response::refuse(409, &why);
        }
        Some(None) => {}
    }
    match crate::account::logout_into(nodes.slot(), &ctx.home, now) {
        Ok(said) => {
            account_gone(ctx, "removed");
            let said: serde_json::Value = serde_json::from_str(&said).unwrap_or_default();
            ok_json(object(&[
                ("told", said["told"].to_string()),
                ("departed", said["departed"].to_string()),
                ("withdrawn", said["withdrawn"].to_string()),
            ]))
        }
        Err(failure) => {
            nodes.release();
            Response::refuse(409, &failure.message())
        }
    }
}

/// Delete the whole account at the coordinator, then remove it from this machine.
/// Only the account's own name, typed, gets this far
/// ([`crate::account::delete_account`]).
pub(super) fn delete_account(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let confirm = field(&form, "confirm").unwrap_or_default();
    let nodes = &ctx.backends.nodes;
    if let Err(failure) = nodes.open() {
        return Response::refuse(409, &failure.message());
    }
    match crate::account::delete_account(
        nodes.slot(),
        &ctx.home,
        confirm,
        itsanas_discover::now_unix(),
    ) {
        Ok(_) => {
            account_gone(ctx, "deleted");
            ok_json(object(&[("deleted", "true".to_owned())]))
        }
        Err(failure) => {
            nodes.release();
            Response::refuse(409, &failure.message())
        }
    }
}

pub(super) fn sign_out(ctx: &Context) -> Response {
    let service = (ctx.backends.service)();
    match sign::sign_out(&ctx.home, service.as_ref()) {
        Ok(done) => {
            ctx.backends.host.event("signedOut", "");
            ok_json(object(&[(
                "said",
                quote(&format!("{done}.\n{}", sign::SIGNED_OUT)),
            )]))
        }
        Err(error) => Response::refuse(409, &error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Copy this account to a USB key
// ---------------------------------------------------------------------------

/// Open the synced folder, or pin it where the file manager keeps favourites:
/// Quick access on Windows, the GTK bookmarks on Linux. macOS has no command
/// for the Finder sidebar, and says how by hand.
/// A second account on this computer is a second instance, in a home of its
/// own: never the home of the account already here, nor of any other one.
/// Found on Windows: an old account's home made every step "Already done",
/// and the page offered no way to set up a new account beside it.
pub(super) fn another_account(ctx: &Context, request: &Request) -> Response {
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
pub(super) fn start_another(name: &str) -> std::result::Result<(), String> {
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
