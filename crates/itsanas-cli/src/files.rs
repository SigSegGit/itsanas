//! The account's files: `ls`, `put`, `get`, `mv`, `versions`, `free`, `rm`,
//! `trash`, the synced folder and `scan`.

use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use itsanas_crypto::DeviceId;
use itsanas_net::session;

use crate::opening::{ask_daemon, daemon_holds, open};
use crate::refusals::{describe_left_out, name_devices};
use crate::syncing::{account_targets, connect_peer};
use crate::{
    config::format_size,
    error::{CliError, Result},
    node::Node,
};
use crate::{deleting, freeing, jobs, trash};

pub(crate) fn list(home: &Path) -> Result<()> {
    let output = home.join(jobs::JOB_OUTPUT);
    match open_or_hand(home, jobs::Kind::List, &output, None)? {
        Ok(node) => print!("{}", listing(&node)?),
        Err(_) => print!("{}", String::from_utf8_lossy(&taken_output(&output)?)),
    }
    Ok(())
}

/// What `itsanas ls` prints, here or in the daemon (`jobs.rs`).
pub(crate) fn listing(node: &Node) -> Result<String> {
    use std::fmt::Write as _;
    let home = &node.home;
    let mut out = String::new();

    // Everything this account has, not everything this machine downloaded. A
    // node that synced on a metered connection knows about files whose contents
    // it never fetched, and listing only what is local would tell somebody
    // their files were gone.
    let known = itsanas_store::catalogue(&node.store, &node.vault)?;
    // Said before the list and before "(no files)", which a chain left out
    // can make untrue.
    if let Some(left_out) = describe_left_out(&known) {
        let _ = writeln!(out, "note: {left_out}");
    }
    if !known.behind.is_empty() {
        let _ = writeln!(
            out,
            concat!(
                "note: {} machine(s) of this account wrote more than has reached this one{}: ",
                "their last files and deletes are not in this list yet (a host serving only ",
                "the start of their log, or the rest not pushed anywhere this machine reached)"
            ),
            known.behind.len(),
            name_devices(&known.behind, known.behind.len())
        );
    }

    if known.files.is_empty() {
        let _ = writeln!(out, "(no files)");
        return Ok(out);
    }

    let trashed = trash::list(home);
    // Freed by the person, not missing: `itsanas free --keep` brings it back.
    let freed = itsanas_store::freed::list(&trash::record_dir(home));
    let mut absent = 0usize;
    for entry in &known.files {
        let mark = if trashed.contains_key(&entry.path) {
            "in trash"
        } else if freed.contains(&entry.path) {
            "freed   "
        } else if entry.presence == itsanas_store::Presence::Absent {
            absent += 1;
            "not here"
        } else {
            "        "
        };
        let _ = writeln!(
            out,
            "{:>12}  {mark}  {}",
            format_size(entry.size),
            entry.path
        );
    }

    if absent > 0 {
        let _ = writeln!(out);
        // What is actually true, which is not the same as what this said in
        // either of its two earlier versions. On a device with a `keep` limit,
        // sync fetches what the limit and the order choose and lets go of the
        // rest, so "run sync" is not advice that brings a particular file down.
        // Opening one always works, because an explicit request beats a
        // background choice.
        let _ = writeln!(out, "{absent} file(s) are known and not on this device.");
        let _ = writeln!(
            out,
            "  `itsanas get <path>` fetches one, whatever the limit."
        );
        if node.config.keep_bytes.is_some() || !node.config.keep_only.is_empty() {
            let _ = writeln!(
                out,
                "  `itsanas sync` fetches whatever this device's limit chooses to hold."
            );
            let _ = writeln!(out, "  `itsanas keep` shows and changes that choice.");
        } else {
            let _ = writeln!(out, "  `itsanas sync` fetches them.");
        }
    }

    Ok(out)
}

