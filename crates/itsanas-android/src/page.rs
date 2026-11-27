//! The setup engine's page, served on 127.0.0.1 for the app's web view: which
//! page is current, and how to stop it.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use itsanas_cli::shell::{Answer, Backends, Failure, Held, Mode, SecretPrompt, Server, Stop};

use crate::{NODE, host, network};

/// Nobody touched the page for this long: it stops, as a computer's does, and
/// the app starts it again when the person comes back. Half an hour, because a
/// phone is picked up and put down all day.
const PAGE_IDLE: Duration = Duration::from_secs(30 * 60);

/// How to stop the page that is being served, if one is.
static PAGE: Mutex<Option<Stop>> = Mutex::new(None);

/// Which page is current. A page stopped by the app, or replaced by a new one,
/// ends without a word: its `closed` would otherwise be read as a page that
/// ended by itself, and the app would start another, for ever.
pub(crate) static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(super) fn start_page(home: &Path, mode: &str, biometric: bool) -> Answer {
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

pub(super) fn stop_page() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let stop = PAGE.lock().ok().and_then(|mut page| page.take());
    if let Some(stop) = stop {
        stop.stop();
    }
}
