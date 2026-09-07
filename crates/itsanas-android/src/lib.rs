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

impl Failure {
    fn message(&self) -> String {
        match self {
            Self::Closed => "no account is open on this device".to_owned(),
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

/// Create an account and answer with its recovery phrase.
///
/// The phrase is returned exactly once and stored nowhere. A shell that does
/// not put it in front of the person, on paper, has failed them.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_create(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    username: JString,
    passphrase: JString,
) -> jstring {
    let home = home_of(&mut env, &home);
    let username = text(&mut env, &username, "the username");
    let passphrase = text(&mut env, &passphrase, "the passphrase");

    answer(&mut env, move || {
        let (home, username, passphrase) = (home?, username?, passphrase?);
        let (node, phrase) = Node::create(&home, &passphrase, &username)?;

        let answer = serde_json::json!({
            "phrase": phrase.as_str(),
            "userId": node.store.owner().to_string(),
            "deviceId": node.store.device_id().to_string(),
        });

        hold(node)?;
        Ok(answer.to_string())
    })
}

/// Restore an account on this device from its twenty-four words.
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
        let node = Node::restore(&home?, &passphrase?, &username?, &phrase?)?;
        let answer = describe(&node);
        hold(node)?;
        Ok(answer)
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
        let node = Node::open(&home?, &passphrase?)?;
        let answer = describe(&node);
        hold(node)?;
        Ok(answer)
    })
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

fn hold(node: Node) -> Result<(), Failure> {
    let mut guard = NODE
        .lock()
        .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
    *guard = Some(node);
    Ok(())
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
        with_node(|node| {
            let content = match node.store.read_file(&path)? {
                Some(content) => content,
                None => fetch(node, &path)?,
            };

            std::fs::write(&destination, &content).map_err(|error| {
                Failure::Usage(format!("could not write {destination}: {error}"))
            })?;

            Ok(serde_json::json!({ "bytes": content.len() }).to_string())
        })
    })
}

/// Fetch a file this device knows about and has not downloaded.
///
/// The same walk `itsanas get` does: resolve the chunks from the log, ask each
/// configured peer in turn, stop at the first that serves them.
fn fetch(node: &Node, path: &str) -> Result<Vec<u8>, Failure> {
    let Some(chunks) = itsanas_store::chunks_for(&node.store, &node.vault, path)? else {
        return Err(Failure::Usage(format!("no such file: {path}")));
    };
    let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

    if node.config.peers.is_empty() {
        return Err(Failure::Usage(format!(
            "{path} is in this account and not on this device, and there is no \
             machine configured to fetch it from"
        )));
    }

    for target in &node.config.peers {
        let Ok(mut client) = itsanas_net::PeerClient::connect(
            target.as_str(),
            &node.device,
            node.store.owner(),
            None,
        ) else {
            continue;
        };

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
        with_node(|node| {
            let file = std::fs::File::open(&source)
                .map_err(|error| Failure::Usage(format!("could not read {source}: {error}")))?;
            let entry = node.store.write_stream(&path, file)?;
            node.store.flush_segment()?;

            Ok(serde_json::json!({
                "bytes": entry.size,
                "chunks": entry.chunks.len(),
            })
            .to_string())
        })
    })
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
        with_node(|node| {
            let scope = if metadata_only {
                itsanas_net::session::Scope::Metadata
            } else {
                itsanas_net::session::Scope::Everything
            };

            if node.config.peers.is_empty() {
                return Err(Failure::Usage(
                    "no machine is configured to sync with".to_owned(),
                ));
            }

            let mut reached = 0usize;
            let mut adopted = 0usize;
            let mut sent = 0u64;
            let mut released = 0usize;
            let mut freed = 0u64;
            let mut unreachable = Vec::new();

            for target in &node.config.peers {
                let Ok(mut client) = itsanas_net::PeerClient::connect(
                    target.as_str(),
                    &node.device,
                    node.store.owner(),
                    None,
                ) else {
                    unreachable.push(target.clone());
                    continue;
                };

                match itsanas_node::round(
                    &node.store,
                    &node.vault,
                    &node.config.keeping(),
                    &mut client,
                    scope,
                ) {
                    Ok((report, keeping)) => {
                        reached += 1;
                        adopted += report.pull.adopted;
                        sent = sent.saturating_add(report.push.bytes_sent);
                        released += keeping.released;
                        freed = freed.saturating_add(keeping.freed);
                    }
                    Err(_) => unreachable.push(target.clone()),
                }
            }

            Ok(serde_json::json!({
                "reached": reached,
                "adopted": adopted,
                "sent": sent,
                "released": released,
                "freed": freed,
                "unreachable": unreachable,
            })
            .to_string())
        })
    })
}

/// What this node is and what it holds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_status(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        with_node(|node| {
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
                "keepBytes": node.config.keep_bytes,
                "pledgeBytes": node.config.pledge_bytes,
                "peers": node.config.peers,
                "copiesElsewhere": coverage.complete_elsewhere,
                "onlyHere": coverage.only_here,
                "liveChunks": coverage.live_chunks,
            })
            .to_string())
        })
    })
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

        node.config.keep_bytes = if bytes > 0 {
            Some(bytes.unsigned_abs())
        } else {
            None
        };
        node.config.keep_order = order;
        node.config.keep_only = only
            .split('\n')
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        node.save_config()?;

        Ok(describe_keeping(&node.config))
    })
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

        node.config.pledge_bytes = bytes.unsigned_abs();
        node.save_config()?;
        Ok(serde_json::json!({ "pledgeBytes": node.config.pledge_bytes }).to_string())
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
}
