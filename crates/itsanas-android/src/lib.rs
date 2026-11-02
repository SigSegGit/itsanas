//! The JNI boundary: everything Kotlin is allowed to ask the core.
//!
//! # Why this crate is shaped the way it is
//!
//! **Coarse, not chatty.** Every call crosses a language boundary, allocates
//! Java strings and can throw; a per-field getter API would make listing a
//! thousand files a thousand crossings. So the calls are whole operations —
//! "list the account", "run a round", "fetch this file" — and they answer with
//! one JSON string. The cost of parsing JSON on the Kotlin side is nothing
//! beside the cost of the operations themselves, all of which touch a disk or a
//! socket.
//!
//! **Errors are Java exceptions, not sentinel values.** A shell that has to
//! remember to check a return code will eventually forget, and the failure it
//! forgets is "the passphrase was wrong". Every entry point either returns its
//! answer or throws, and the message is the same sentence the command line
//! prints for the same fault, unless that sentence names a command the phone
//! does not have (`Failure::message`).
//!
//! **One node, behind a lock.** The store permits a single writer, which is a
//! property of the storage engine rather than a decision made here. The
//! application opens the node once and every call takes the same lock, so a
//! background round and a tap on a file cannot both be inside the store.
//!
//! **Only what no page can do.** Creating, restoring and leaving an account,
//! the words, the passphrase, the devices, the space figures and the limits are
//! the library's (`itsanas-cli`: the setup engine and Settings, served as a page
//! to the app's web view), the same for every shell. What is left here is what a
//! native screen alone needs: the open node and its files (`exists`, `open`,
//! `close`, `status`, `list`, `get`, `put`, `remove`, `sync`), the sync policy
//! (`plan`), the application's own update check, and the passphrase rule. The
//! library's `shell` module lends this crate the machines to dial (`targets`)
//! and the failure sentences.
//!
//! # The only unsafe in the project
//!
//! A JVM calls `extern "system"` symbols by name, and Rust treats
//! `#[unsafe(no_mangle)]` as unsafe because a duplicate exported symbol is
//! undefined behaviour at link time. The workspace forbids unsafe code and this
//! crate is the single exception.
//!
//! The exception is **narrow and checked, not asserted**:
//! `scripts/check-unsafe.py` fails if any crate contains an `unsafe` block or
//! an `unsafe fn`, and if any crate other than this one relaxes the lint. So
//! the claim "the only unsafe in the project is the export attribute" is a
//! thing the build verifies rather than a sentence in a comment — which is what
//! the sentence would be worth otherwise.
//!
//! Everything past the first line of each entry point is ordinary safe Rust,
//! calling the same code the command line calls.
#![allow(unsafe_code)]

mod host;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use itsanas_cli::shell::{
    Answer, Backends, Failure, Held, Mode, NO_MACHINE_YET, Network, SecretPrompt, Server, Stop,
    check_words, describe, run_round, targets,
};
use itsanas_node::Node;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jstring};

/// The node this application has open, if any.
///
/// A process holds at most one. Android will happily start a second activity
/// against the same files, and the store's single-writer rule would then
/// surface as "already open in another process" — from *this* process, which
/// reads as a bug in the storage engine rather than as what it is.
static NODE: Mutex<Option<Node>> = Mutex::new(None);

// The store and the network answer with their own error types, and every one of
// them already reads as a sentence. Converting through `NodeError` keeps the
// words a person sees identical to the ones the command line prints for the
// same fault, which is what makes it possible to help somebody over a phone
// call. The exceptions are in `Failure::message`.

/// Run `body` and give the JVM either its answer or an exception.
///
/// Written once so that no entry point can forget it. A Rust panic crossing
/// into the JVM is undefined behaviour, so the body is caught: a panic here
/// would otherwise take the whole application down with a stack trace nobody
/// can read.
fn answer(env: &mut JNIEnv, body: impl FnOnce() -> Answer) -> jstring {
    // `AssertUnwindSafe` because the only state shared across the boundary is
    // the node, and it is behind a `Mutex` whose poisoning is detected: a panic
    // holding that lock makes every later call answer "the node lock was
    // poisoned by a panic" instead of reading whatever the panic left behind.
    // That is the property `UnwindSafe` exists to ask about, and it is provided
    // by the lock rather than by the types inside it -- which is why the
    // compiler cannot see it.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));

    let text = match outcome {
        Ok(Ok(text)) => text,
        Ok(Err(failure)) => {
            throw(env, &failure.message());
            return std::ptr::null_mut();
        }
        Err(_) => {
            throw(env, "the core panicked; see logcat for the message");
            return std::ptr::null_mut();
        }
    };

    if let Ok(string) = env.new_string(text) {
        string.into_raw()
    } else {
        throw(env, "could not allocate the answer");
        std::ptr::null_mut()
    }
}

fn throw(env: &mut JNIEnv, message: &str) {
    // If throwing itself fails there is nothing left to do: the JVM is in a
    // state this code cannot repair, and an ignored result is honest about it.
    let _ = env.throw_new("fr/ngas/itsanas/NativeException", message);
}

/// Read a Java string, or fail with a sentence naming the argument.
fn text(env: &mut JNIEnv, value: &JString<'_>, what: &str) -> Result<String, Failure> {
    env.get_string(value)
        .map(Into::into)
        .map_err(|_| Failure::Usage(format!("{what} was not a string")))
}

fn home_of(env: &mut JNIEnv, home: &JString<'_>) -> Result<PathBuf, Failure> {
    Ok(PathBuf::from(text(env, home, "the home directory")?))
}

/// Do something with the open node.
fn with_node<T>(body: impl FnOnce(&Node) -> Result<T, Failure>) -> Result<T, Failure> {
    let guard = NODE
        .lock()
        .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
    let node = guard.as_ref().ok_or(Failure::Closed)?;
    body(node)
}

// ---------------------------------------------------------------- lifecycle

/// Whether a node already exists at `home`.
///
/// The question the first screen asks: create an account, or unlock the one
/// that is here.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_exists(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
) -> jboolean {
    let Ok(path) = env.get_string(&home).map(String::from) else {
        return u8::from(false);
    };
    u8::from(Node::exists(Path::new(&path)))
}

/// The rule for a new passphrase, for the screens to state and check as it is
/// typed: `{"min":6,"max":128}`. The core refuses outside it whatever they do.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_passphraseRule(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || Ok(passphrase_rule()))
}

fn passphrase_rule() -> String {
    serde_json::json!({
        "min": itsanas_node::node::MIN_PASSPHRASE_CHARS,
        "max": itsanas_node::node::MAX_PASSPHRASE_CHARS,
    })
    .to_string()
}

