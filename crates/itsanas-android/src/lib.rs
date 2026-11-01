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
//! prints for the same fault.
//!
//! **One node, behind a lock.** The store permits a single writer, which is a
//! property of the storage engine rather than a decision made here. The
//! application opens the node once and every call takes the same lock, so a
//! background round and a tap on a file cannot both be inside the store.
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

mod account;
mod devices;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use itsanas_node::{Config, Node, NodeError};
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jlong, jstring};

/// The node this application has open, if any.
///
/// A process holds at most one. Android will happily start a second activity
/// against the same files, and the store's single-writer rule would then
/// surface as "already open in another process" — from *this* process, which
/// reads as a bug in the storage engine rather than as what it is.
static NODE: Mutex<Option<Node>> = Mutex::new(None);

/// Anything an entry point can fail at, in the words the caller will see.
#[derive(Debug)]
enum Failure {
    /// The node is not open. Distinct from every other failure because it is
    /// the one a shell can fix by itself.
    Closed,
    /// Something the core refused, phrased for a person.
    Node(NodeError),
    /// The shell asked for something impossible.
    Usage(String),
}

impl From<NodeError> for Failure {
    fn from(error: NodeError) -> Self {
        Self::Node(error)
    }
}

// The store and the network answer with their own error types, and every one of
// them already reads as a sentence. Converting through `NodeError` keeps the
// words a person sees identical to the ones the command line prints for the
// same fault, which is what makes it possible to help somebody over a phone
// call.
impl From<itsanas_store::StoreError> for Failure {
    fn from(error: itsanas_store::StoreError) -> Self {
        Self::Node(NodeError::Store(error))
    }
}

impl From<itsanas_net::NetError> for Failure {
    fn from(error: itsanas_net::NetError) -> Self {
        Self::Node(NodeError::Net(error))
    }
}

/// The Settings section where this phone says how much it offers, and the one
/// where it says how much it keeps. A refusal names the place to act, since the
/// commands the core's own sentences name do not exist here.
const PLEDGE_SECTION: &str = "Space you offer other people";
const KEEP_SECTION: &str = "How much of your data to keep here";

impl Failure {
    fn message(&self) -> String {
        use itsanas_node::config::format_size;
        use itsanas_store::StoreError;
        match self {
            Self::Closed => "no account is open on this device".to_owned(),
            Self::Node(NodeError::Store(StoreError::DiskFull {
                path,
                incoming,
                room,
            })) => format!(
                "{path} needs at least {} on this phone, and only {} are free once the space it \
                 offers other people is set aside. Nothing was stored. Free some room, or lower \
                 \"{PLEDGE_SECTION}\" in Settings.",
                format_size(*incoming),
                format_size(*room),
            ),
            Self::Node(NodeError::Store(StoreError::OverBudget {
                path,
                total,
                allowed,
                ..
            })) => format!(
                "{path} would bring this account to at least {}, over the {} it may hold. \
                 Nothing was stored. Raising \"{PLEDGE_SECTION}\" in Settings, here or on \
                 another of your machines, earns it more.",
                format_size(*total),
                format_size(*allowed),
            ),
            Self::Node(error) => error.to_string(),
            Self::Usage(what) => what.clone(),
        }
    }
}

type Answer = Result<String, Failure>;

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

/// Whether a username can be taken, asked before anything is written. See
/// `account::check_name`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_checkName(
    mut env: JNIEnv,
    _class: JClass,
    username: JString,
) -> jstring {
    let username = text(&mut env, &username, "the username");
    answer(&mut env, move || {
        Ok(account::check_name(&network()?.address, &username?))
    })
}

/// The words and the name of an existing account, before anything is written.
/// See `account::check_restore`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_checkRestore(
    mut env: JNIEnv,
    _class: JClass,
    username: JString,
    phrase: JString,
) -> jstring {
    let username = text(&mut env, &username, "the username");
    let phrase = text(&mut env, &phrase, "the recovery phrase");
    answer(&mut env, move || {
        Ok(account::check_restore(
            &network()?.address,
            &username?,
            &phrase?,
        ))
    })
}

/// A fresh identity's words, written nowhere.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_newPhrase(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, account::new_phrase)
}

