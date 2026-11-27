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

mod folder;
mod host;
mod page;
#[cfg(test)]
mod tests;
mod update;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use folder::{folder_import, folder_wanted};
use itsanas_cli::phone::{
    bring_back, get_file, list_at, network_in, open_into, passphrase_rule, put_file, rename,
    status_of, sync_node, trash_delete, trash_keep, trash_list, trash_put, use_coordinator,
    versions_of,
};
use itsanas_cli::shell::{Answer, Failure, Network, check_words};
#[cfg(test)]
use itsanas_cli::{
    phone::refresh_old_pledges,
    shell::{run_round, targets},
};
use itsanas_node::Node;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jstring};
use page::{GENERATION, start_page, stop_page};
use update::{apk_matches, release_failure, update_offer};

/// The node this application has open, if any.
///
/// A process holds at most one. Android will happily start a second activity
/// against the same files, and the store's single-writer rule would then
/// surface as "already open in another process" — from *this* process, which
/// reads as a bug in the storage engine rather than as what it is.
static NODE: Mutex<Option<Arc<Node>>> = Mutex::new(None);

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
        open_into(
            &NODE,
            &home?,
            &passphrase?,
            &network()?,
            itsanas_discover::now_unix(),
        )
    })
}

/// Close the node, releasing the store for another process.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_close(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        // The loop holds the node too: it ends before the node can close.
        itsanas_cli::shell::host_stop();
        let mut guard = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
        *guard = None;
        Ok("{}".to_owned())
    })
}

/// Bring the hosting loop in line with the shared policy, from what the phone
/// can see. The loop is the daemon's own; the decision is `itsanas_policy`'s.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_hostTick(
    mut env: JNIEnv,
    _class: JClass,
    metered: jboolean,
    charging: jboolean,
    percent: jni::sys::jint,
    content_on_metered: jboolean,
    metered_used: jni::sys::jlong,
) -> jstring {
    answer(&mut env, move || {
        let node = NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?
            .clone()
            .ok_or_else(|| Failure::Usage("no account is open".to_owned()))?;
        let settings = node.config.hosting_settings(content_on_metered != 0);
        let conditions = itsanas_policy::Conditions {
            network: if metered != 0 {
                itsanas_policy::Network::Metered
            } else {
                itsanas_policy::Network::Unmetered
            },
            power: itsanas_policy::Power::of(
                u8::try_from(percent).unwrap_or(100),
                charging != 0,
                settings.battery_floor_percent,
            ),
            attention: itsanas_policy::Attention::Background,
        };
        let mut host = itsanas_cli::shell::HOST_RUN
            .lock()
            .map_err(|_| Failure::Usage("the hosting lock was poisoned by a panic".to_owned()))?;
        let plan = itsanas_cli::shell::host_tick(
            &mut host,
            &node,
            conditions,
            settings,
            u64::try_from(metered_used).unwrap_or(0),
            true,
        );
        Ok(serde_json::json!({
            "running": itsanas_cli::shell::host_running(&host),
            "because": plan.because,
            "periodSeconds": settings.metered_period.seconds(),
        })
        .to_string())
    })
}

/// Stop the hosting loop and wait for it: the service that kept the process
/// alive is going.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_hostStop(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || {
        itsanas_cli::shell::host_stop();
        Ok("{}".to_owned())
    })
}

/// The owner's battery floor, or the default with no account open.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_batteryFloor(
    _env: JNIEnv,
    _class: JClass,
) -> jni::sys::jint {
    NODE.lock()
        .ok()
        .and_then(|node| node.as_ref().map(|node| node.config.battery_floor_percent))
        .map_or(
            jni::sys::jint::from(itsanas_policy::Settings::DEFAULT.battery_floor_percent),
            jni::sys::jint::from,
        )
}

/// Set by an instrumented test, and nothing else: see [`use_coordinator`].
static TEST_NETWORK: Mutex<Option<Network>> = Mutex::new(None);

/// The coordinator this process's nodes belong to: the official one, unless an
/// instrumented test said otherwise.
fn network() -> Result<Network, Failure> {
    network_in(&TEST_NETWORK)
}

// -------------------------------------------------------------------- page

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
        let message = itsanas_cli::shell::forgot(
            &home?,
            &mut host::PhonePrompt::default(),
            &host::PhoneService,
        )?;
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
    answer(&mut env, || with_node(itsanas_cli::phone::list_all))
}

