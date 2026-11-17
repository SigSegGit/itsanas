//! What a delete does, from a terminal and from the page (HANDOVER §8 13): the
//! trash of this machine and of the account, and the rules each machine follows.
//!
//! One module for the command and the page, so the two cannot disagree on what
//! a rule means. The rules themselves are `itsanas_store::deletion`.

use clap::Subcommand;
use itsanas_node::Node;
use itsanas_store::deletion::{self, Mode, Receive, Rules};
use serde_json::json;

use crate::failure::Failure;

/// `itsanas trash ...`.
#[derive(Subcommand, Debug)]
pub enum TrashAction {
    /// List what is in the trash and when each file is really deleted.
    List,
    /// Bring a file back; in the account's trash, on every machine.
    Restore { path: String },
    /// Delete files of the trash for good, from every machine.
    Delete { paths: Vec<String> },
    /// Delete everything in the trash for good.
    Empty,
    /// Set how many days a deleted file waits here; 0 turns the trash off.
    Days { days: u64 },
    /// Show the rules of this machine, of the account and of every machine.
    Rules,
    /// What a delete made here does: trash, direct or local.
    Mode {
        mode: String,
        /// Set it for the whole account, every machine following it.
        #[arg(long)]
        all: bool,
    },
    /// What a direct delete from another machine does here: protect or apply.
    Receive {
        rule: String,
        #[arg(long)]
        all: bool,
    },
    /// Whether a definitive delete asks who you are first: on or off.
    Ask {
        on: String,
        #[arg(long)]
        all: bool,
    },
    /// Whether a delete asks "are you sure" first: on or off.
    Confirm {
        on: String,
        #[arg(long)]
        all: bool,
    },
    /// Follow the account's rules again, this machine's own taken away.
    Follow,
}

fn usage(text: impl Into<String>) -> Failure {
    Failure::Usage(text.into())
}

fn switch(word: &str) -> Result<bool, Failure> {
    match word {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(usage(format!("`{word}`: say on or off"))),
    }
}

/// The rules of this machine, of the account and of every machine, as the page
/// shows them.
///
/// # Errors
///
/// When the store cannot be read.
pub fn rules_json(node: &Node) -> Result<String, Failure> {
    let here = deletion::rules_here(&node.store, &node.vault)?;
    let (account, machines) = deletion::all_rules(&node.store, &node.vault)?;
    let me = node.store.device_id().to_hex();
    let rows: Vec<_> = machines
        .iter()
        .map(|(device, rules)| {
            json!({ "device": device, "me": *device == me, "rules": rules_value(rules) })
        })
        .collect();
    Ok(json!({
        "here": rules_value(&here),
        "own": machines.contains_key(&me),
        "account": rules_value(&account),
        "machines": rows,
    })
    .to_string())
}

fn rules_value(rules: &Rules) -> serde_json::Value {
    json!({
        "mode": rules.mode.word(),
        "receive": rules.receive.word(),
        "days": rules.days,
        "ask_auth": rules.ask_auth,
        "confirm": rules.confirm,
    })
}

/// Read rules sent by the page: every field, or the request is refused.
///
/// # Errors
///
/// When a field is missing or not understood.
pub fn rules_from(value: &serde_json::Value) -> Result<Rules, Failure> {
    let word = |key: &str| {
        value[key]
            .as_str()
            .ok_or_else(|| usage(format!("`{key}` is missing")))
    };
    let flag = |key: &str| {
        value[key]
            .as_bool()
            .ok_or_else(|| usage(format!("`{key}` is missing")))
    };
    Ok(Rules {
        mode: Mode::from_word(word("mode")?)
            .ok_or_else(|| usage("mode: trash, direct or local"))?,
        receive: Receive::from_word(word("receive")?)
            .ok_or_else(|| usage("receive: protect or apply"))?,
        days: value["days"]
            .as_u64()
            .filter(|days| (1..=itsanas_store::trash::MAX_DAYS).contains(days))
            .ok_or_else(|| usage("days: from 1 to 9999"))?,
        ask_auth: flag("ask_auth")?,
        confirm: flag("confirm")?,
    })
}

