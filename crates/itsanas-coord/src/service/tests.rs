//! The service's unit tests, one module per concern, and the helpers they
//! share.

use super::*;
use crate::claim::{NodeClaim, Presence};

mod peers;
mod presence;
mod probe;
mod standing;
mod strangers;

/// A fixed wall clock, so nothing in these tests depends on today's date.
const NOW: u64 = 1_700_000_000;

fn directory() -> Directory {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("directory.redb");
    // The directory owns its file; the handle keeps the directory alive for
    // the length of the test by leaking it, which is what a test process
    // exiting does anyway.
    std::mem::forget(dir);
    Directory::open(path).expect("directory")
}

fn user(seed: u8) -> itsanas_crypto::UserKeys {
    itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([seed; 32]))
}

fn device(seed: u8) -> itsanas_crypto::DeviceKeys {
    itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]))
}

fn enrol(
    directory: &Directory,
    owner: &itsanas_crypto::UserKeys,
    device: &itsanas_crypto::DeviceKeys,
) {
    directory
        .register(
            &crate::directory::Registration {
                username: format!("member{}", device.device_id().short()),
                user: owner.public(),
                issued_unix: NOW,
            }
            .sign(owner),
            NOW,
        )
        .expect("register");
    directory
        .claim(
            &NodeClaim {
                owner: owner.user_id(),
                device: device.device_id(),
                pledged_bytes: 1 << 30,
                issued_unix: NOW,
                revoked: false,
            }
            .sign(owner),
            NOW,
        )
        .expect("claim");
}

fn announce_address(
    directory: &Directory,
    _owner: &itsanas_crypto::UserKeys,
    device: &itsanas_crypto::DeviceKeys,
    address: &str,
) {
    directory
        .announce(
            &Presence {
                device: device.device_id(),
                address: address.to_owned(),
                at_unix: NOW,
                head: 0,
            }
            .sign(device),
            NOW,
        )
        .expect("announce");
}

fn ask(service: &CoordService<'_>, request: &Request, caller: DeviceId) -> Response {
    ask_at(service, request, caller, NOW)
}

fn ask_at(service: &CoordService<'_>, request: &Request, caller: DeviceId, now: u64) -> Response {
    let limiter = Mutex::new(EscrowLimiter::new());
    service
        .handle(request, caller, now, &limiter, Instant::now())
        .unwrap()
}

fn claim_of(
    owner: &itsanas_crypto::UserKeys,
    device: &itsanas_crypto::DeviceKeys,
    revoked: bool,
    at: u64,
) -> crate::claim::SignedClaim {
    NodeClaim {
        owner: owner.user_id(),
        device: device.device_id(),
        pledged_bytes: 1 << 30,
        issued_unix: at,
        revoked,
    }
    .sign(owner)
}

/// One escrow limiter is shared by every connection. Locked around every
/// request, one slow write held up every other connection's lookups, and
/// a panic while it was held dropped every request until a restart.
/// Sabotage: lock the limiter at the top of `handle`; refuse on a
/// poisoned lock.
#[test]
fn a_held_or_poisoned_escrow_limiter_holds_up_no_other_request() {
    let directory = directory();
    let service = CoordService::new(&directory);
    let caller = device(50).device_id();
    let limiter = Mutex::new(EscrowLimiter::new());
    let (service, limiter) = (&service, &limiter);

    std::thread::scope(|scope| {
        let held = limiter.lock().expect("lock");
        let (tx, rx) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let peers = Request::Peers {
                user: user(50).user_id(),
            };
            let answer = service.handle(&peers, caller, NOW, limiter, Instant::now());
            let _ = tx.send(answer.is_ok());
        });
        let answered = rx.recv_timeout(Duration::from_secs(5));
        drop(held);
        assert_eq!(
            answered,
            Ok(true),
            "a lookup waited for another connection's escrow fetch"
        );
    });

    let poisoned = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _held = limiter.lock();
                panic!("a request panicked while it held the limiter");
            })
            .join()
    });
    assert!(poisoned.is_err() && limiter.is_poisoned());
    let fetch = Request::GetEscrow {
        username: "anyone".to_owned(),
    };
    let answer = service
        .handle(&fetch, caller, NOW, limiter, Instant::now())
        .expect("answer");
    assert!(
        matches!(answer, Response::Escrow(_)),
        "a panic elsewhere left recovery refused until a restart: {answer:?}"
    );
}
