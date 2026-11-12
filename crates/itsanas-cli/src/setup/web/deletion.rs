//! The page's Trash section (HANDOVER §8 13): the list, restore and delete for
//! good, and the rules a delete follows, here or for the whole account.
//!
//! What keeps less than what applies now (a delete for good, a rule that keeps
//! less, a question turned off) runs as a job: the person proves who they are
//! in the system's own window first ([`crate::setup::keys::prove_identity`]),
//! one proof for the whole selection, and the page follows it through
//! `/api/state`. Everything else is done at once.

use std::sync::Arc;

use itsanas_store::deletion::Rules;

use super::{
    Context, Request, Response, json::field, json::parse_form, node_call, start_boxed, with_node,
};
use crate::{
    deleting::{self, Scope},
    failure::Failure,
};

/// The trash and this machine's rules, for the page.
fn listed(node: &itsanas_node::Node, said: &str) -> Result<String, Failure> {
    let hidden = crate::phone::purged_trash(node)?;
    let mut answer = crate::phone::trash_rows(node, &hidden)?;
    answer["said"] = serde_json::Value::String(said.to_owned());
    Ok(answer.to_string())
}

/// `GET /api/trash`.
pub(super) fn trash_list(ctx: &Context) -> Response {
    node_call(ctx, |node| listed(node, ""))
}

/// `POST /api/trash`: `restore` a path, `delete` paths (one a line) or `empty`.
pub(super) fn trash_change(ctx: &Arc<Context>, request: &Request) -> Response {
    let form = match parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let paths: Vec<String> = field(&form, "paths")
        .unwrap_or_default()
        .lines()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    match field(&form, "action") {
        Some("restore") => node_call(ctx, |node| {
            for path in &paths {
                crate::phone::trash_restore_in(node, path)?;
            }
            listed(node, "Restored on every machine of the account.")
        }),
        Some(action @ ("delete" | "empty")) => {
            let empty = action == "empty";
            if !empty && paths.is_empty() {
                return Response::refuse(400, "no file was named");
            }
            let ask = with_node(ctx, |node| {
                Ok(
                    itsanas_store::deletion::rules_here(&node.store, &node.vault)?
                        .ask_auth
                        .to_string(),
                )
            });
            match ask.as_deref() {
                Ok("true") => start_boxed(
                    ctx,
                    "purge",
                    "who you are, to delete for good",
                    Box::new(move |worker, prompt, _| {
                        crate::setup::keys::prove_identity(
                            &worker.home,
                            prompt,
                            "to delete files for good",
                        )?;
                        with_node(worker, |node| {
                            let named = if empty {
                                deleting::everything(node)?
                            } else {
                                paths
                            };
                            deleting::purge(node, &named)
                        })
                        .map_err(|failure| crate::error::CliError::Usage(failure.message()))
                    }),
                ),
                Ok(_) => node_call(ctx, |node| {
                    let named = if empty {
                        deleting::everything(node)?
                    } else {
                        paths
                    };
                    let said = deleting::purge(node, &named)?;
                    listed(node, &said)
                }),
                Err(failure) => Response::refuse(409, &failure.message()),
            }
        }
        _ => Response::refuse(400, "the action is restore, delete or empty"),
    }
}

/// `GET /api/deletion`: the rules of this machine, of the account and of every
/// machine.
pub(super) fn rules(ctx: &Context) -> Response {
    node_call(ctx, |node| deleting::rules_json(node))
}

/// `POST /api/deletion`: `scope` here, all or follow, and every rule.
pub(super) fn rules_change(ctx: &Arc<Context>, request: &Request) -> Response {
    let form = match parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let scope = match field(&form, "scope") {
        Some("here") => Scope::Here,
        Some("all") => Scope::All,
        Some("follow") => Scope::Follow,
        _ => return Response::refuse(400, "the scope is here, all or follow"),
    };
    let value = serde_json::json!({
        "mode": field(&form, "mode").unwrap_or("trash"),
        "receive": field(&form, "receive").unwrap_or("protect"),
        "days": field(&form, "days").and_then(|days| days.parse::<u64>().ok()),
        "ask_auth": field(&form, "ask_auth") == Some("true"),
        "confirm": field(&form, "confirm") == Some("true"),
    });
    let rules: Rules = match deleting::rules_from(&value) {
        Ok(rules) => rules,
        Err(failure) => return Response::refuse(400, &failure.message()),
    };
    let needs = with_node(ctx, |node| {
        Ok(deleting::needs_identity(node, scope, &rules)?.to_string())
    });
    match needs.as_deref() {
        Ok("true") => start_boxed(
            ctx,
            "rules",
            "who you are, to keep less of what is deleted",
            Box::new(move |worker, prompt, _| {
                crate::setup::keys::prove_identity(
                    &worker.home,
                    prompt,
                    "to change what a delete keeps",
                )?;
                with_node(worker, |node| deleting::set_rules(node, scope, &rules))
                    .map_err(|failure| crate::error::CliError::Usage(failure.message()))
            }),
        ),
        Ok(_) => node_call(ctx, |node| {
            deleting::set_rules(node, scope, &rules)?;
            deleting::rules_json(node)
        }),
        Err(failure) => Response::refuse(409, &failure.message()),
    }
}