/// One folder of the account: the folders in it and the files directly in it.
///
/// The folder answered may be above the one asked for, when the last file below
/// it is gone ([`itsanas_cli::file_tree`]).
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_listAt(
    mut env: JNIEnv,
    _class: JClass,
    folder: JString,
) -> jstring {
    let folder = text(&mut env, &folder, "the folder");
    answer(&mut env, move || {
        let folder = folder?;
        with_node(|node| list_at(node, &folder))
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

/// Let this phone's copy of a file go, once other machines have proved they
/// hold it ([`itsanas_cli::freeing::release_here`]).
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_release(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    answer(&mut env, move || {
        let path = path?;
        with_node(|node| itsanas_cli::freeing::release_here(node, &path))
    })
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

/// Which files of the chosen folder the application should copy
/// (`phone_folder.rs`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_folderWanted(
    mut env: JNIEnv,
    _class: JClass,
    listing: JString,
) -> jstring {
    let listing = text(&mut env, &listing, "the listing");
    answer(&mut env, move || {
        let listing = listing?;
        with_node(|node| folder_wanted(node, &listing))
    })
}

/// Put a batch of copied files of the folder into the account, then record them.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_folderImport(
    mut env: JNIEnv,
    _class: JClass,
    batch: JString,
) -> jstring {
    let batch = text(&mut env, &batch, "the batch");
    answer(&mut env, move || {
        let batch = batch?;
        with_node(|node| folder_import(node, &batch))
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
            let removed = itsanas_store::remove_known(&node.store, &node.vault, &path)?;
            if removed {
                node.store.flush_segment()?;
            }
            Ok(serde_json::json!({ "removed": removed }).to_string())
        })
    })
}

// -------------------------------------------------------------------- trash

/// Give a file a new name, or a folder a new place, on every machine.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_rename(
    mut env: JNIEnv,
    _class: JClass,
    from: JString,
    to: JString,
) -> jstring {
    let from = text(&mut env, &from, "the old name");
    let to = text(&mut env, &to, "the new name");
    answer(&mut env, move || {
        let (from, to) = (from?, to?);
        with_node(|node| rename(node, &from, &to))
    })
}

/// Every version of a file the account knows, newest first.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_versions(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    answer(&mut env, move || {
        let path = path?;
        with_node(|node| versions_of(node, &path))
    })
}

/// Make one version of a file (0 is the newest) the file again, everywhere.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_bringBack(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    at: jni::sys::jint,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    answer(&mut env, move || {
        let path = path?;
        let at = usize::try_from(at)
            .map_err(|_| Failure::Usage("a version is counted from 0".to_owned()))?;
        with_node(|node| bring_back(node, &path, at))
    })
}

/// Put a file in this phone's trash for some days.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_trashPut(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    days: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    let days = text(&mut env, &days, "the days");
    answer(&mut env, move || {
        let (path, days) = (path?, days?);
        let days = days
            .parse()
            .map_err(|_| Failure::Usage("the days are a whole number".to_owned()))?;
        with_node(|node| trash_put(node, &path, days))
    })
}

/// What is in the trash, and what left the chosen folder.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_trashList(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || with_node(trash_list))
}

/// What is in the trash, from its record alone: a round holds the node for as
/// long as it takes, and the screen said the trash was empty meanwhile.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_trashRecord(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
) -> jstring {
    let home = text(&mut env, &home, "the home");
    answer(&mut env, move || {
        itsanas_cli::phone::trash_record(Path::new(&home?))
    })
}

/// Take a file out of the trash, wherever it waits: the account's trash brings
/// it back on every machine, so the node is needed. `home` is kept for the
/// app's call.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_trashRestore(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    path: JString,
) -> jstring {
    let _ = text(&mut env, &home, "the home");
    let path = text(&mut env, &path, "the path");
    answer(&mut env, move || {
        let path = path?;
        with_node(|node| itsanas_cli::phone::trash_restore_in(node, &path))
    })
}

/// Keep a file that left the folder: it stays in the account, unasked. Its
/// record alone, like [`Java_fr_ngas_itsanas_Native_trashRestore`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_trashKeep(
    mut env: JNIEnv,
    _class: JClass,
    home: JString,
    path: JString,
) -> jstring {
    let home = text(&mut env, &home, "the home");
    let path = text(&mut env, &path, "the path");
    answer(&mut env, move || {
        let (home, path) = (home?, path?);
        trash_keep(Path::new(&home), &path)
    })
}

/// Delete a file from every machine, whether trashed or gone from the folder.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_trashDelete(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jstring {
    let path = text(&mut env, &path, "the path");
    answer(&mut env, move || {
        let path = path?;
        with_node(|node| trash_delete(node, &path))
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

/// What this node is and what it holds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_ngas_itsanas_Native_status(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    answer(&mut env, || with_node(status_of))
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
            ..itsanas_policy::Settings::default()
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
