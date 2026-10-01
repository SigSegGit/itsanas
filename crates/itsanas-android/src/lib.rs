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
            let incoming = file
                .metadata()
                .map_err(|error| Failure::Usage(format!("could not read {source}: {error}")))?
                .len();
            node.bound_writes()?;
            node.store.check_room(&path, incoming)?;
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

            // What comes in is held to the disk bound (8.1b); without this the
            // ceiling is the one opening the node set, which is none.
            node.bound_writes()?;

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
    Node::check_split(&node.config, node.config.pledge_bytes, keep)
        .map_err(|refusal| phone_split(&refusal))?;

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
    Node::check_split(&node.config, bytes, node.config.keep_bytes)
        .map_err(|refusal| phone_split(&refusal))?;

    node.config.pledge_bytes = bytes;
    node.save_config()?;
    Ok(serde_json::json!({ "pledgeBytes": node.config.pledge_bytes }).to_string())
}

/// Set the pledge and how much to keep here together, checked as a pair.
///
/// The settings screen saves both at once. Saved one after the other, raising
/// both was refused: the new keep was checked against the old pledge, and the
/// person was told to raise a pledge they had just raised. `bytes` of zero or
/// less for `keep` means no limit.
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

/// [`Java_fr_ngas_itsanas_Native_setLimits`] without the JNI. Takes the
/// JVM's signed longs so the sign is checked where a test reaches it: a
/// negative pledge read as its absolute value would offer space nobody asked
/// to give (found by `itsanas-redteam`).
fn set_limits(node: &mut Node, pledge: i64, keep: i64) -> Result<String, Failure> {
    let pledge = u64::try_from(pledge)
        .map_err(|_| Failure::Usage(format!("a pledge cannot be negative ({pledge} bytes)")))?;
    let keep = (keep > 0).then(|| keep.unsigned_abs());
    Node::check_split(&node.config, pledge, keep).map_err(|refusal| phone_split(&refusal))?;

    node.config.pledge_bytes = pledge;
    node.config.keep_bytes = keep;
    node.save_config()?;
    Ok(serde_json::json!({
        "pledgeBytes": node.config.pledge_bytes,
        "keepBytes": node.config.keep_bytes,
    })
    .to_string())
}

/// A split refusal in the phone's words.
///
/// `SplitRefusal`'s own `Display` ends with `itsanas space --pledge ... --apply`,
/// a command the phone does not have; this names the field on the settings
/// screen instead, with the same figures.
fn phone_split(refusal: &itsanas_node::node::SplitRefusal) -> Failure {
    use itsanas_node::config::format_size;
    Failure::Usage(format!(
        "keeping {} on this phone needs at least {} in \"Space you offer other people\", \
         and this phone offers {}. Raise that, or keep less here.",
        format_size(refusal.keep),
        format_size(refusal.needed),
        format_size(refusal.pledge),
    ))
}

/// Point this node at a coordinator, so it can be reached from off the network.
///
/// # Why the phone needed this
///
/// Until 2026-09-16 this crate had no coordinator calls at all, so an Android
/// device could reach the network only through [`Java_fr_ngas_itsanas_Native_addPeer`]
/// — an address typed in by hand — and an account created on a phone was
/// enrolled nowhere. Joining is the one thing a new member has to do, and it
/// was the one thing the phone could not do.
///
/// `device` may be empty, which means "trust whatever answers at that
/// address". It is parsed here rather than at first use so that a mistyped id
/// fails while the person who typed it is still looking at it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_setCoordinator(
    mut env: JNIEnv,
    _class: JClass,
    address: JString,
    device: JString,
) -> jstring {
    let address = text(&mut env, &address, "the coordinator address");
    let device = text(&mut env, &device, "the coordinator device id");

    answer(&mut env, move || {
        let address = address?;
        let device = device?;
        let device = device.trim().to_owned();
        if !device.is_empty() {
            itsanas_node::coordinator::parse_device(&device)?;
        }

        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        let node = guard.as_mut().ok_or(Failure::Closed)?;

        node.config.coordinator = Some(address.clone());
        node.config.coordinator_device = if device.is_empty() {
            None
        } else {
            Some(device)
        };
        node.save_config()?;

        Ok(serde_json::json!({
            "coordinator": node.config.coordinator,
            "pinnedTo": node.config.coordinator_device,
        })
        .to_string())
    })
}

/// Enrol this account and device with the configured coordinator.
///
/// `invite` may be empty. A coordinator running `--invite-only` — which is
/// what this project's own does — refuses an account with no code the first
/// time, and accepts a member re-registering without one for ever after, which
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
        set_limits(&mut node, gib(500), gib(200))
            .expect("raising pledge and keep together was refused");
        assert_eq!(
            (node.config.pledge_bytes, node.config.keep_bytes),
            (500 * GIB, Some(200 * GIB))
        );

        assert!(
            set_limits(&mut node, -gib(5), 0).is_err(),
            "a negative pledge was taken as a positive one"
        );
        let Err(refused) = set_limits(&mut node, gib(1), gib(200)) else {
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
}