/// The node, opened here; or, when a running daemon holds it, `Err` with
/// what the daemon said once it did `kind` (`jobs.rs`). Before 2026-11-17 a
/// file command refused to start while the daemon ran.
pub(crate) fn open_or_hand(
    home: &Path,
    kind: jobs::Kind,
    path: &Path,
    name: Option<&str>,
) -> Result<std::result::Result<Node, String>> {
    if daemon_holds(home) {
        return ask_daemon(home, kind, path, name).map(Err);
    }
    match open(home) {
        // A daemon that started between the two looks.
        Err(_) if daemon_holds(home) => ask_daemon(home, kind, path, name).map(Err),
        other => other.map(Ok),
    }
}

/// What the daemon wrote for the terminal, read and removed.
pub(crate) fn taken_output(output: &Path) -> Result<Vec<u8>> {
    let read = std::fs::read(output).map_err(|source| CliError::Io {
        path: output.to_owned(),
        source,
    });
    let _ = std::fs::remove_file(output);
    read
}

pub(crate) fn put(home: &Path, path: &str, source: &std::path::Path) -> Result<()> {
    let stdin = source == std::path::Path::new("-");
    if daemon_holds(home) {
        // The daemon cannot read this terminal: what it says waits in the home.
        let handed = if stdin {
            let input = home.join(jobs::JOB_INPUT);
            let mut buffer = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buffer)
                .map_err(|error| CliError::Io {
                    path: PathBuf::from("<stdin>"),
                    source: error,
                })?;
            std::fs::write(&input, buffer).map_err(|error| CliError::Io {
                path: input.clone(),
                source: error,
            })?;
            input
        } else {
            source.to_owned()
        };
        let said = ask_daemon(home, jobs::Kind::Put, &handed, Some(path));
        if stdin {
            let _ = std::fs::remove_file(&handed);
        }
        println!("{}", said?);
        return Ok(());
    }
    let node = open(home)?;
    println!("{}", put_into(&node, path, source)?);
    Ok(())
}

/// Store `source` as `path`; what `itsanas put` says.
pub(crate) fn put_into(node: &Node, path: &str, source: &std::path::Path) -> Result<String> {
    // What the account's other machines lend counts towards what it may hold.
    // A coordinator that cannot be reached leaves the figure it gave last time,
    // which is the right answer offline and not worth failing a write over.
    if node.config.coordinator.is_some()
        && let Err(error) = itsanas_node::coordinator::refresh_others_pledged(node)
    {
        eprintln!("itsanas: using the last known pledges of your other machines: {error}");
    }
    node.bound_writes()?;

    // A file on disk says how big it is, so it is refused before it is read;
    // stdin cannot, and is refused by the store as it streams past.
    if source != std::path::Path::new("-") {
        let incoming = std::fs::metadata(source)
            .map_err(|error| CliError::Io {
                path: source.to_owned(),
                source: error,
            })?
            .len();
        node.store
            .check_room(path, incoming)
            .map_err(|error| over_budget(node, error))?;
    }

    let content = if source == std::path::Path::new("-") {
        let mut buffer = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buffer)
            .map_err(|error| CliError::Io {
                path: PathBuf::from("<stdin>"),
                source: error,
            })?;
        buffer
    } else {
        std::fs::read(source).map_err(|error| CliError::Io {
            path: source.to_owned(),
            source: error,
        })?
    };

    let entry = node
        .store
        .write_file(path, &content)
        .map_err(|error| over_budget(node, error))?;
    node.store.flush_segment()?;

    Ok(format!(
        "stored {} as {path} ({} chunks)",
        format_size(entry.size),
        entry.chunks.len()
    ))
}