/// Who a change of rules is set for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// This machine only.
    Here,
    /// The account's default, every machine following it.
    All,
    /// This machine follows the account again.
    Follow,
}

/// Whether a change asks who the person is first: it keeps less than what
/// applies now, or turns the question off ([`Rules::loosened_by`]).
///
/// # Errors
///
/// When the store cannot be read.
pub fn needs_identity(node: &Node, scope: Scope, next: &Rules) -> Result<bool, Failure> {
    let now = deletion::rules_here(&node.store, &node.vault)?;
    let next = match scope {
        Scope::Here | Scope::All => *next,
        Scope::Follow => deletion::account_rules(&node.store, &node.vault)?,
    };
    let (_, machines) = deletion::all_rules(&node.store, &node.vault)?;
    // Applied to all, every machine's own rules go: one that kept more loosens.
    let others_loosen = scope == Scope::All && machines.values().any(|own| own.loosened_by(&next));
    Ok(now.ask_auth && (now.loosened_by(&next) || others_loosen))
}

/// Set the rules; says what changed.
///
/// # Errors
///
/// When the store cannot be written.
pub fn set_rules(node: &Node, scope: Scope, rules: &Rules) -> Result<String, Failure> {
    match scope {
        Scope::Here => {
            deletion::set_rules_here(&node.store, &node.vault, Some(rules))?;
            Ok("This machine's rules are set.".to_owned())
        }
        Scope::All => {
            deletion::apply_to_all(&node.store, &node.vault, rules)?;
            Ok("Every machine of the account follows these rules on its next round.".to_owned())
        }
        Scope::Follow => {
            deletion::set_rules_here(&node.store, &node.vault, None)?;
            Ok("This machine follows the account's rules.".to_owned())
        }
    }
}

/// Delete files of the trash for good, the shared ones from every machine. A
/// path that is not in the trash is refused, all of them before any is
/// deleted: the page and the command delete out of the trash, never around it.
///
/// # Errors
///
/// When a path is not in the trash, or the store cannot be written.
pub fn purge(node: &Node, paths: &[String]) -> Result<String, Failure> {
    let inside = everything(node)?;
    if let Some(stray) = paths.iter().find(|path| !inside.contains(path)) {
        return Err(usage(format!("{stray} is not in the trash")));
    }
    if paths.is_empty() {
        return Ok("The trash is empty.".to_owned());
    }
    for path in paths {
        crate::phone::trash_delete(node, path)?;
    }
    Ok(match paths.len() {
        1 => format!("{} is deleted for good.", paths[0]),
        count => format!("{count} files are deleted for good."),
    })
}