/// Debug builds and instrumented tests only: every node this process creates,
/// restores or opens is pinned to this coordinator instead of the official one.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_useCoordinatorForTests(
    mut env: JNIEnv,
    _class: JClass,
    address: JString,
    device: JString,
) -> jstring {
    let address = text(&mut env, &address, "the coordinator's address");
    let device = text(&mut env, &device, "the coordinator's device");
    answer(&mut env, move || {
        use_coordinator(&TEST_NETWORK, &address?, &device?, cfg!(debug_assertions))
    })
}

/// Unlock the node already on this device.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_open(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    passphrase: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let passphrase = text(&mut env, &passphrase, "the passphrase");

    answer(&mut env, move || {
        open_into(&NODE, &home?, &passphrase?, &network()?)
    })
}

/// [`Java_fr_ngas_itsanas_Native_open`] without the JNI, on the slot given.
///
/// A node already open for `home` in this process is answered as it is, not
/// opened again. The sync service runs in the foreground and keeps the
/// process -- and the open node -- alive after the screen is closed; opening
/// the store a second time is refused by the database ("already open"), and
/// on 2026-10-31 that left the owner's phone stuck on Unlock with nothing
/// usable. The passphrase is still checked against the keystore: the screen
/// asking for it is what stands between a person and the account.
fn open_into(
    slot: &Mutex<Option<Node>>,
    home: &Path,
    passphrase: &str,
    network: &Network,
) -> Answer {
    let mut guard = slot
        .lock()
        .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
    if let Some(node) = guard.as_ref().filter(|node| node.home == home) {
        itsanas_node::node::Identity::secrets(home, passphrase)?;
        return Ok(describe(node));
    }
    // Another account's node, if any, is closed before this one opens.
    *guard = None;
    let node = network.pin(Node::open(home, passphrase)?)?;
    let answer = describe(&node);
    *guard = Some(node);
    Ok(answer)
}

/// Close the node, releasing the store for another process.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_close(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        *guard = None;
        Ok("{}".to_owned())
    })
}

/// Set by an instrumented test, and nothing else: see [`use_coordinator`].
static TEST_NETWORK: Mutex<Option<Network>> = Mutex::new(None);

/// The coordinator this process's nodes belong to: the official one, unless an
/// instrumented test said otherwise.
fn network() -> Result<Network, Failure> {
    let test = TEST_NETWORK
        .lock()
        .map_err(|_| Failure::Usage("the network lock was poisoned by a panic".to_owned()))?;
    Ok(test.clone().unwrap_or_else(Network::official))
}

/// [`Java_fr_ngas_itsanas_Native_useCoordinatorForTests`] without the JNI, with
/// the slot and the kind of build given.
///
/// Only a debug build takes it: the reason a phone cannot name its coordinator
/// (see [`Network::pin`]) is exactly what a call that could reach a release
/// build would undo, whoever made it.
fn use_coordinator(
    slot: &Mutex<Option<Network>>,
    address: &str,
    device: &str,
    debug_build: bool,
) -> Answer {
    if !debug_build {
        return Err(Failure::Usage(
            "this build joins the official coordinator and no other".to_owned(),
        ));
    }
    let device = (!device.is_empty()).then(|| device.to_owned());
    if let Some(device) = &device {
        itsanas_node::coordinator::parse_device(device)?;
    }
    let mut slot = slot
        .lock()
        .map_err(|_| Failure::Usage("the network lock was poisoned by a panic".to_owned()))?;
    *slot = Some(Network {
        address: address.to_owned(),
        device,
    });
    Ok(serde_json::json!({ "address": address }).to_string())
}

// -------------------------------------------------------------------- page

/// Nobody touched the page for this long: it stops, as a computer's does, and
/// the app starts it again when the person comes back. Half an hour, because a
/// phone is picked up and put down all day.
const PAGE_IDLE: Duration = Duration::from_secs(30 * 60);

/// How to stop the page that is being served, if one is.
static PAGE: Mutex<Option<Stop>> = Mutex::new(None);

/// Which page is current. A page stopped by the app, or replaced by a new one,
/// ends without a word: its `closed` would otherwise be read as a page that
/// ended by itself, and the app would start another, for ever.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Serve the setup engine's page on 127.0.0.1 and answer its address, the key
/// in the fragment. `mode` is `setup` (first run) or `settings`; `biometric` is
/// whether the phone has a fingerprint or a face to offer. The page's secrets
/// are asked through the app's `Host` object (`host.rs`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_pageStart(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    mode: JString,
    biometric: jboolean,
) -> jstring {
    let home = home_of(&mut env, &home);
    let mode = text(&mut env, &mode, "the mode");
    let remembered = host::remember(&mut env);
    answer(&mut env, move || {
        remembered.map_err(Failure::Usage)?;
        start_page(&home?, &mode?, biometric != 0)
    })
}

fn start_page(home: &Path, mode: &str, biometric: bool) -> Answer {
    let mode = match mode {
        "setup" => Mode::Setup,
        "settings" => Mode::Settings,
        other => return Err(Failure::Usage(format!("no page is called {other}"))),
    };
    stop_page();
    let network = network()?;
    let mine = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let backends = Backends::of_a_phone(
        Box::new(move || {
            let prompt: Box<dyn SecretPrompt> = Box::new(host::PhonePrompt::of_page(mine));
            Ok(prompt)
        }),
        Box::new(|| Box::new(host::PhoneService)),
        Arc::new(host::PhoneHost),
        Arc::new(Held(&NODE)),
        (network.address, network.device),
    )
    .with_biometric(biometric);
    let server = Server::bind(home, None, mode, home, backends)?;
    let url = server.url();
    *PAGE
        .lock()
        .map_err(|_| Failure::Usage("the page lock was poisoned by a panic".to_owned()))? =
        Some(server.stopper());
    std::thread::Builder::new()
        .name("page".to_owned())
        .spawn(move || {
            let done = server.serve(PAGE_IDLE);
            if GENERATION.load(Ordering::SeqCst) == mine {
                itsanas_cli::shell::Host::event(
                    &host::PhoneHost,
                    "closed",
                    if done { "done" } else { "left" },
                );
            }
        })
        .map_err(|error| Failure::Usage(format!("the page could not start: {error}")))?;
    Ok(url)
}

fn stop_page() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let stop = PAGE.lock().ok().and_then(|mut page| page.take());
    if let Some(stop) = stop {
        stop.stop();
    }
}

/// The person left the page (Back on its first step, or the app went away):
/// the engine stops and nothing answers on that port any more.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_pageStop(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        stop_page();
        Ok("{}".to_owned())
    })
}