/// A refused write, in the words `itsanas keep` and `itsanas space` use: what
/// it would take, what that needs pledged, and the command that offers it.
pub(crate) fn over_budget(node: &Node, error: itsanas_store::StoreError) -> CliError {
    let itsanas_store::StoreError::OverBudget { path, total, .. } = error else {
        return error.into();
    };
    // What the account needs in all, less what the other machines already
    // lend, is what this one would have to offer.
    let account = Node::account_pledge(&node.home, &node.config);
    let others = account.saturating_sub(node.config.pledge_bytes);
    let needed = node.config.split.pledge_needed_for(total);
    let here = needed.saturating_sub(others);
    CliError::Usage(format!(
        concat!(
            "{} would bring this account to {}, which needs {} pledged across its ",
            "machines, and they offer {}. Nothing was stored. ",
            "`itsanas space --pledge {} --apply` here makes up the difference, ",
            "or make room first."
        ),
        path,
        format_size(total),
        itsanas_node::config::size_argument(needed),
        format_size(account),
        itsanas_node::config::size_argument(here),
    ))
}

/// Bring here the chunks of version `at` of `path`: from the account's
/// machines first, then from the hosts of other accounts the coordinator
/// names, which keep the account's chunks too (HANDOVER §8 15). Each chunk is
/// checked as it arrives, and one source can finish what another began.
pub(crate) fn host_targets(node: &Node) -> Vec<(String, Option<DeviceId>)> {
    itsanas_node::coordinator::hosts(node)
        .unwrap_or_default()
        .into_iter()
        .map(|(device, address)| (address, Some(device)))
        .collect()
}

pub(crate) fn fetch_version_here(node: &Node, path: &str, at: usize) -> bool {
    let from = |targets: &[(String, Option<DeviceId>)]| {
        targets.iter().any(|(target, pinned)| {
            connect_peer(target.as_str(), &node.device, node.store.owner(), *pinned).is_ok_and(
                |mut client| {
                    let _ = node.introduce(&mut client, itsanas_discover::now_unix());
                    session::fetch_version(&node.store, &node.vault, &mut client, path, at)
                        .unwrap_or(false)
                },
            )
        })
    };
    from(&account_targets(node, None)) || from(&host_targets(node))
}

/// Go and get a file this device knows about and has not downloaded.
///
/// Fetches exactly that file's chunks from the first peer that has them, rather
/// than syncing the account: opening one document on a phone must not pull
/// somebody's photo library.
pub(crate) fn fetch_absent(node: &Node, path: &str) -> Result<Vec<u8>> {
    let Some(chunks) = itsanas_store::chunks_for(&node.store, &node.vault, path)? else {
        return Err(CliError::Usage(format!("no such file: {path}")));
    };

    let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

    // The account's machines, not only `peer add` entries: a laptop away from
    // home with a storage budget answered "there is no peer to fetch it from"
    // while the coordinator listed its other machines and the address book
    // knew them, a parked one included (§8 0v).
    // The account's machines first, then the hosts of other accounts the
    // coordinator names, which keep its chunks too: asked only when no machine
    // of the account served the file.
    let mut targets = account_targets(node, None);
    let mine = targets.len();
    if mine == 0 {
        targets = host_targets(node);
    }
    if targets.is_empty() {
        return Err(CliError::Usage(format!(
            "{path} is in this account and not on this device, and no machine of this account is known to fetch it from. Try `itsanas peer add <host:port>` or `itsanas peer find <username>`."
        )));
    }

    let mut at = 0;
    while at < targets.len() {
        let (target, pinned) = targets[at].clone();
        at += 1;
        if at == mine {
            targets.extend(host_targets(node));
        }
        let Ok(mut client) =
            connect_peer(target.as_str(), &node.device, node.store.owner(), pinned)
        else {
            continue;
        };
        let _ = node.introduce(&mut client, itsanas_discover::now_unix());

        if session::fetch_only(&node.store, &node.vault, &mut client, &wanted).is_err() {
            continue;
        }

        if let Some(content) = node.store.read_file(path)? {
            println!("fetched {path} from {target}");
            return Ok(content);
        }
    }

    Err(CliError::Usage(format!(
        "{path} is in this account and no reachable peer would serve it. It is still listed; try again when one is up."
    )))
}