/// Every path in the trash, of every kind.
///
/// # Errors
///
/// When the store cannot be read.
pub fn everything(node: &Node) -> Result<Vec<String>, Failure> {
    let hidden = crate::phone::purged_trash(node)?;
    let mut paths: Vec<String> = hidden.into_keys().collect();
    paths.extend(deletion::protected(node.store.root()).into_keys());
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn rules_text(node: &Node) -> Result<String, Failure> {
    use std::fmt::Write as _;
    let here = deletion::rules_here(&node.store, &node.vault)?;
    let (account, machines) = deletion::all_rules(&node.store, &node.vault)?;
    let me = node.store.device_id().to_hex();
    let show = |rules: &Rules| {
        format!(
            "{} · {} · {} days · identity {} · confirm {}",
            rules.mode.word(),
            rules.receive.word(),
            rules.days,
            if rules.ask_auth { "asked" } else { "not asked" },
            if rules.confirm { "on" } else { "off" }
        )
    };
    let mut out = String::new();
    let _ = writeln!(out, "this machine:  {}", show(&here));
    let _ = writeln!(out, "the account:   {}", show(&account));
    for (device, rules) in &machines {
        let mark = if *device == me { " (this one)" } else { "" };
        let _ = writeln!(out, "  {}{mark}: {}", &device[..12], show(rules));
    }
    Ok(out)
}

/// What `itsanas trash` lists, here or in the daemon.
fn list_text(node: &Node) -> Result<String, Failure> {
    use std::fmt::Write as _;
    let hidden = crate::phone::purged_trash(node)?;
    let rows = crate::phone::trash_rows(node, &hidden)?;
    let now = itsanas_store::now_unix();
    let empty = Vec::new();
    let listed = rows["trashed"].as_array().unwrap_or(&empty);
    let mut out = String::new();
    if listed.is_empty() {
        let _ = writeln!(out, "the trash is empty");
    }
    for row in listed {
        let due = row["due"].as_u64().unwrap_or(0);
        let path = row["path"].as_str().unwrap_or_default();
        let kind = row["kind"].as_str().unwrap_or_default();
        if kind == "waiting" {
            let _ = writeln!(
                out,
                "   to confirm  {path}  (`itsanas trash delete` deletes it everywhere)"
            );
        } else {
            let days = due.saturating_sub(now).div_ceil(itsanas_store::trash::DAY);
            let _ = writeln!(out, "{days:>4} days  {kind:<9}  {path}");
        }
    }
    Ok(out)
}

/// `itsanas trash` as words, the way the terminal typed them: what travels
/// to a running daemon in a job's name, one word per tab.
fn words(action: &TrashAction) -> Vec<String> {
    let with_all = |word: &str, value: &str, all: bool| {
        let mut said = vec![word.to_owned(), value.to_owned()];
        if all {
            said.push("--all".to_owned());
        }
        said
    };
    match action {
        TrashAction::List => vec!["list".to_owned()],
        TrashAction::Rules => vec!["rules".to_owned()],
        TrashAction::Empty => vec!["empty".to_owned()],
        TrashAction::Follow => vec!["follow".to_owned()],
        TrashAction::Restore { path } => vec!["restore".to_owned(), "--".to_owned(), path.clone()],
        TrashAction::Delete { paths } => ["delete", "--"]
            .into_iter()
            .map(str::to_owned)
            .chain(paths.iter().cloned())
            .collect(),
        TrashAction::Days { days } => vec!["days".to_owned(), days.to_string()],
        TrashAction::Mode { mode, all } => with_all("mode", mode, *all),
        TrashAction::Receive { rule, all } => with_all("receive", rule, *all),
        TrashAction::Ask { on, all } => with_all("ask", on, *all),
        TrashAction::Confirm { on, all } => with_all("confirm", on, *all),
    }
}

/// The trash's words read back, as the terminal would have.
#[derive(clap::Parser)]
#[command(name = "trash")]
struct Words {
    #[command(subcommand)]
    action: TrashAction,
}

/// `itsanas trash` handed to a running daemon that holds the store
/// (`jobs.rs`); what it printed.
fn handed(home: &std::path::Path, action: &TrashAction) -> Result<String, Failure> {
    let output = home.join(crate::jobs::JOB_OUTPUT);
    // Tabs between words: a name in the account holds none (the folder
    // refuses them), and the job's name is one line.
    crate::ask_daemon(
        home,
        crate::jobs::Kind::Trash,
        &output,
        Some(&words(action).join("\t")),
    )
    .map_err(|error| usage(error.to_string()))?;
    let text = crate::taken_output(&output).map_err(|error| usage(error.to_string()))?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

/// A trash command the daemon was handed, done on the node it holds; what
/// it prints goes into the job's path.
///
/// # Errors
///
/// What the command says when it fails.
pub(crate) fn trash_job(node: &Node, job: &crate::jobs::Job) -> Result<String, String> {
    use clap::Parser as _;
    let words = job.name.as_deref().unwrap_or_default().split('\t');
    let action = Words::try_parse_from(std::iter::once("trash").chain(words))
        .map_err(|error| error.to_string())?
        .action;
    let text = run_on(node, action).map_err(|failure| failure.message())?;
    std::fs::write(&job.path, text)
        .map(|()| String::new())
        .map_err(|error| format!("could not write {}: {error}", job.path.display()))
}

fn changed(node: &Node, all: bool, change: impl FnOnce(&mut Rules)) -> Result<String, Failure> {
    let mut rules = if all {
        deletion::account_rules(&node.store, &node.vault)?
    } else {
        deletion::rules_here(&node.store, &node.vault)?
    };
    change(&mut rules);
    let scope = if all { Scope::All } else { Scope::Here };
    Ok(format!("{}\n", set_rules(node, scope, &rules)?))
}

/// `itsanas trash ...`. The terminal opened the node with the passphrase, so
/// who is there is already proved; a running daemon that holds the store
/// does it instead (2026-11-17), having been opened with it.
///
/// # Errors
///
/// When the node cannot be opened, or a value is not understood.
pub fn command(home: &std::path::Path, action: TrashAction) -> Result<(), Failure> {
    if crate::daemon_holds(home) {
        print!("{}", handed(home, &action)?);
        return Ok(());
    }
    if let TrashAction::Days { days: 0 } = action {
        print!("{}", trash_off(home)?);
        return Ok(());
    }
    let node = crate::open(home).map_err(|error| usage(error.to_string()))?;
    print!("{}", run_on(&node, action)?);
    Ok(())
}

/// The trash off, as before the rules: a delete is written at once.
fn trash_off(home: &std::path::Path) -> Result<String, Failure> {
    itsanas_store::trash::set_days(&crate::trash::record_dir(home), 0)
        .map_err(|error| crate::phone::trash_failed(&error))?;
    Ok("the trash is off here: a delete is written at once\n".to_owned())
}

/// What `itsanas trash ...` does on an open node, and what it prints.
fn run_on(node: &Node, action: TrashAction) -> Result<String, Failure> {
    let home = &node.home;
    Ok(match action {
        TrashAction::List => list_text(node)?,
        TrashAction::Rules => rules_text(node)?,
        TrashAction::Restore { path } => {
            crate::phone::trash_restore_in(node, &path)?;
            format!("{path} is back\n")
        }
        TrashAction::Delete { paths } => format!("{}\n", purge(node, &paths)?),
        TrashAction::Empty => format!("{}\n", purge(node, &everything(node)?)?),
        TrashAction::Days { days: 0 } => trash_off(home)?,
        TrashAction::Days { days } => {
            itsanas_store::trash::set_days(&crate::trash::record_dir(home), days)
                .map_err(|error| crate::phone::trash_failed(&error))?;
            changed(node, false, |rules| {
                rules.days = days.min(itsanas_store::trash::MAX_DAYS);
            })?
        }
        TrashAction::Mode { mode, all } => {
            let mode = Mode::from_word(&mode).ok_or_else(|| usage("say trash, direct or local"))?;
            changed(node, all, |rules| rules.mode = mode)?
        }
        TrashAction::Receive { rule, all } => {
            let rule = Receive::from_word(&rule).ok_or_else(|| usage("say protect or apply"))?;
            changed(node, all, |rules| rules.receive = rule)?
        }
        TrashAction::Ask { on, all } => {
            let on = switch(&on)?;
            changed(node, all, |rules| rules.ask_auth = on)?
        }
        TrashAction::Confirm { on, all } => {
            let on = switch(&on)?;
            changed(node, all, |rules| rules.confirm = on)?
        }
        TrashAction::Follow => format!("{}\n", set_rules(node, Scope::Follow, &Rules::default())?),
    })
}