/// This node's words again, from its keystore. A wrong passphrase throws.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_words(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    passphrase: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let passphrase = text(&mut env, &passphrase, "the passphrase");
    answer(&mut env, move || account::words(&home?, &passphrase?))
}

/// Re-seal the keystore. The new passphrase obeys the rule; a wrong current one
/// throws.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_changePassphrase(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    current: JString,
    new: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let current = text(&mut env, &current, "the current passphrase");
    let new = text(&mut env, &new, "the new passphrase");
    answer(&mut env, move || {
        account::change_passphrase(&home?, &current?, &new?)
    })
}

/// One best-effort round, then what logging out would lose. Closes nothing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_logoutCheck(
    mut env: JNIEnv,
    _class: JClass,
    metered: jboolean,
) -> jstring {
    answer(&mut env, move || {
        with_node(|node| account::logout_check(node, metered != 0))
    })
}

/// Tell the machines, withdraw this device, close and erase the node.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_logout(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    answer(&mut env, move || {
        account::logout_into(&NODE, &home?, itsanas_discover::now_unix())
    })
}

/// "I forgot my passphrase": the words replace the node on this phone. See
/// `account::restore_over`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_restoreOver(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    phrase: JString,
    passphrase: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let phrase = text(&mut env, &phrase, "the recovery phrase");
    let passphrase = text(&mut env, &passphrase, "the passphrase");
    answer(&mut env, move || {
        let (home, phrase, passphrase) = (home?, phrase?, passphrase?);
        let login = account::Login {
            home: &home,
            username: "",
            phrase: &phrase,
            passphrase: &passphrase,
        };
        account::restore_over(
            &NODE,
            login,
            itsanas_node::pledges::disk_space(&home),
            &network()?,
            itsanas_discover::now_unix(),
        )
    })
}

/// Delete the whole account at the coordinator, then log out. `confirm` must be
/// the account's username: refused otherwise, nothing sent.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_deleteAccount(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    confirm: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let confirm = text(&mut env, &confirm, "the account's name");
    answer(&mut env, move || {
        account::delete_account(&NODE, &home?, &confirm?, itsanas_discover::now_unix())
    })
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