pub(crate) fn get(home: &Path, path: &str, destination: Option<&std::path::Path>) -> Result<()> {
    let output = home.join(jobs::JOB_OUTPUT);
    let node = match open_or_hand(
        home,
        jobs::Kind::Get,
        destination.unwrap_or(&output),
        Some(path),
    )? {
        Ok(node) => node,
        Err(said) => {
            if destination.is_some() {
                println!("{said}");
            } else {
                std::io::stdout()
                    .write_all(&taken_output(&output)?)
                    .map_err(|error| CliError::Io {
                        path: PathBuf::from("<stdout>"),
                        source: error,
                    })?;
            }
            return Ok(());
        }
    };

    if let Some(destination) = destination {
        println!("{}", get_into(&node, path, destination)?);
    } else {
        std::io::stdout()
            .write_all(&fetched(&node, path)?)
            .map_err(|error| CliError::Io {
                path: PathBuf::from("<stdout>"),
                source: error,
            })?;
    }
    Ok(())
}

/// Between the old name and the new in a rename handed to the daemon: a path
/// the store accepts holds no control character.
pub(crate) const TAB: char = '\u{9}';

pub(crate) fn rename(home: &Path, from: &str, to: &str) -> Result<()> {
    let both = format!("{from}{TAB}{to}");
    let said = match open_or_hand(home, jobs::Kind::Move, home, Some(&both))? {
        Ok(node) => rename_in(&node, from, to)?,
        Err(said) => said,
    };
    println!("{said}");
    Ok(())
}

pub(crate) fn versions(home: &Path, path: &str, restore: Option<usize>) -> Result<()> {
    let named = restore.map_or_else(|| path.to_owned(), |n| format!("{path}{TAB}{n}"));
    let said = match open_or_hand(home, jobs::Kind::Versions, home, Some(&named))? {
        Ok(node) => versions_in(&node, path, restore)?,
        Err(said) => said,
    };
    println!("{said}");
    Ok(())
}

/// The versions of `path`, numbered from 1, or (`restore`) version N made the
/// file again; what `itsanas versions` says.
pub(crate) fn versions_in(node: &Node, path: &str, restore: Option<usize>) -> Result<String> {
    use std::fmt::Write as _;
    let listed = itsanas_store::versions(&node.store, &node.vault, path)?;
    if listed.is_empty() {
        return Ok(format!("no version of {path} is known"));
    }
    if let Some(n) = restore {
        let Some(chosen) = n.checked_sub(1).and_then(|at| listed.get(at)) else {
            return Ok(format!(
                "{path} has {} versions; there is no {n}",
                listed.len()
            ));
        };
        if chosen.current {
            return Ok(format!(
                "version {n} of {path} is the one the account has now"
            ));
        }
        if !chosen.here && !fetch_version_here(node, path, n - 1) {
            return Ok(format!(
                "version {n} of {path} is on no machine of this account that answered; ask again when the one that wrote it is up"
            ));
        }
        if !itsanas_store::bring_back_version(&node.store, &node.vault, path, n - 1)? {
            return Ok(format!("version {n} of {path} could not be brought back"));
        }
        return Ok(format!("{path} is version {n} again, on every machine"));
    }
    let mine = node.store.device_id();
    let mut text = String::new();
    for (at, version) in listed.iter().enumerate() {
        let when = crate::control::describe_moment(version.modified_unix).unwrap_or_default();
        let by = if version.author == mine {
            "this machine".to_owned()
        } else {
            version.author.to_hex()[..8].to_owned()
        };
        let mut notes = Vec::new();
        if version.current {
            notes.push("current");
        }
        notes.push(if version.here { "here" } else { "not here" });
        let _ = writeln!(
            text,
            "{:>3}  {when}  {:>10}  {by}  {}",
            at + 1,
            format_size(version.size),
            notes.join(", ")
        );
    }
    let _ = write!(
        text,
        "`itsanas versions {path} --restore N` makes one the file again"
    );
    Ok(text)
}

