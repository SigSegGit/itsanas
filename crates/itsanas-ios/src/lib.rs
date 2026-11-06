//! The C boundary: everything Swift is allowed to ask the core.
//!
//! The iPhone's twin of `itsanas-android`, and shaped the same way for the same
//! reasons. The calls are whole operations that answer one JSON string; the
//! work behind them is `itsanas_cli::phone`, the very lines Android calls, so a
//! file listed, fetched or added on either phone takes the same path. What is
//! left here is converting C strings and wrapping the answer.
//!
//! **Every answer is an envelope.** `{"ok": "<answer>"}` or
//! `{"error": "<sentence>"}`, where the sentence is the one the command line
//! prints for the same fault. C has no exceptions, and a shell that has to
//! check a return code beside the string will one day forget; the envelope is
//! one string to decode, and the error is in it.
//!
//! **Every string returned is freed once, with [`itsanas_free`].**
//! `include/itsanas.h` says so beside each declaration, and the Swift wrapper
//! (`ios/ITSaNAS/Core.swift`) is the only caller and does it in one place.
//!
//! **One node, behind a lock**, as on Android: the store permits one writer.
//!
//! # Unsafe
//!
//! The entry points carry `#[unsafe(no_mangle)]`, as Android's do, and C
//! strings are read in `c.rs`, the one file `scripts/check-unsafe.py` lets
//! write an `unsafe` block in this crate, each with its `SAFETY:` argument.
#![allow(unsafe_code)]

mod c;
mod host;

use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use itsanas_cli::phone::{
    get_file, list_at, open_into, passphrase_rule, put_file, status_of, sync_node,
};
use itsanas_cli::shell::{
    Answer, Backends, Failure, Held, Mode, Network, SecretPrompt, Server, Stop,
};
use itsanas_node::Node;
use zeroize::Zeroizing;

/// The node this application has open, if any.
static NODE: Mutex<Option<Arc<Node>>> = Mutex::new(None);

/// Run `body` and wrap what it answers. A panic must not cross into Swift,
/// where it is undefined behaviour: it is caught and told as a fault.
fn answer(body: impl FnOnce() -> Answer) -> *mut c_char {
    // `AssertUnwindSafe` for the reason `itsanas-android` gives: the only state
    // shared across the boundary is behind locks whose poisoning is detected.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    c::give(&envelope(outcome))
}

fn envelope(outcome: std::thread::Result<Answer>) -> String {
    match outcome {
        Ok(Ok(text)) => serde_json::json!({ "ok": text }).to_string(),
        Ok(Err(failure)) => serde_json::json!({ "error": failure.message() }).to_string(),
        Err(_) => serde_json::json!({
            "error": "the core panicked; see the device log for the message"
        })
        .to_string(),
    }
}

/// Do something with the open node.
fn with_node<T>(body: impl FnOnce(&Node) -> Result<T, Failure>) -> Result<T, Failure> {
    let guard = NODE
        .lock()
        .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
    let node = guard.as_ref().ok_or(Failure::Closed)?;
    body(node)
}

fn home_of(home: *const c_char) -> Result<PathBuf, Failure> {
    c::text(home, "the home directory").map(PathBuf::from)
}

/// Give back a string an entry point returned. Null is ignored.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_free(answer: *mut c_char) {
    c::free(answer);
}

// ---------------------------------------------------------------- lifecycle

/// Whether a node already exists at `home`: create an account, or unlock one.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_exists(home: *const c_char) -> bool {
    home_of(home).is_ok_and(|home| Node::exists(&home))
}

/// The rule for a new passphrase: `{"min":6,"max":128}`.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_passphrase_rule() -> *mut c_char {
    answer(|| Ok(passphrase_rule()))
}

/// Unlock the node at `home`.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_open(home: *const c_char, passphrase: *const c_char) -> *mut c_char {
    let home = home_of(home);
    let passphrase = c::text(passphrase, "the passphrase").map(Zeroizing::new);
    answer(move || {
        open_into(
            &NODE,
            &home?,
            &passphrase?,
            &Network::official(),
            itsanas_cli::phone::unix_now(),
        )
    })
}