/// Restore an account on this device from its twenty-four words, in any form a
/// person saved them. A new account comes this way too, once its words were
/// shown and checked. See `account::login_into`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_login(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    username: JString,
    phrase: JString,
    passphrase: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let username = text(&mut env, &username, "the username");
    let phrase = text(&mut env, &phrase, "the recovery phrase");
    let passphrase = text(&mut env, &passphrase, "the passphrase");

    answer(&mut env, move || {
        let (home, username, phrase, passphrase) = (home?, username?, phrase?, passphrase?);
        let login = account::Login {
            home: &home,
            username: &username,
            phrase: &phrase,
            passphrase: &passphrase,
        };
        account::login_into(
            &NODE,
            login,
            itsanas_node::pledges::disk_space(&home),
            &network()?,
            itsanas_discover::now_unix(),
        )
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

/// The coordinator a node on this phone belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Network {
    address: String,
    /// The device to pin it to; `None` only for a test's coordinator.
    device: Option<String>,
}

impl Network {
    fn official() -> Self {
        Self {
            address: itsanas_node::coordinator::DEFAULT_COORDINATOR.to_owned(),
            device: Some(itsanas_node::coordinator::DEFAULT_COORDINATOR_DEVICE.to_owned()),
        }
    }

    /// Point `node` at this coordinator, pinned to its device, and save.
    ///
    /// The app offers no way to name another coordinator: a field for one let
    /// anybody who could talk a user into typing an address send that user's
    /// account to a network of their choosing. Done on restore and open, so a
    /// node made by an older build that could be pointed elsewhere comes back
    /// to the official network the next time it is unlocked.
    fn pin(&self, mut node: Node) -> Result<Node, Failure> {
        node.config.coordinator = Some(self.address.clone());
        node.config.coordinator_device.clone_from(&self.device);
        node.save_config()?;
        Ok(node)
    }
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

/// A restored node, on `network`'s coordinator.
fn restore_account(
    home: &std::path::Path,
    passphrase: &str,
    username: &str,
    phrase: &str,
    network: &Network,
) -> Result<Node, Failure> {
    network.pin(Node::restore(home, passphrase, username, phrase)?)
}

fn describe(node: &Node) -> String {
    serde_json::json!({
        "userId": node.store.owner().to_string(),
        "deviceId": node.store.device_id().to_string(),
        "username": node.config.username,
    })
    .to_string()
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

/// The machines to talk to: the account's own from the coordinator, pinned,
/// beside any address typed in.
///
/// One list for every path that dials. The phone read `peers` alone and said
/// "no machine is configured" while the owner's Windows node ran on the same
/// account (2026-10-31), first for a round and then for a fetch. A parked
/// machine (`relay:`) is left to the desktop's dialler for now.
fn targets(node: &Node) -> Vec<(String, Option<itsanas_crypto::DeviceId>)> {
    let mut targets: Vec<(String, Option<itsanas_crypto::DeviceId>)> = node
        .config
        .peers
        .iter()
        .map(|peer| (peer.clone(), None))
        .collect();
    if node.config.coordinator.is_some()
        && let Ok(found) = itsanas_node::coordinator::peers(node, node.store.owner())
    {
        for (device, address) in found {
            if !address.starts_with(itsanas_node::contact::RELAY_PREFIX)
                && !targets.iter().any(|(known, _)| *known == address)
            {
                targets.push((address, Some(device)));
            }
        }
    }
    targets
}

/// What a phone says when there is nobody to talk to.
const NO_MACHINE_YET: &str = "no other machine of this account is known yet: is ITSaNAS \
                              running on another of your machines?";

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

/// What one round against a list of machines did.
#[derive(Debug, Default)]
struct RoundTotals {
    reached: usize,
    adopted: usize,
    sent: u64,
    released: usize,
    freed: u64,
    unreachable: Vec<String>,
}

/// Run one round against each of `targets`; one that fails is counted, not
/// fatal.
fn run_round(
    node: &Node,
    targets: &[(String, Option<itsanas_crypto::DeviceId>)],
    scope: itsanas_net::session::Scope,
) -> RoundTotals {
    let mut totals = RoundTotals::default();
    for (target, pinned) in targets {
        let Ok(mut client) = itsanas_net::PeerClient::connect(
            itsanas_net::AddressList(target.as_str()),
            &node.device,
            node.store.owner(),
            *pinned,
        ) else {
            totals.unreachable.push(target.clone());
            continue;
        };
        let _ = node.introduce(&mut client, itsanas_discover::now_unix());

        match itsanas_node::round(
            &node.store,
            &node.vault,
            &node.config.keeping(),
            &mut client,
            scope,
        ) {
            Ok((report, keeping)) => {
                totals.reached += 1;
                totals.adopted += report.pull.adopted;
                totals.sent = totals.sent.saturating_add(report.push.bytes_sent);
                totals.released += keeping.released;
                totals.freed = totals.freed.saturating_add(keeping.freed);
            }
            Err(_) => totals.unreachable.push(target.clone()),
        }
    }
    totals
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

/// What the Space step and Settings show, in bytes.
///
/// Asks the coordinator what the account's other machines pledge, and falls back
/// on what it last said: offline the step still works, without "the same as your
/// other machines".
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_space(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        with_node(|node| space_of(node, itsanas_node::pledges::disk_space(&node.home)))
    })
}

/// [`Java_fr_ngas_itsanas_Native_space`] without the JNI, the disk given.
fn space_of(node: &Node, disk: itsanas_node::pledges::DiskSpace) -> Answer {
    use itsanas_node::pledges;
    let asked = itsanas_node::coordinator::other_pledges(node)
        .ok()
        .flatten();
    let others = asked.map_or_else(|| Node::others_pledged(&node.home), |told| told.total);
    let largest = asked.map_or(0, |told| told.largest);

    let (free, total) = (disk.free.unwrap_or(0), disk.total.unwrap_or(0));
    let local = node.store.local_bytes()?;
    let held = node.held_for_others()?;
    let account: u64 = itsanas_store::catalogue(&node.store, &node.vault)?
        .files
        .iter()
        .fold(0, |sum, file| sum.saturating_add(file.size));
    let pledged = others.saturating_add(node.config.pledge_bytes);
    let split = node.config.split;
    let usable = pledges::phone_usable(total, free, local);
    let suggested = pledges::suggest_phone(&node.config, total, free, local, others, largest);

    Ok(serde_json::json!({
        "freeBytes": free,
        "totalBytes": total,
        "localBytes": local,
        "heldForOthers": held,
        "pledgeBytes": node.config.pledge_bytes,
        "keepBytes": node.config.keep_bytes,
        "othersPledgedBytes": others,
        "largestOtherPledgeBytes": largest,
        "accountBytes": account,
        "earnedBytes": split.room_earned(pledged),
        "allowedBytes": Node::allowed_for(&node.config, pledged),
        // What leaves the phone its headroom, and what the disk can promise at
        // all. Sliders end here; `setLimits` is what refuses.
        "maxPledgeBytes": usable.min(free.saturating_add(held)),
        "maxKeepBytes": usable,
        "suggested": {
            "pledgeBytes": suggested.pledge,
            "keepBytes": suggested.keep,
            "sameAsOtherMachines": suggested.same_as_other_machines,
        },
        "split": { "own": split.own, "network": split.network },
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

/// Say how much of this account's own data to hold here, and which files.
///
/// `bytes` of zero or less means no limit. `order` is one of `newest`,
/// `oldest`, `smallest`; anything else is refused rather than guessed at.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_setKeep(
    mut env: JNIEnv,
    _class: JClass,
    bytes: jlong,
    order: JString,
    only: JString,
) -> jstring {
    let order = text(&mut env, &order, "the order");
    let only = text(&mut env, &only, "the path filter");

    answer(&mut env, move || {
        let (order, only) = (order?, only?);
        let order = itsanas_node::config::parse_order(&order)
            .ok_or_else(|| Failure::Usage(format!("unknown order {order:?}")))?;

        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        let node = guard.as_mut().ok_or(Failure::Closed)?;
        let keep = (bytes > 0).then(|| bytes.unsigned_abs());
        set_keep(node, keep, order, &only)
    })
}

/// [`Java_fr_ngas_itsanas_Native_setKeep`] without the JNI, so a test reaches
/// it: the split is checked before anything is changed, so a refused limit
/// leaves order and filter as they were too.
fn set_keep(
    node: &mut Node,
    keep: Option<u64>,
    order: itsanas_policy::keeping::Order,
    only: &str,
) -> Result<String, Failure> {
    check_phone_split(node, node.config.pledge_bytes, keep)?;

    node.config.keep_bytes = keep;
    node.config.keep_order = order;
    node.config.keep_only = only
        .split('\n')
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    node.save_config()?;

    Ok(describe_keeping(&node.config))
}

/// Say how much room this device offers other people.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_setPledge(
    mut env: JNIEnv,
    _class: JClass,
    bytes: jlong,
) -> jstring {
    answer(&mut env, move || {
        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        let node = guard.as_mut().ok_or(Failure::Closed)?;
        set_pledge(node, bytes.unsigned_abs())
    })
}

/// [`Java_fr_ngas_itsanas_Native_setPledge`] without the JNI, so a test
/// reaches it. A pledge under what the phone's `keep` needs is refused, as
/// the CLI's `pledge` refuses it.
fn set_pledge(node: &mut Node, bytes: u64) -> Result<String, Failure> {
    check_phone_split(node, bytes, node.config.keep_bytes)?;

    node.config.pledge_bytes = bytes;
    node.save_config()?;
    Ok(serde_json::json!({ "pledgeBytes": node.config.pledge_bytes }).to_string())
}

/// Set the pledge and how much to keep here together, checked as a pair.
///
/// The settings screen saves both at once. Saved one after the other, raising
/// both was refused: the new keep was checked against the old pledge, and the
/// person was told to raise a pledge they had just raised. A negative `keep`
/// means no limit; 0 is a budget of nothing, the listing and no contents.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_setLimits(
    mut env: JNIEnv,
    _class: JClass,
    pledge: jlong,
    keep: jlong,
) -> jstring {
    answer(&mut env, move || {
        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        let node = guard.as_mut().ok_or(Failure::Closed)?;
        set_limits(node, pledge, keep)
    })
}

/// [`Java_fr_ngas_itsanas_Native_setLimits`] without the JNI, against this
/// phone's own disk.
fn set_limits(node: &mut Node, pledge: i64, keep: i64) -> Result<String, Failure> {
    let free = itsanas_node::pledges::disk_space(&node.home).free;
    set_limits_on(node, pledge, keep, free)
}

/// [`set_limits`] with what the disk has free given, so a test can ask about a
/// disk the size of its choosing. `None` skips the disk: nothing is known of it.
///
/// Takes the JVM's signed longs so the sign is checked where a test reaches it:
/// a negative pledge read as its absolute value would offer space nobody asked
/// to give (found by `itsanas-redteam`).
fn set_limits_on(
    node: &mut Node,
    pledge: i64,
    keep: i64,
    free: Option<u64>,
) -> Result<String, Failure> {
    let pledge = u64::try_from(pledge)
        .map_err(|_| Failure::Usage(format!("a pledge cannot be negative ({pledge} bytes)")))?;
    let keep = u64::try_from(keep).ok();
    check_phone_split(node, pledge, keep)?;
    check_phone_disk(node, pledge, keep, free)?;

    node.config.pledge_bytes = pledge;
    node.config.keep_bytes = keep;
    node.save_config()?;
    Ok(serde_json::json!({
        "pledgeBytes": node.config.pledge_bytes,
        "keepBytes": node.config.keep_bytes,
    })
    .to_string())
}

/// Refuse a keep the account's pledges do not earn.
///
/// The account's, where a desktop's `keep` asks of its own machine's alone: a
/// phone that lends little is the ordinary case, and it may keep what the owner's
/// other machines earn. Phone setters only; `check_split` is unchanged.
fn check_phone_split(node: &Node, pledge: u64, keep: Option<u64>) -> Result<(), Failure> {
    let others = Node::others_pledged(&node.home);
    Node::check_split(&node.config, pledge.saturating_add(others), keep)
        .map_err(|refusal| phone_split(&refusal, others))
}

/// A split refusal in the phone's words.
///
/// `SplitRefusal`'s own `Display` ends with `itsanas space --pledge ... --apply`,
/// a command the phone does not have; this names the field on the settings
/// screen instead, with the same figures: this phone's share of them, since
/// the refusal counts what the account's other machines lend.
fn phone_split(refusal: &itsanas_node::node::SplitRefusal, others: u64) -> Failure {
    use itsanas_node::config::format_size;
    Failure::Usage(format!(
        "keeping {} on this phone needs at least {} in \"{PLEDGE_SECTION}\", and this phone \
         offers {}. Raise that, or keep less here.",
        format_size(refusal.keep),
        format_size(refusal.needed.saturating_sub(others)),
        format_size(refusal.pledge.saturating_sub(others)),
    ))
}

/// Refuse a pledge and a keep this phone's disk cannot hold, in its words: a
/// phone pledging more than its disk has is refused every later write and every
/// pull, with nothing that says why.
fn check_phone_disk(
    node: &Node,
    pledge: u64,
    keep: Option<u64>,
    free: Option<u64>,
) -> Result<(), Failure> {
    use itsanas_node::config::format_size;
    let held = node.held_for_others()?;
    if let Some(refused) = node.disk_refusal_given(free, pledge, held) {
        return Err(Failure::Usage(format!(
            "this phone has {} free, so at most {} can go in \"{PLEDGE_SECTION}\"; {} is more \
             than it could keep. Lower it.",
            format_size(refused.free),
            format_size(refused.room),
            format_size(refused.bytes),
        )));
    }
    let own = node.store.local_bytes()?;
    if let Some(committed) = itsanas_node::pledges::overcommitted(pledge, keep, free, own, held) {
        return Err(Failure::Usage(format!(
            "offering {} and keeping {} needs {}, and this phone has room for {}. Lower \
             \"{PLEDGE_SECTION}\" or \"{KEEP_SECTION}\".",
            format_size(pledge),
            format_size(keep.unwrap_or(0)),
            format_size(committed),
            format_size(free.unwrap_or(0).saturating_add(own).saturating_add(held)),
        )));
    }
    Ok(())
}

/// Enrol this account and device with the configured coordinator.
///
/// `invite` may be empty. A coordinator running `--invite-only` refuses an
/// account with no code the first time, which answers `needsInvite` rather than
/// throws, and accepts a member re-registering without one for ever after, which
/// is how a device refreshes its keys without costing an invitation.
///
/// Publishing an address is part of registering rather than a separate step: a
/// device nobody can reach has not really joined anything. A failure to
/// announce is reported and does not undo the enrolment, exactly as on the
/// command line.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_register(
    mut env: JNIEnv,
    _class: JClass,
    invite: JString,
) -> jstring {
    let invite = text(&mut env, &invite, "the invitation code");

    answer(&mut env, move || {
        let invite = invite?;
        let invite = invite.trim().to_owned();

        let secret = if invite.is_empty() {
            None
        } else {
            Some(itsanas_node::coordinator::decode_secret(&invite)?)
        };
        with_node(|node| {
            devices::register(node, secret.as_ref(), itsanas_discover::now_unix())
                .map(|answer| answer.to_string())
        })
    })
}

/// The account's devices, this phone marked, as the coordinator lists them.
///
/// See `devices::list` for when the list is partial and says so.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_devices(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        with_node(|node| devices::list(node).map(|answer| answer.to_string()))
    })
}