/// Rename `from` to `to` on every machine; what `itsanas mv` says.
pub(crate) fn rename_in(node: &Node, from: &str, to: &str) -> Result<String> {
    use itsanas_store::Renamed;
    Ok(
        match itsanas_store::rename_known(&node.store, &node.vault, from, to)? {
            Renamed::Moved(1) => format!("{from} is now {to}, on every machine"),
            Renamed::Moved(count) => {
                format!("{count} files moved from {from}/ to {to}/, on every machine")
            }
            Renamed::NoSuchFile => format!("no such file or folder: {from}"),
            Renamed::Taken(name) => {
                format!("{name} is already a file of the account; nothing moved")
            }
            Renamed::InTrash(name) => format!(
                "{name} is in the account's trash; `itsanas trash restore {name}` puts it back, then it can move"
            ),
        },
    )
}

pub(crate) fn free(home: &Path, path: Option<&str>, keep: bool) -> Result<()> {
    if let Some(path) = path {
        let said = match open_or_hand(home, jobs::Kind::Free { keep }, home, Some(path))? {
            Ok(node) => free_in(&node, path, keep).map_err(CliError::Usage)?,
            Err(said) => said,
        };
        println!("{said}");
        return Ok(());
    }
    // The list is a file next to the store: read whether or not a daemon runs.
    for path in itsanas_store::freed::list(&Node::store_path(home)) {
        println!("{path}");
    }
    Ok(())
}

/// Free `path` from this machine, or (`keep`) bring it back; what
/// `itsanas free` says.
pub(crate) fn free_in(node: &Node, path: &str, keep: bool) -> std::result::Result<String, String> {
    if keep {
        freeing::keep(node, path)
    } else {
        freeing::free(node, path)
            .map(|said| format!("{said} `itsanas free --keep {path}` brings it back."))
    }
    .map_err(|failure| failure.message())
}

pub(crate) fn remove(home: &Path, path: &str, now: bool) -> Result<()> {
    let said = match open_or_hand(home, jobs::Kind::Remove { now }, home, Some(path))? {
        Ok(node) => remove_in(&node, path, now)?,
        Err(said) => said,
    };
    println!("{said}");
    Ok(())
}

/// Delete `path`, into the trash or (`now`) from every machine; what
/// `itsanas rm` says.
pub(crate) fn remove_in(node: &Node, path: &str, now: bool) -> Result<String> {
    use itsanas_store::trash::Deleted;
    let home = &node.home;
    if now {
        if !itsanas_store::remove_known(&node.store, &node.vault, path)? {
            return Ok(format!("no such file: {path}"));
        }
        node.store.flush_segment()?;
        trash::take_out(home, path).map_err(|source| itsanas_node::NodeError::Io {
            path: trash::record_dir(home),
            source,
        })?;
        return Ok(format!("deleted {path}"));
    }
    Ok(
        match itsanas_store::trash::delete(
            &node.store,
            &node.vault,
            path,
            itsanas_store::now_unix(),
        )? {
            Deleted::Unknown => format!("no such file: {path}"),
            Deleted::Everywhere => format!("deleted {path} from every machine"),
            Deleted::Trashed => format!(
                "{path} is in the account's trash; `itsanas trash restore {path}` brings it back on every machine"
            ),
            Deleted::Waiting => format!(
                "{path} waits here for `itsanas trash delete {path}` to delete it from every machine"
            ),
            Deleted::HereOnly => format!("{path} left this machine; the account keeps it"),
        },
    )
}