/// Close the node and stop the page, releasing the store.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_close() -> *mut c_char {
    answer(|| {
        stop_page();
        *NODE
            .lock()
            .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))? =
            None;
        Ok("{}".to_owned())
    })
}

// -------------------------------------------------------------------- files

/// One folder of the account: its folders and the files directly in it.
/// `""` is the top.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_list_at(folder: *const c_char) -> *mut c_char {
    let folder = c::text(folder, "the folder");
    answer(move || {
        let folder = folder?;
        with_node(|node| list_at(node, &folder))
    })
}

/// Write one file of the account to `destination`, fetching it first if it is
/// not on this phone.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_get(path: *const c_char, destination: *const c_char) -> *mut c_char {
    let path = c::text(path, "the path");
    let destination = c::text(destination, "the destination");
    answer(move || {
        let (path, destination) = (path?, destination?);
        with_node(|node| get_file(node, &path, &destination))
    })
}

/// Put the file at `source` into the account as `path`.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_put(path: *const c_char, source: *const c_char) -> *mut c_char {
    let path = c::text(path, "the path");
    let source = c::text(source, "the source file");
    answer(move || {
        let (path, source) = (path?, source?);
        with_node(|node| put_file(node, &path, &source))
    })
}

/// One round with the account's machines. `metadata_only` is the metered
/// case: the listing arrives, the contents do not.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_sync(metadata_only: bool) -> *mut c_char {
    answer(move || with_node(|node| sync_node(node, metadata_only, itsanas_cli::phone::unix_now())))
}

/// What this node is and what it holds.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_status() -> *mut c_char {
    answer(|| with_node(status_of))
}

// -------------------------------------------------------------------- page

/// Nobody touched the page for this long: it stops, as Android's does.
const PAGE_IDLE: Duration = Duration::from_secs(30 * 60);

/// How to stop the page that is being served, if one is.
static PAGE: Mutex<Option<Stop>> = Mutex::new(None);

/// Which page is current (see `itsanas-android`): a page stopped or replaced
/// ends without a word, and its engine stops waiting for answers.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Serve the setup engine's page on 127.0.0.1 and answer its address, the key
/// in the fragment. `mode` is `setup` or `settings`. The page's secrets are
/// asked through the mailbox (`itsanas_next_request`).
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_page_start(
    home: *const c_char,
    mode: *const c_char,
    biometric: bool,
) -> *mut c_char {
    let home = home_of(home);
    let mode = c::text(mode, "the mode");
    answer(move || start_page(&home?, &mode?, biometric))
}

fn start_page(home: &Path, mode: &str, biometric: bool) -> Answer {
    let mode = match mode {
        "setup" => Mode::Setup,
        "settings" => Mode::Settings,
        other => return Err(Failure::Usage(format!("no page is called {other}"))),
    };
    stop_page();
    let network = Network::official();
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
                host::tell("event", &["closed", if done { "done" } else { "left" }]);
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

/// The person left the page: the engine stops and the port closes.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_page_stop() -> *mut c_char {
    answer(|| {
        stop_page();
        Ok("{}".to_owned())
    })
}

/// The next question or piece of news from the page, as
/// `{"id":..,"kind":..,"args":[..],"reply":..}`, or `{}` when there is none.
/// The app polls this while the page is on screen.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_next_request() -> *mut c_char {
    answer(|| Ok(host::next_request()))
}

/// Answer question `id`. `text` null means the person went Back. Answers
/// `{"taken":false}` for a question nobody is waiting on any more.
#[unsafe(no_mangle)]
pub extern "C" fn itsanas_reply(id: u64, text: *const c_char) -> *mut c_char {
    let said = (!text.is_null()).then(|| c::text(text, "the answer"));
    answer(move || {
        let said = said.transpose()?.map(Zeroizing::new);
        Ok(serde_json::json!({ "taken": host::reply(id, said) }).to_string())
    })
}

#[cfg(test)]
mod tests;