/// Recover the passphrase from the 24 words of the account at `home`, asked on
/// the app's own screens: the locked phone has no node open, so no page.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_forgot(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let remembered = host::remember(&mut env);
    answer(&mut env, move || {
        remembered.map_err(Failure::Usage)?;
        let message =
            itsanas_cli::shell::forgot(&home?, &mut host::PhonePrompt::default(), &host::PhoneService)?;
        Ok(serde_json::json!({ "message": message }).to_string())
    })
}

/// The words as typed, for the native screen's live check:
/// `{"ok":bool,"wordIndex":n|null,"count":n}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_checkWords(
    mut env: JNIEnv,
    _class: JClass,
    phrase: JString,
) -> jstring {
    let phrase = text(&mut env, &phrase, "the words");
    answer(&mut env, move || Ok(check_words(&phrase?)))
}

// ------------------------------------------------------------------- files

/// Every file the account has, downloaded or not.
///
/// The Drive listing. `here` is what decides whether a tap opens the file or
/// fetches it first, and a file that is not here is still a file: showing only
/// what this device holds would tell somebody their photos were gone.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_list(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        with_node(|node| {
            let listing = itsanas_store::catalogue(&node.store, &node.vault)?;
            let files: Vec<_> = listing
                .files
                .iter()
                .map(|file| {
                    serde_json::json!({
                        "path": file.path,
                        "size": file.size,
                        "modified": file.modified_unix,
                        "here": file.presence == itsanas_store::Presence::Local,
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "files": files,
                "complete": listing.complete,
            })
            .to_string())
        })
    })
}

/// Write one file out of the account, fetching it first if it is not here.
///
/// `destination` is a path the application may write: on Android that is its
/// own directory or a document the person chose. Fetching overrides the
/// storage limit on purpose — an explicit request beats a background budget.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_get(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    destination: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    let destination = text(&mut env, &destination, "the destination");

    answer(&mut env, move || {
        let (path, destination) = (path?, destination?);
        with_node(|node| get_file(node, &path, &destination))
    })
}

/// [`Java_fr_ngas_itsanas_Native_get`] without the JNI.
fn get_file(node: &Node, path: &str, destination: &str) -> Answer {
    let content = match node.store.read_file(path)? {
        Some(content) => content,
        None => fetch(node, path)?,
    };

    std::fs::write(destination, &content)
        .map_err(|error| Failure::Usage(format!("could not write {destination}: {error}")))?;

    Ok(serde_json::json!({ "bytes": content.len() }).to_string())
}

/// Fetch a file this device knows about and has not downloaded.
///
/// The same walk `itsanas get` does: resolve the chunks from the log, ask each
/// of the account's machines in turn, stop at the first that serves them.
fn fetch(node: &Node, path: &str) -> Result<Vec<u8>, Failure> {
    let Some(chunks) = itsanas_store::chunks_for(&node.store, &node.vault, path)? else {
        return Err(Failure::Usage(format!("no such file: {path}")));
    };
    let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

    let targets = targets(node);
    if targets.is_empty() {
        return Err(Failure::Usage(format!(
            "{path} is in this account and not on this device, and {NO_MACHINE_YET}"
        )));
    }

    for (target, pinned) in &targets {
        let Ok(mut client) = itsanas_net::PeerClient::connect(
            itsanas_net::AddressList(target.as_str()),
            &node.device,
            node.store.owner(),
            *pinned,
        ) else {
            continue;
        };
        let _ = node.introduce(&mut client, itsanas_discover::now_unix());

        if itsanas_net::session::fetch_only(&node.store, &node.vault, &mut client, &wanted).is_err()
        {
            continue;
        }

        if let Some(content) = node.store.read_file(path)? {
            return Ok(content);
        }
    }

    Err(Failure::Usage(format!(
        "{path} is in this account and no machine that is up would serve it"
    )))
}

/// Put a file into the account.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_put(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    source: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    let source = text(&mut env, &source, "the source file");

    answer(&mut env, move || {
        let (path, source) = (path?, source?);
        with_node(|node| put_file(node, &path, &source))
    })
}

/// How old the account's other pledges may be before a write asks again. So a
/// file added offline waits on a connect timeout once an hour, not every time.
const PLEDGES_FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(3600);

/// Bring what the account's other machines pledge up to date, if it is old:
/// without it a phone is held to the joining allowance for the whole account,
/// however much the owner's other machines lend. Written back unchanged when the
/// coordinator could not say, so the file's age is when this last asked.
fn refresh_old_pledges(node: &Node, now: std::time::SystemTime) {
    if Node::others_pledged_age(&node.home, now).is_some_and(|age| age < PLEDGES_FRESH_FOR) {
        return;
    }
    if !matches!(
        itsanas_node::coordinator::refresh_others_pledged(node),
        Ok(Some(_))
    ) {
        let _ = node.remember_others_pledged(Node::others_pledged(&node.home));
    }
}

/// [`Java_fr_ngas_itsanas_Native_put`] without the JNI.
fn put_file(node: &Node, path: &str, source: &str) -> Answer {
    let file = std::fs::File::open(source)
        .map_err(|error| Failure::Usage(format!("could not read {source}: {error}")))?;
    let incoming = file
        .metadata()
        .map_err(|error| Failure::Usage(format!("could not read {source}: {error}")))?
        .len();
    refresh_old_pledges(node, std::time::SystemTime::now());
    node.bound_writes()?;
    node.store.check_room(path, incoming)?;
    let entry = node.store.write_stream(path, file)?;
    node.store.flush_segment()?;

    Ok(serde_json::json!({
        "bytes": entry.size,
        "chunks": entry.chunks.len(),
    })
    .to_string())
}

/// Delete a file from the account, everywhere.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_remove(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");

    answer(&mut env, move || {
        let path = path?;
        with_node(|node| {
            let removed = node.store.remove_file(&path)?;
            if removed {
                node.store.flush_segment()?;
            }
            Ok(serde_json::json!({ "removed": removed }).to_string())
        })
    })
}

// ------------------------------------------------------------------- rounds

/// Run one sync round against every configured machine.
///
/// `metadataOnly` is the metered case: log segments arrive, contents do not,
/// and the listing is current for kilobytes. It is what
/// `itsanas_policy::plan` selects on a metered connection, and the shell is
/// expected to pass what the policy said rather than deciding for itself.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_sync(
    mut env: JNIEnv,
    _class: JClass,
    metadata_only: jboolean,
) -> jstring {
    let metadata_only = metadata_only != 0;

    answer(&mut env, move || {
        with_node(|node| sync_node(node, metadata_only, itsanas_discover::now_unix()))
    })
}