/// Withdraw one of the account's devices by its full id, freeing its slot.
///
/// Never this phone, and only a device the account's listing shows: see
/// `devices::withdraw`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_withdrawDevice(
    mut env: JNIEnv,
    _class: JClass,
    device: JString,
) -> jstring {
    let device = text(&mut env, &device, "the device id");

    answer(&mut env, move || {
        let device = device?;
        with_node(|node| {
            devices::withdraw(node, &device, itsanas_discover::now_unix())
                .map(|answer| answer.to_string())
        })
    })
}

/// Add a machine to sync with.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_addPeer(
    mut env: JNIEnv,
    _class: JClass,
    address: JString,
) -> jstring {
    let address = text(&mut env, &address, "the address");

    answer(&mut env, move || {
        let address = address?;
        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        let node = guard.as_mut().ok_or(Failure::Closed)?;

        if !node.config.peers.contains(&address) {
            node.config.peers.push(address);
            node.save_config()?;
        }
        Ok(serde_json::json!({ "peers": node.config.peers }).to_string())
    })
}

/// Forget a machine.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_removePeer(
    mut env: JNIEnv,
    _class: JClass,
    address: JString,
) -> jstring {
    let address = text(&mut env, &address, "the address");

    answer(&mut env, move || {
        let address = address?;
        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        let node = guard.as_mut().ok_or(Failure::Closed)?;

        node.config.peers.retain(|peer| peer != &address);
        node.save_config()?;
        Ok(serde_json::json!({ "peers": node.config.peers }).to_string())
    })
}