/// A file command the daemon was handed (`jobs.rs`), done on the node it
/// holds; what it says.
pub(crate) fn file_job(node: &Node, job: &jobs::Job) -> std::result::Result<String, String> {
    let name = || {
        job.name
            .as_deref()
            .ok_or_else(|| "a file command came with no file name".to_owned())
    };
    let done = match job.kind {
        jobs::Kind::List => listing(node).and_then(|text| {
            std::fs::write(&job.path, text).map_err(|source| CliError::Io {
                path: job.path.clone(),
                source,
            })
        }),
        jobs::Kind::Put => return put_into(node, name()?, &job.path).map_err(|e| e.to_string()),
        jobs::Kind::Get => return get_into(node, name()?, &job.path).map_err(|e| e.to_string()),
        jobs::Kind::Remove { now } => {
            return remove_in(node, name()?, now).map_err(|e| e.to_string());
        }
        jobs::Kind::Free { keep } => return free_in(node, name()?, keep),
        jobs::Kind::Versions => {
            let named = name()?;
            let (path, restore) = match named.split_once(TAB) {
                Some((path, n)) => (
                    path,
                    Some(
                        n.parse()
                            .map_err(|_| "a version number is a whole number".to_owned())?,
                    ),
                ),
                None => (named, None),
            };
            return versions_in(node, path, restore).map_err(|e| e.to_string());
        }
        jobs::Kind::Move => {
            let (from, to) = name()?
                .split_once(TAB)
                .ok_or_else(|| "a rename came without its new name".to_owned())?;
            return rename_in(node, from, to).map_err(|e| e.to_string());
        }
        _ => return deleting::trash_job(node, job),
    };
    done.map(|()| String::new())
        .map_err(|error| error.to_string())
}

/// The account's `path`, from here or fetched from a machine that has it.
pub(crate) fn fetched(node: &Node, path: &str) -> Result<Vec<u8>> {
    match node.store.read_file(path)? {
        Some(content) => Ok(content),
        // Not here does not mean not yours. A device with a storage budget, or
        // one that synced over a metered link, knows about files it has not
        // downloaded -- `itsanas ls` shows them. This used to answer "no such
        // file" for a file the account plainly had, which was both a lie and
        // the reason the budget setting had nothing behind it.
        None => fetch_absent(node, path),
    }
}

/// Write the account's `path` into `destination`; what `itsanas get` says.
pub(crate) fn get_into(node: &Node, path: &str, destination: &std::path::Path) -> Result<String> {
    let content = fetched(node, path)?;
    std::fs::write(destination, &content).map_err(|error| CliError::Io {
        path: destination.to_owned(),
        source: error,
    })?;
    Ok(format!(
        "wrote {} to {}",
        format_size(content.len() as u64),
        destination.display()
    ))
}

/// `itsanas trash`, its list when nothing is asked ([`deleting::command`]).
pub(crate) fn trash_command(home: &Path, action: Option<deleting::TrashAction>) -> Result<()> {
    deleting::command(home, action.unwrap_or(deleting::TrashAction::List))
        .map_err(|failure| CliError::Usage(failure.message()))
}

pub(crate) fn folder(home: &Path, path: Option<&Path>, confirm: bool) -> Result<()> {
    let mut node = open(home)?;

    let Some(path) = path else {
        let Some(configured) = node.config.folder.clone() else {
            println!("(no folder configured — `itsanas folder <path>`)");
            return Ok(());
        };
        println!("{}", configured.display());

        if confirm {
            // Only here, and only because somebody typed it: this is the one
            // path that writes deletions a pass refused to write on its own.
            let folder = itsanas_folder::Folder::open(&configured)?;
            let report = folder.reconcile_confirmed(&node.store, false)?;
            println!();
            println!("{}", report.summary());
            if report.removed_from_store.is_empty() {
                println!("  nothing was waiting to be deleted.");
            } else {
                println!(
                    "  {} file(s) removed from the account, as confirmed.",
                    report.removed_from_store.len()
                );
            }
        }
        return Ok(());
    };

    let (absolute, first_pass) = set_folder(&mut node, path)?;
    println!("synced folder set to {}", absolute.display());
    println!("{first_pass}");
    Ok(())
}