/// [`Java_fr_ngas_itsanas_Native_sync`] without the JNI.
///
/// Enrols first, best effort: a person has no step for it, and a phone that
/// never enrolled holds no slot, no name, and cannot read what its machines
/// pledge. Then what they pledge, which is the bound `bound_writes` sets.
fn sync_node(node: &Node, metadata_only: bool, now: u64) -> Answer {
    let scope = if metadata_only {
        itsanas_net::session::Scope::Metadata
    } else {
        itsanas_net::session::Scope::Everything
    };

    let _ = itsanas_node::coordinator::ensure_enrolled(node, now);
    let _ = itsanas_node::coordinator::refresh_others_pledged(node);

    let targets = targets(node);
    if targets.is_empty() {
        return Err(Failure::Usage(NO_MACHINE_YET.to_owned()));
    }

    // What comes in is held to the disk bound (8.1b); without this the
    // ceiling is the one opening the node set, which is none.
    node.bound_writes()?;

    let totals = run_round(node, &targets, scope);
    Ok(serde_json::json!({
        "reached": totals.reached,
        "adopted": totals.adopted,
        "sent": totals.sent,
        "released": totals.released,
        "freed": totals.freed,
        "unreachable": totals.unreachable,
    })
    .to_string())
}

/// What this node is and what it holds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_status(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || with_node(status_of))
}

/// [`Java_fr_ngas_itsanas_Native_status`] without the JNI.
fn status_of(node: &Node) -> Answer {
    let stats = node.store.stats()?;
    let vault = node.vault.stats()?;
    let coverage = node.store.coverage(itsanas_discover::now_unix())?;
    let absent = itsanas_store::absent_count(&node.store, &node.vault)?;

    Ok(serde_json::json!({
        "username": node.config.username,
        "userId": node.store.owner().to_string(),
        "deviceId": node.store.device_id().to_string(),
        "files": stats.files,
        "here": stats.files,
        "notHere": absent,
        "bytesOnDisk": stats.bytes_on_disk,
        "vaultBytes": vault.bytes,
        // Not `vaultBytes`: this account's own chains relayed here are in that.
        "heldForOthers": node.held_for_others()?,
        "enrolled": itsanas_node::coordinator::registered_with(&node.home).is_some(),
        "keepBytes": node.config.keep_bytes,
        "pledgeBytes": node.config.pledge_bytes,
        "peers": node.config.peers,
        "copiesElsewhere": coverage.complete_elsewhere,
        "onlyHere": coverage.only_here,
        "liveChunks": coverage.live_chunks,
    })
    .to_string())
}

// ------------------------------------------------------------------ updates

/// Whether the signed release `manifest` offers this application a newer APK:
/// `{"update":false}`, or `{"update":true,"version":..,"size":..,"sha256":..}`.
///
/// The phone is installed by hand, outside Play, so nothing else ever tells it
/// a release exists (2026-10-31). The manifest is checked with the release key
/// compiled into this build, the check a desktop node makes before it replaces
/// its own binary; a bad signature fails, it never reads as "up to date".
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_checkUpdate(
    mut env: JNIEnv,
    _class: JClass,
    manifest: JString,
    signature: JString,
) -> jstring {
    let manifest = text(&mut env, &manifest, "the release manifest");
    let signature = text(&mut env, &signature, "the manifest's signature");
    answer(&mut env, move || {
        let trust = itsanas_release::Trust::pinned().map_err(|error| release_failure(&error))?;
        update_offer(
            &manifest?,
            &signature?,
            &trust,
            itsanas_release::Version::running(),
        )
    })
}

/// [`Java_fr_ngas_itsanas_Native_checkUpdate`] without the JNI.
fn update_offer(
    manifest: &str,
    signature: &str,
    trust: &itsanas_release::Trust,
    running: itsanas_release::Version,
) -> Answer {
    let offered = itsanas_release::android_update(manifest.as_bytes(), signature, trust, running)
        .map_err(|error| release_failure(&error))?;
    let manifest =
        itsanas_release::Manifest::parse(manifest).map_err(|error| release_failure(&error))?;
    Ok(match offered {
        None => serde_json::json!({ "update": false }),
        Some(entry) => serde_json::json!({
            "update": true,
            "version": manifest.version.to_string(),
            "size": entry.size,
            "sha256": entry.sha256.iter().fold(String::new(), |mut hex, byte| {
                let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
                hex
            }),
        }),
    }
    .to_string())
}

/// Whether the APK downloaded to `path` is the one the signed `manifest`
/// names, size and both hashes: `"ok"`, or an exception saying which. Asked
/// again here rather than trusted from `update_offer`, so a file swapped
/// between the download and the install is still refused.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_checkApk(
    mut env: JNIEnv,
    _class: JClass,
    manifest: JString,
    signature: JString,
    path: JString,
) -> jstring {
    let manifest = text(&mut env, &manifest, "the release manifest");
    let signature = text(&mut env, &signature, "the manifest's signature");
    let path = text(&mut env, &path, "the downloaded file");
    answer(&mut env, move || {
        let trust = itsanas_release::Trust::pinned().map_err(|error| release_failure(&error))?;
        apk_matches(
            &manifest?,
            &signature?,
            &trust,
            itsanas_release::Version::running(),
            Path::new(&path?),
        )
    })
}

/// [`Java_fr_ngas_itsanas_Native_checkApk`] without the JNI.
fn apk_matches(
    manifest: &str,
    signature: &str,
    trust: &itsanas_release::Trust,
    running: itsanas_release::Version,
    path: &Path,
) -> Answer {
    let entry = itsanas_release::android_update(manifest.as_bytes(), signature, trust, running)
        .map_err(|error| release_failure(&error))?
        .ok_or_else(|| Failure::Usage("this release offers no newer application".to_owned()))?;
    entry
        .check_file(path)
        .map_err(|error| release_failure(&error))?;
    Ok("ok".to_owned())
}

fn release_failure(error: &itsanas_release::ReleaseError) -> Failure {
    Failure::Usage(error.to_string())
}

// ----------------------------------------------------------------- settings