fn describe_keeping(config: &Config) -> String {
    serde_json::json!({
        "keepBytes": config.keep_bytes,
        "keepOnly": config.keep_only,
    })
    .to_string()
}

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
    use crate::account::tests::{serving, serving_on};
    use crate::devices::tests::{NOW, live, phone, with_directory};

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

    /// The phone's setters keep the split as `pledge` and `keep` do: a pledge
    /// lowered under what `keep` needs, or a keep the pledge does not earn,
    /// is refused and the node file is left as it was. Covers `set_pledge`
    /// and `set_keep`, not the JNI shims that lock the node and call them.
    /// Sabotage: drop the `check_split` call in either.
    #[test]
    fn red_team_the_phone_s_setters_keep_the_split() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, _phrase) = Node::create(&home, "a long passphrase", "nicolas").unwrap();
        node.config.pledge_bytes = 50 * GIB;
        node.config.keep_bytes = Some(20 * GIB);
        node.save_config().unwrap();
        let path = Node::config_path(&home);
        let before = std::fs::read(&path).unwrap();

        assert!(
            matches!(set_pledge(&mut node, 1024 * 1024), Err(Failure::Usage(_))),
            "setPledge accepted 1 MiB beside a 20 GiB keep"
        );
        let order = node.config.keep_order;
        let only = node.config.keep_only.clone();
        assert!(
            matches!(
                set_keep(&mut node, Some(40 * GIB), order, "Photos"),
                Err(Failure::Usage(_))
            ),
            "setKeep accepted 40 GiB on a 50 GiB pledge"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "a refused setter still rewrote the node file"
        );
        // Nor the node in memory: it lives in a process-wide mutex, so the
        // next setter that succeeds would save what a refused one left there.
        assert_eq!(
            node.config.pledge_bytes,
            50 * GIB,
            "refused pledge, yet set"
        );
        assert_eq!(
            node.config.keep_bytes,
            Some(20 * GIB),
            "refused keep, yet set"
        );
        assert_eq!(node.config.keep_only, only, "refused keep, filter changed");

        // Not setters that refuse everything.
        assert!(
            set_keep(&mut node, None, order, "").is_ok(),
            "no limit refused"
        );
        assert!(
            set_pledge(&mut node, 1024 * 1024).is_ok(),
            "a small pledge with no keep refused"
        );
    }

    /// The settings screen saves pledge and keep together. Raising both is
    /// accepted in one call (saved one after the other, the new keep was
    /// checked against the old pledge and refused), and a pair that does not
    /// fit is refused in words naming the screen's field, not a command the
    /// phone does not have; a negative pledge is refused. Sabotage: check the
    /// keep against the old pledge; word the refusal with `SplitRefusal`'s own
    /// `Display`; take the pledge's absolute value.
    #[test]
    fn red_team_the_phone_saves_pledge_and_keep_as_a_pair_and_says_so_in_its_words() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, _phrase) = Node::create(&home, "a long passphrase", "nicolas").unwrap();
        node.config.pledge_bytes = 50 * GIB;
        node.config.keep_bytes = Some(20 * GIB);
        node.save_config().unwrap();

        let gib = |n: i64| n * 1024 * 1024 * 1024;
        set_limits_on(&mut node, gib(500), gib(200), None)
            .expect("raising pledge and keep together was refused");
        assert_eq!(
            (node.config.pledge_bytes, node.config.keep_bytes),
            (500 * GIB, Some(200 * GIB))
        );

        assert!(
            set_limits_on(&mut node, -gib(5), 0, None).is_err(),
            "a negative pledge was taken as a positive one"
        );
        let Err(refused) = set_limits_on(&mut node, gib(1), gib(200), None) else {
            panic!("1 GiB offered earned a 200 GiB keep");
        };
        let said = refused.message();
        assert!(
            !said.contains("itsanas ") && said.contains("Space you offer"),
            "the phone was told something it cannot act on: {said}"
        );
        assert_eq!(
            node.config.pledge_bytes,
            500 * GIB,
            "a refused pair still changed the pledge"
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

    /// A phone's keep is judged against what the account pledges, not this
    /// phone alone; a desktop's rule is unchanged. Sabotage: pass the phone's
    /// pledge alone to `check_split` (a phone beside a 100 GiB NAS may keep 10);
    /// add the others to the desktop's rule.
    #[test]
    fn red_team_a_phone_keeps_what_the_account_earns_not_what_it_lends_itself() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, _) =
            Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        node.remember_others_pledged(100 * GIB).unwrap();

        set_limits_on(&mut node, 0, gib(42), None)
            .expect("the account's 100 GiB earn 42 GiB, and the phone was refused it");
        let refused = set_limits_on(&mut node, 0, gib(43), None)
            .expect_err("100 GiB earned more than 42 GiB")
            .message();
        assert!(
            refused.contains("Space you offer") && !refused.contains("itsanas "),
            "{refused}"
        );
        let order = node.config.keep_order;
        assert!(set_keep(&mut node, Some(43 * GIB), order, "").is_err());
        assert!(set_keep(&mut node, Some(42 * GIB), order, "").is_ok());
        assert!(set_pledge(&mut node, 0).is_ok());

        assert!(
            Node::check_split(&node.config, 0, Some(42 * GIB)).is_err(),
            "the desktop's rule, per machine, changed with the phone's"
        );
        assert!(
            set_limits_on(&mut node, gib(10), gib(43), None).is_ok(),
            "this phone's pledge no longer counts"
        );
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

    /// A refusal from the store on a phone names the Settings section, in sizes a
    /// person reads, never a command it has not got. Sabotage: fall through to the
    /// store's own `Display`.
    #[test]
    fn red_team_a_full_disk_and_a_full_account_are_said_in_the_phone_s_words() {
        use itsanas_store::StoreError;
        let full = Failure::from(StoreError::DiskFull {
            path: "one.jpg".to_owned(),
            incoming: 3 * GIB,
            room: GIB,
        })
        .message();
        let over = Failure::from(StoreError::OverBudget {
            path: "one.jpg".to_owned(),
            account: 9 * GIB,
            total: 12 * GIB,
            allowed: 10 * GIB,
        })
        .message();
        for said in [&full, &over] {
            assert!(
                said.contains("Space you offer") && said.contains("Settings"),
                "the phone was not told where to act: {said}"
            );
            assert!(
                !said.contains("itsanas ") && !said.contains(" bytes"),
                "the phone was told a command it lacks, or a count of bytes: {said}"
            );
            assert!(said.contains("one.jpg"), "which file: {said}");
        }
        assert!(
            full.contains("3.0 GiB") && full.contains("1.0 GiB"),
            "{full}"
        );
        assert!(
            over.contains("12.0 GiB") && over.contains("10.0 GiB"),
            "{over}"
        );
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

    /// The Space step's answer, in the names Kotlin reads, with a sibling's pledge
    /// suggested when the phone has the room. Sabotage: pass 0 for the largest
    /// (the same value is never offered); leave a field out.
    #[test]
    fn red_team_the_space_step_is_told_the_figures_it_shows() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().unwrap();
            let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
            nas.config.pledge_bytes = 100 * GIB;
            itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
            let (mut me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));
            itsanas_node::coordinator::register_with(&me, None, NOW).unwrap();

            let disk = itsanas_node::pledges::DiskSpace {
                free: Some(400 * GIB),
                total: Some(512 * GIB),
            };
            let said: serde_json::Value =
                serde_json::from_str(&space_of(&me, disk).unwrap()).unwrap();
            for key in [
                "freeBytes",
                "totalBytes",
                "localBytes",
                "heldForOthers",
                "pledgeBytes",
                "keepBytes",
                "othersPledgedBytes",
                "largestOtherPledgeBytes",
                "accountBytes",
                "earnedBytes",
                "allowedBytes",
                "maxPledgeBytes",
                "maxKeepBytes",
                "suggested",
                "split",
            ] {
                assert!(said.get(key).is_some(), "Kotlin reads {key}: {said}");
            }
            assert_eq!(said["freeBytes"], 400 * GIB);
            assert_eq!(said["othersPledgedBytes"], 100 * GIB);
            assert_eq!(said["largestOtherPledgeBytes"], 100 * GIB);
            assert_eq!(said["pledgeBytes"], 0);
            assert_eq!(said["suggested"]["pledgeBytes"], 100 * GIB);
            assert_eq!(said["suggested"]["keepBytes"], 85 * GIB);
            assert_eq!(said["suggested"]["sameAsOtherMachines"], true);
            assert_eq!(said["split"]["own"], 30);
            assert_eq!(said["split"]["network"], 70);
            assert_eq!(said["allowedBytes"], 100 * GIB * 30 / 70);

            // Coordinator gone: what it last said, and no "the same".
            me.config.coordinator = Some("127.0.0.1:1".to_owned());
            let said: serde_json::Value =
                serde_json::from_str(&space_of(&me, disk).unwrap()).unwrap();
            assert_eq!(said["othersPledgedBytes"], 100 * GIB, "{said}");
            assert_eq!(said["largestOtherPledgeBytes"], 0, "{said}");
            assert_eq!(said["suggested"]["sameAsOtherMachines"], false, "{said}");
        });
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
    /// `Java_fr_ngas_itsanas_Native_withdrawDevice`.
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
            declared.contains(&"withdrawDevice"),
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
}