/// Keep `path` in step with this node, and say what the first pass did.
pub(crate) fn set_folder(node: &mut Node, path: &Path) -> Result<(PathBuf, String)> {
    // Store it absolute. A relative path would mean something different
    // depending on where the daemon happened to be started from, which is the
    // sort of thing that quietly syncs the wrong directory.
    let absolute = std::path::absolute(path).map_err(|error| CliError::Io {
        path: path.to_owned(),
        source: error,
    })?;

    let folder = itsanas_folder::Folder::open(&absolute)?;

    node.config.folder = Some(absolute.clone());
    node.save_config()?;

    // Show what the first pass would do rather than doing it silently. Pointing
    // this at an existing directory full of files is a big action, and the user
    // should see the size of it.
    let report = folder.reconcile(&node.store, false)?;
    let said = if report.changed_anything() {
        format!("first pass: {}", report.summary())
    } else {
        "the folder and the store already agree.".to_owned()
    };
    Ok((absolute, said))
}

pub(crate) fn scan(home: &Path, deep: bool) -> Result<()> {
    let node = open(home)?;

    let Some(path) = node.config.folder.clone() else {
        return Err(CliError::Usage(
            "no synced folder configured. Try `itsanas folder <path>`.".to_owned(),
        ));
    };

    let folder = itsanas_folder::Folder::open(&path)?;
    let report = folder.reconcile(&node.store, deep)?;

    println!("{}", report.summary());
    // Every file, unbounded: somebody typed `scan` to see them.
    for line in report.lines(None) {
        println!("{line}");
    }
    for (path, why) in &report.failed {
        eprintln!("  err  {path}: {why}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    /// `itsanas mv` while the daemon holds the store: the job carries both
    /// names, and the daemon's node renames. Sabotage: split the job's name on
    /// a space (a name with a space moves nothing, or to the wrong place).
    #[test]
    fn a_rename_handed_to_the_daemon_moves_the_file_with_spaces_in_its_names() {
        let base = tempfile::tempdir().expect("temp dir");
        let (node, _) =
            itsanas_node::Node::create(&base.path().join("desk"), "a passphrase", "camille")
                .expect("create");
        node.store
            .write_file("my notes.txt", b"kept as they are")
            .expect("write");
        node.store.flush_segment().expect("flush");
        let job = crate::jobs::Job {
            id: 1,
            kind: crate::jobs::Kind::Move,
            path: base.path().to_owned(),
            name: Some(format!("my notes.txt{}old notes/my notes.txt", super::TAB)),
        };
        let said = super::file_job(&node, &job).expect("the daemon's rename");
        assert!(said.contains("is now"), "{said}");
        assert_eq!(
            node.store
                .read_file("old notes/my notes.txt")
                .expect("read"),
            Some(b"kept as they are".to_vec()),
            "the daemon's rename did not move the file: {said}"
        );
        assert_eq!(node.store.stat("my notes.txt").expect("stat"), None);
    }

    /// `itsanas versions` while the daemon holds the store: the list numbers the
    /// versions from 1, newest first, and `--restore 2` brings back the one
    /// before. Sabotage: count from 0 (the current version is "restored").
    #[test]
    fn versions_are_numbered_as_listed_and_the_second_comes_back() {
        let base = tempfile::tempdir().expect("temp dir");
        let (node, _) =
            itsanas_node::Node::create(&base.path().join("desk"), "a passphrase", "camille")
                .expect("create");
        node.store.write_file("plan.txt", b"before").expect("write");
        node.store.flush_segment().expect("flush");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        node.store.write_file("plan.txt", b"after").expect("write");
        node.store.flush_segment().expect("flush");
        let job = |name: String| crate::jobs::Job {
            id: 1,
            kind: crate::jobs::Kind::Versions,
            path: base.path().to_owned(),
            name: Some(name),
        };

        let listed = super::file_job(&node, &job("plan.txt".to_owned())).expect("list");
        let first = listed.lines().next().unwrap_or_default();
        assert!(
            first.trim_start().starts_with("1 ") && first.contains("current"),
            "the newest version is not number 1, marked current:\n{listed}"
        );
        let said =
            super::file_job(&node, &job(format!("plan.txt{}2", super::TAB))).expect("restore");
        assert_eq!(
            node.store.read_file("plan.txt").expect("read"),
            Some(b"before".to_vec()),
            "`--restore 2` did not bring back the version listed second: {said}"
        );
    }
}