/// What the sync policy says to do right now, and why.
///
/// The shell reports the conditions it can see — Android answers "is this
/// connection metered" directly — and gets back an interval, a scope and a
/// sentence to show. Deciding this in Kotlin would mean a second copy of a
/// decision table that a desktop daemon has already been exercising.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_plan(
    mut env: JNIEnv,
    _class: JClass,
    metered: jboolean,
    charging: jboolean,
    battery_low: jboolean,
    foreground: jboolean,
    content_on_metered: jboolean,
) -> jstring {
    answer(&mut env, move || {
        let conditions = itsanas_policy::Conditions {
            network: if metered != 0 {
                itsanas_policy::Network::Metered
            } else {
                itsanas_policy::Network::Unmetered
            },
            power: if battery_low != 0 {
                itsanas_policy::Power::Low
            } else if charging != 0 {
                itsanas_policy::Power::Charging
            } else {
                itsanas_policy::Power::OnBattery
            },
            attention: if foreground != 0 {
                itsanas_policy::Attention::Foreground
            } else {
                itsanas_policy::Attention::Background
            },
        };

        let settings = itsanas_policy::Settings {
            content_on_metered: content_on_metered != 0,
            background: true,
        };

        let plan = itsanas_policy::plan(conditions, settings);
        Ok(serde_json::json!({
            "scope": match plan.scope {
                itsanas_policy::Scope::Nothing => "nothing",
                itsanas_policy::Scope::Metadata => "metadata",
                itsanas_policy::Scope::Everything => "everything",
            },
            "everySeconds": plan.every.map(|every| every.as_secs()),
            "because": plan.because,
        })
        .to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_cli::account::{Login, login_into, logout_check, logout_into, restore_account};
    use itsanas_cli::limits::set_limits_on;
    use itsanas_cli::testing::{NOW, PASSPHRASE, live, phone, serving, serving_on, with_directory};
    use itsanas_node::NodeError;
    use itsanas_node::coordinator;
    use itsanas_node::pledges::DiskSpace;

    /// The JSON this shell answers with is the shape Kotlin parses.
    ///
    /// Not a deep test — the behaviour underneath belongs to the crates that
    /// own it — but the field names are a contract with another language, and
    /// a rename here fails silently over there.
    #[test]
    fn a_plan_is_reported_with_the_names_kotlin_reads() {
        let plan = itsanas_policy::plan(
            itsanas_policy::Conditions {
                network: itsanas_policy::Network::Metered,
                power: itsanas_policy::Power::OnBattery,
                attention: itsanas_policy::Attention::Background,
            },
            itsanas_policy::Settings::default(),
        );

        let rendered = serde_json::json!({
            "scope": "metadata",
            "everySeconds": plan.every.map(|every| every.as_secs()),
            "because": plan.because,
        });

        assert_eq!(rendered["scope"], "metadata");
        assert_eq!(rendered["everySeconds"], 24 * 60 * 60);
        assert!(
            rendered["because"]
                .as_str()
                .is_some_and(|why| !why.is_empty()),
            "a plan with no reason leaves the application with nothing to show"
        );
    }

    fn signed(
        version: &str,
        apk: Option<&[u8]>,
    ) -> (tempfile::TempDir, String, String, itsanas_release::Trust) {
        let dir = tempfile::tempdir().expect("scratch");
        std::fs::write(
            dir.path().join("itsanas-x86_64-unknown-linux-gnu"),
            b"linux".repeat(50),
        )
        .unwrap();
        if let Some(apk) = apk {
            std::fs::write(dir.path().join("itsanas-android-apk"), apk).unwrap();
        }
        let version = itsanas_release::Version::parse(version).unwrap();
        let manifest = itsanas_release::Manifest::from_dir(version, dir.path()).unwrap();
        let text = manifest.to_text();
        let key = itsanas_release::ReleaseKey::generate().unwrap();
        let (_, signature) = key.sign_manifest(text.as_bytes()).unwrap();
        let trust = itsanas_release::Trust::from_pinned(Some(key.public_bytes())).unwrap();
        (dir, text, signature, trust)
    }

    /// The application's own update, as the phone sees it: a newer signed
    /// release with an APK is offered with the hash the download must have,
    /// the downloaded file is accepted only when it is that APK, and a file
    /// changed after the check is refused. Sabotage: drop `check_file` in
    /// `apk_matches` -- red on the swapped file, which would be installed.
    #[test]
    fn red_team_the_phone_installs_only_the_apk_the_signed_release_names() {
        let running = itsanas_release::Version::parse("0.1.13").unwrap();
        let apk = b"PK the application".repeat(200);
        let (dir, text, signature, trust) = signed("0.1.14", Some(&apk));
        let offer: serde_json::Value =
            serde_json::from_str(&update_offer(&text, &signature, &trust, running).unwrap())
                .unwrap();
        assert_eq!(offer["update"], true);
        assert_eq!(offer["version"], "0.1.14");
        assert_eq!(offer["size"], apk.len());

        let download = dir.path().join("download");
        std::fs::write(&download, &apk).unwrap();
        assert_eq!(
            apk_matches(&text, &signature, &trust, running, &download).unwrap(),
            "ok"
        );

        std::fs::write(&download, b"PK something else".repeat(200)).unwrap();
        assert!(
            apk_matches(&text, &signature, &trust, running, &download).is_err(),
            "a file that is not the signed APK was accepted for install"
        );

        let (_d, text, signature, trust) = signed("0.1.13", Some(&apk));
        let offer = update_offer(&text, &signature, &trust, running).unwrap();
        assert_eq!(
            offer, r#"{"update":false}"#,
            "the running version offered as an update"
        );
        let other = itsanas_release::ReleaseKey::generate().unwrap();
        let stranger = itsanas_release::Trust::from_pinned(Some(other.public_bytes())).unwrap();
        assert!(
            update_offer(&text, &signature, &stranger, running).is_err(),
            "a release signed by another key read as up to date or as an update"
        );
    }

    /// A coordinator nobody listens at, for the tests that must not reach the
    /// official one.
    fn nowhere() -> Network {
        Network {
            address: "127.0.0.1:1".to_owned(),
            device: None,
        }
    }

    /// The phone stuck on Unlock (2026-10-31): the sync service kept the node
    /// open in the process, and opening it again was refused by the database
    /// as "already open". A second open of the same home answers the node
    /// already held. Sabotage: open before letting go of the held node, as the
    /// code did, without the reuse -- the second open fails with the phone's
    /// own words, "index.redb is already open in another process".
    #[test]
    fn red_team_unlocking_while_the_sync_service_holds_the_node_opens_nothing_twice() {
        let dir = tempfile::tempdir().expect("scratch");
        let home = dir.path().join("phone");
        drop(Node::create(&home, "a passphrase", "camille").expect("create"));
        let slot = Mutex::new(None);
        let first = open_into(&slot, &home, "a passphrase", &nowhere()).expect("first open");
        let again = open_into(&slot, &home, "a passphrase", &nowhere());
        assert!(
            again.is_ok(),
            "unlocking while the node is already open failed ({:?}): the phone stays \
             on Unlock and nothing can be used",
            again.err().map(|f| f.message())
        );
        assert_eq!(again.unwrap(), first);
    }

    /// The Unlock screen checks the passphrase when the node is already open:
    /// what is typed there is judged by the keystore, whoever holds the node, and
    /// the right one is answered as before. Sabotage: answer the held node before
    /// the check.
    #[test]
    fn red_team_the_unlock_screen_checks_the_passphrase_when_the_node_is_already_open() {
        let dir = tempfile::tempdir().expect("scratch");
        let home = dir.path().join("phone");
        drop(Node::create(&home, "a passphrase", "camille").expect("create"));
        let slot = Mutex::new(None);
        let first = open_into(&slot, &home, "a passphrase", &nowhere()).expect("first open");

        let wrong = open_into(&slot, &home, "not the passphrase", &nowhere());
        assert!(
            matches!(wrong, Err(Failure::Node(NodeError::Unlock))),
            "the Unlock screen opened the account for a wrong passphrase: {:?}",
            wrong.map_err(|failure| failure.message())
        );
        assert!(
            slot.lock().unwrap().is_some(),
            "a wrong passphrase closed the node the sync service holds"
        );
        assert_eq!(
            open_into(&slot, &home, "a passphrase", &nowhere()).expect("right passphrase"),
            first
        );
    }

    const GIB: u64 = 1 << 30;
    const NEW_PASSPHRASE: &str = "a new passphrase for this phone";

    fn network_at(address: std::net::SocketAddr) -> Network {
        Network {
            address: address.to_string(),
            device: None,
        }
    }

    /// A disk that answers, roomy enough to matter in no test that does not ask.
    fn roomy() -> DiskSpace {
        DiskSpace {
            free: Some(100 * GIB),
            total: Some(128 * GIB),
        }
    }

    fn answer(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("JSON")
    }

    fn gib(n: i64) -> i64 {
        n * (1 << 30)
    }

    /// A negative `keep` is no limit and 0 is the listing alone: Kotlin always
    /// sends bytes, so "nothing" and "everything" must not read the same. 0
    /// survives the file, and a round then lists a file without fetching it.
    /// Sabotage: `(keep > 0).then(..)`, as it was (0 reads as no limit, and a
    /// person who asked for nothing here downloads everything).
    #[test]
    fn red_team_a_negative_keep_is_no_limit_and_zero_is_the_listing_alone() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().unwrap();
            let (nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
            let source = dir.path().join("source");
            std::fs::write(&source, b"kept on the NAS").unwrap();
            put_file(&nas, "one.txt", source.to_str().unwrap()).unwrap();

            let (mut me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));
            set_limits_on(&mut me, 0, -1, None).unwrap();
            assert_eq!(
                me.config.keep_bytes, None,
                "a negative keep is not no limit"
            );

            set_limits_on(&mut me, 0, 0, None).unwrap();
            assert_eq!(me.config.keep_bytes, Some(0), "keep 0 read as no limit");
            let saved = itsanas_node::Config::load(&Node::config_path(&me.home)).unwrap();
            assert_eq!(saved.keep_bytes, Some(0), "keep 0 did not survive the file");
            assert!(!saved.keeping().is_everything());

            serving(&nas, |at| {
                me.config.peers.push(at.to_string());
                let said: serde_json::Value =
                    serde_json::from_str(&sync_node(&me, false, NOW).unwrap()).unwrap();
                assert_eq!(said["reached"], 1, "{said}");
            });
            let status: serde_json::Value = serde_json::from_str(&status_of(&me).unwrap()).unwrap();
            assert_eq!(status["notHere"], 1, "{status}");
            assert_eq!(
                me.store.read_file("one.txt").unwrap(),
                None,
                "a budget of nothing downloaded the file"
            );
        });
    }

    /// A phone cannot promise a disk it has not got: pledging more than is free
    /// made every later write and pull refuse, with no sentence that said why.
    /// Sabotage: drop either check in `check_phone_disk`.
    #[test]
    fn red_team_the_phone_refuses_limits_its_disk_cannot_hold() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, _) =
            Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        let path = Node::config_path(&node.home);
        let before = std::fs::read(&path).unwrap();
        let free = Some(30 * GIB);

        let said = set_limits_on(&mut node, gib(31), -1, free)
            .expect_err("31 GiB promised on 30 free")
            .message();
        assert!(
            said.contains("Space you offer") && !said.contains("itsanas "),
            "{said}"
        );
        let said = set_limits_on(&mut node, gib(25), gib(10), free)
            .expect_err("35 GiB promised on 30 free")
            .message();
        assert!(
            said.contains("Space you offer")
                && said.contains("How much of your data")
                && !said.contains("itsanas "),
            "the phone was told something it cannot act on: {said}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "a refused pair rewrote the node file"
        );
        assert_eq!(
            node.config.pledge_bytes, 0,
            "a refused pair changed the pledge"
        );

        // Promised beyond what is free, though the pair would fit: the files here
        // can be released to make room for a keep, not for somebody else's data.
        let source = dir.path().join("source");
        std::fs::write(&source, vec![7u8; 4096]).unwrap();
        put_file(&node, "one.bin", source.to_str().unwrap()).unwrap();
        let said = set_limits_on(&mut node, gib(30) + 1, -1, free)
            .expect_err("a byte more than is free was promised")
            .message();
        assert!(said.contains("at most"), "{said}");

        set_limits_on(&mut node, gib(20), gib(10), free).expect("exactly what the disk holds");
        set_limits_on(&mut node, gib(31), gib(10), None).expect("a disk that will not say");
    }

    /// `get` dials who `sync` dials: a file reached through a machine known only
    /// from the coordinator, with nothing typed in. The same bug the round had, on
    /// the path that opens a file. Sabotage: `fetch` reads `config.peers` alone
    /// (it says "no machine configured" beside a running machine).
    #[test]
    fn red_team_a_file_is_fetched_from_a_machine_known_only_from_the_coordinator() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().unwrap();
            let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
            let source = dir.path().join("source");
            std::fs::write(&source, b"kept on the NAS").unwrap();
            put_file(&nas, "Photos/two.jpg", source.to_str().unwrap()).unwrap();
            let (me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));

            itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
            let server = itsanas_net::PeerServer::bind("127.0.0.1:0").unwrap();
            let at = server.local_addr().unwrap().to_string();
            nas.config.announce = Some(at.clone());
            itsanas_node::coordinator::announce(&nas, &at, itsanas_discover::now_unix()).unwrap();

            serving_on(&server, &nas, |_| {
                assert!(
                    me.config.peers.is_empty(),
                    "the fixture typed an address in"
                );
                let round: serde_json::Value = serde_json::from_str(
                    &sync_node(&me, true, itsanas_discover::now_unix()).unwrap(),
                )
                .unwrap();
                assert_eq!(round["reached"], 1, "{round}");
                assert_eq!(
                    me.store.read_file("Photos/two.jpg").unwrap(),
                    None,
                    "a metadata round fetched the content, so the fetch proves nothing"
                );

                let out = dir.path().join("out");
                get_file(&me, "Photos/two.jpg", out.to_str().unwrap())
                    .unwrap_or_else(|failure| panic!("not fetched: {}", failure.message()));
                assert_eq!(std::fs::read(&out).unwrap(), b"kept on the NAS");
            });
        });
    }

    /// A phone with no machine yet says so, and says nothing of an invitation it
    /// was never asked for; a round still enrols it first, since a person has no
    /// step for that, and reads what the account's machines pledge. Sabotage: drop
    /// `ensure_enrolled` from `sync_node` (no slot, no name, nothing to read
    /// pledges with); drop `refresh_others_pledged`.
    #[test]
    fn red_team_a_round_enrols_the_phone_and_reads_what_its_machines_pledge() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().unwrap();
            let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
            nas.config.pledge_bytes = 100 * GIB;
            itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
            let (me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));

            let said = sync_node(&me, true, NOW + 1)
                .expect_err("no machine has announced itself")
                .message();
            assert!(said.contains("no other machine"), "{said}");
            assert!(!said.contains("invitation"), "{said}");

            assert!(
                live(directory, me.store.device_id()),
                "a phone that ran a round never enrolled: its name is unclaimed"
            );
            assert_eq!(
                itsanas_node::coordinator::registered_with(&me.home),
                Some(address.to_string())
            );
            assert_eq!(
                Node::others_pledged(&me.home),
                100 * GIB,
                "the phone holds the account to 10 GiB however much the NAS lends"
            );
            let status: serde_json::Value = serde_json::from_str(&status_of(&me).unwrap()).unwrap();
            assert_eq!(status["enrolled"], true, "{status}");
            assert_eq!(status["heldForOthers"], 0, "{status}");
        });
    }

    /// A write counts what the account's machines lend, asked when it is missing
    /// or an hour old and not otherwise, so a file added offline does not wait on a
    /// connect timeout every time. Sabotage: drop the call from `put_file` (a phone
    /// on an account above 10 GiB is refused every write); drop the age check; leave
    /// the file alone when the coordinator cannot say.
    #[test]
    fn red_team_a_write_asks_what_the_machines_lend_at_most_once_an_hour() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().unwrap();
            let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
            nas.config.pledge_bytes = 100 * GIB;
            itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
            let (me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));
            itsanas_node::coordinator::register_with(&me, None, NOW).unwrap();

            let source = dir.path().join("source");
            std::fs::write(&source, b"a photo").unwrap();
            put_file(&me, "one.jpg", source.to_str().unwrap()).unwrap();
            assert_eq!(
                Node::account_pledge(&me.home, &me.config),
                100 * GIB,
                "a write did not ask what the account's machines lend"
            );

            nas.config.pledge_bytes = 200 * GIB;
            itsanas_node::coordinator::register_with(&nas, None, NOW + 1).unwrap();
            put_file(&me, "two.jpg", source.to_str().unwrap()).unwrap();
            assert_eq!(
                Node::others_pledged(&me.home),
                100 * GIB,
                "every write asked, so every file added offline waits on a timeout"
            );

            let two_hours_ago =
                std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
            std::fs::File::options()
                .write(true)
                .open(me.home.join("others-pledged"))
                .unwrap()
                .set_modified(two_hours_ago)
                .unwrap();
            put_file(&me, "three.jpg", source.to_str().unwrap()).unwrap();
            assert_eq!(
                Node::others_pledged(&me.home),
                200 * GIB,
                "an hour on, the phone still believes the old figure"
            );
        });

        let dir = tempfile::tempdir().unwrap();
        let (mut me, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        me.config.coordinator = Some("127.0.0.1:1".to_owned());
        refresh_old_pledges(&me, std::time::SystemTime::now());
        assert!(
            Node::others_pledged_age(&me.home, std::time::SystemTime::now()).is_some(),
            "nothing remembers that the coordinator was asked, so the next write waits too"
        );
        assert_eq!(Node::others_pledged(&me.home), 0);
    }

    /// The rule the screens state is the core's.
    #[test]
    fn the_passphrase_rule_is_the_core_s() {
        let rule: serde_json::Value = serde_json::from_str(&passphrase_rule()).unwrap();
        assert_eq!(rule["min"], itsanas_node::node::MIN_PASSPHRASE_CHARS);
        assert_eq!(rule["max"], itsanas_node::node::MAX_PASSPHRASE_CHARS);
    }

    /// A RELEASE BUILD JOINS THE OFFICIAL COORDINATOR AND NO OTHER: the test hook
    /// exists on both sides so Kotlin and Rust agree on the native calls, and it
    /// refuses unless compiled for debugging. Sabotage: pass `true` in the shim
    /// (the source check); drop the `debug_build` test.
    #[test]
    fn red_team_a_release_build_cannot_be_pointed_at_another_coordinator() {
        let slot = Mutex::new(None);
        assert!(
            use_coordinator(&slot, "evil.test:1", "", false).is_err(),
            "a release build took another coordinator"
        );
        assert_eq!(*slot.lock().unwrap(), None);
        let source = include_str!("lib.rs");
        assert!(
            source.contains(concat!("&device?, cfg!", "(debug_assertions))")),
            "the shim no longer tells this function what kind of build it is"
        );

        use_coordinator(&slot, "127.0.0.1:9", "", true).expect("a debug build");
        assert_eq!(
            *slot.lock().unwrap(),
            Some(Network {
                address: "127.0.0.1:9".to_owned(),
                device: None,
            })
        );
        assert!(
            use_coordinator(&slot, "127.0.0.1:9", "not hex", true).is_err(),
            "a device that is no device was taken for a pin"
        );
    }

    /// Every `external fun` in `Native.kt` has an entry point here, and every
    /// entry point here is declared there. A name on one side only compiles
    /// on both and fails on the phone, at the first tap, with
    /// `UnsatisfiedLinkError` -- and no CI job runs the app. Sabotage: rename
    /// `Java_fr_ngas_itsanas_Native_passphraseRule`.
    #[test]
    fn every_kotlin_native_call_has_its_rust_entry_point_and_back() {
        let kotlin = include_str!("../../../android/app/src/main/java/fr/ngas/itsanas/Native.kt");
        let rust = include_str!("lib.rs");
        let mut declared: Vec<&str> = kotlin
            .lines()
            .filter_map(|line| line.trim().strip_prefix("external fun "))
            .filter_map(|rest| rest.split('(').next())
            .collect();
        let mut exported: Vec<&str> = rust
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("pub extern \"system\" fn Java_fr_ngas_itsanas_Native_")
            })
            .filter_map(|rest| rest.split('(').next())
            .collect();
        declared.sort_unstable();
        exported.sort_unstable();
        assert!(
            declared.contains(&"passphraseRule"),
            "Native.kt was not read: {declared:?}"
        );
        assert_eq!(
            declared, exported,
            "Kotlin and Rust disagree on the native calls"
        );
    }

    /// A closed node fails with a sentence a person can act on.
    #[test]
    fn asking_a_closed_node_says_so_rather_than_crashing() {
        let outcome: Result<(), Failure> = with_node(|_| Ok(()));
        match outcome {
            Err(Failure::Closed) => {}
            _ => panic!("a closed node answered as though it were open"),
        }
        assert_eq!(
            Failure::Closed.message(),
            "no account is open on this device"
        );
    }

    /// A node restored or unlocked on the phone joins the official coordinator,
    /// pinned to its device, and nothing else: an app that let a node be pointed
    /// elsewhere would let whoever talked a user into it send that user's account
    /// to their own network. A new account comes through `login` too, so this
    /// covers every way a node gets onto a phone. Covers `restore_account` and
    /// `open_into`, which the JNI `login` and `open` call, with the network the
    /// shims pass. Sabotage: return the node from either without `pin`; have
    /// `network()` answer anything else when no test set it.
    #[test]
    fn red_team_a_new_or_restored_phone_node_is_on_the_official_coordinator() {
        use itsanas_node::coordinator::{DEFAULT_COORDINATOR, DEFAULT_COORDINATOR_DEVICE};
        let dir = tempfile::tempdir().unwrap();
        let official = network().unwrap();
        assert_eq!(official, Network::official());
        let (older, phrase) =
            Node::create(&dir.path().join("older"), "a long passphrase", "nicolas").unwrap();
        drop(older);
        let slot = Mutex::new(None);
        open_into(
            &slot,
            &dir.path().join("older"),
            "a long passphrase",
            &official,
        )
        .unwrap();
        let guard = slot.lock().unwrap();
        let opened = guard.as_ref().unwrap();
        let restored = restore_account(
            &dir.path().join("restored"),
            "a long passphrase",
            "nicolas",
            phrase.as_str(),
            &official,
        )
        .unwrap();
        for (what, node) in [("unlocked", opened), ("restored", &restored)] {
            let saved = itsanas_node::Config::load(&Node::config_path(&node.home)).unwrap();
            for config in [&node.config, &saved] {
                assert_eq!(
                    (
                        config.coordinator.as_deref(),
                        config.coordinator_device.as_deref()
                    ),
                    (Some(DEFAULT_COORDINATOR), Some(DEFAULT_COORDINATOR_DEVICE)),
                    "a {what} account is not pinned to the official coordinator: the \
                     user cannot join, or joins a network nobody chose for them"
                );
            }
        }
    }

    /// The reason a withdrawal is safe: what the phone wrote outlives it. Written
    /// here, pushed to a sibling that serves, logged out; a new login dialling that
    /// sibling lists the file and reads it. Sabotage: ask the host for no claims
    /// in `Node::introduce` (the new login's roster refuses the old phone's chain:
    /// the file is listed and cannot be read).
    #[test]
    fn red_team_files_written_by_a_phone_that_logged_out_reach_the_next_login() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().expect("temp dir");
            let home = dir.path().join("node");
            let (mut phone_node, phrase) = phone(&home, address, "camille", None);
            let (sibling, _) = phone(&dir.path().join("nas"), address, "camille", Some(&phrase));
            for node in [&phone_node, &sibling] {
                coordinator::register_with(node, None, NOW).expect("enrol");
            }

            let source = dir.path().join("source");
            std::fs::write(&source, b"written on the phone").expect("source");
            crate::put_file(
                &phone_node,
                "Photos/one.jpg",
                source.to_str().expect("path"),
            )
            .expect("put");

            serving(&sibling, |at| {
                phone_node.config.peers.push(at.to_string());
                phone_node.save_config().expect("save");

                let check = answer(&logout_check(&phone_node, false).expect("check"));
                assert!(check["reached"].as_u64() >= Some(1), "{check}");
                assert_eq!(
                    check["onlyHere"], 0,
                    "a pushed file is still called the only copy: the person is warned for \
                     nothing, and learns to type through it: {check}"
                );

                let slot = Mutex::new(Some(phone_node));
                let said = answer(&logout_into(&slot, &home, NOW + 1).expect("logout"));
                assert_eq!(said["withdrawn"], true, "{said}");
                assert!(said["told"].as_u64() >= Some(1), "{said}");

                let login = Login {
                    home: &home,
                    username: "camille",
                    phrase: &phrase,
                    passphrase: NEW_PASSPHRASE,
                };
                login_into(&slot, login, roomy(), &network_at(address), NOW + 2).expect("login");
                let mut guard = slot.lock().expect("lock");
                let node = guard.as_mut().expect("held");
                node.config.peers.push(at.to_string());

                crate::run_round(
                    node,
                    &crate::targets(node),
                    itsanas_net::session::Scope::Everything,
                );
                let listing = itsanas_store::catalogue(&node.store, &node.vault).expect("list");
                assert!(
                    listing
                        .files
                        .iter()
                        .any(|file| file.path == "Photos/one.jpg"),
                    "the next login does not see what the logged-out phone wrote: withdrawing \
                     a phone orphans its files. Listed: {:?}",
                    listing
                        .files
                        .iter()
                        .map(|file| &file.path)
                        .collect::<Vec<_>>()
                );
                let out = dir.path().join("out");
                crate::get_file(node, "Photos/one.jpg", out.to_str().expect("path")).expect("get");
                assert_eq!(std::fs::read(&out).expect("read"), b"written on the phone");
            });
        });
    }

    /// What a logout would lose is said before it happens, and the check closes
    /// nothing. Sabotage: report the coverage from before the round.
    #[test]
    fn a_phone_with_no_machine_is_told_its_files_are_only_here() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut node, _) =
            Node::create(&dir.path().join("node"), PASSPHRASE, "camille").expect("create");
        node.config.coordinator = Some("127.0.0.1:1".to_owned());
        let source = dir.path().join("source");
        std::fs::write(&source, b"the only copy").expect("source");
        crate::put_file(&node, "one.txt", source.to_str().expect("path")).expect("put");

        let said = answer(&logout_check(&node, true).expect("check"));
        assert_eq!(said["reached"], 0, "{said}");
        assert_eq!(said["username"], "camille", "{said}");
        assert!(said["onlyHere"].as_u64() >= Some(1), "{said}");
        assert_eq!(said["onlyHere"], said["liveChunks"], "{said}");
        assert_eq!(said["heldForOthers"], 0, "{said}");
    }
}
