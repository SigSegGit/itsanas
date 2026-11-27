//! Two nodes, one socket, real bytes.
//!
//! The M4 exit criterion. Everything below the socket is real: real stores,
//! real chunking, real sealing, real signatures, real TCP. The only thing these
//! tests simulate is which machine each node is running on.
//!
//! Each test runs a server in a scoped thread and drives a client from the test
//! thread, so a failure is a normal assertion in the normal place rather than a
//! panic in a detached thread that the harness reports as a hang.

use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_net::{
    Offer, PeerClient, PeerServer, PeerService, Pledge, Refusal,
    protocol::{Request, Response},
    session,
};
use itsanas_store::{AtRisk, ChunkerConfig, Store, Vault};

/// One machine: its own store, and a vault for other people's data.
struct Node {
    dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    device: DeviceKeys,
}

fn node(master: &MasterSecret, device_seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let device = DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32]));
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(master),
        DeviceKeys::from_seed(&device.seed()),
        ChunkerConfig::default(),
    )
    .expect("store");
    let vault = Vault::open(dir.path().join("vault")).expect("vault");

    Node {
        dir,
        store,
        vault,
        device,
    }
}

fn alice() -> MasterSecret {
    MasterSecret::from_bytes([0xA1; 32])
}

/// Stops the server even when the body panics.
///
/// Without it a failing assertion leaves the accept loop running,
/// `thread::scope` never returns, and the harness reports a **hang** instead of
/// the assertion.
///
/// That is worse than it sounds here. Every red-team test in this file runs
/// inside `with_server`, so for as long as this was missing, the tests written
/// to catch an attack reported a timeout when they caught one — and a timeout
/// reads like flakiness, which is the thing everybody retries and nobody
/// investigates. Found by sabotaging a verification step on purpose and
/// watching the suite hang instead of fail.
///
/// `itsanas-coord`'s test harness has had exactly this guard, with exactly this
/// rationale written above it, since the day its server was written. It was
/// never applied here.
struct StopOnDrop<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        // Unblock the accept loop's sleep by connecting once more.
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// Run `body` with `server_node` serving on loopback.
///
/// The server stops as soon as `body` returns, whether it returned or panicked.
fn with_server<T>(
    server_node: &Node,
    pledge: Pledge,
    body: impl FnOnce(std::net::SocketAddr) -> T,
) -> T {
    let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
    let address = server.local_addr().expect("local address");
    let shutdown = AtomicBool::new(false);

    let service = PeerService::new(&server_node.store, &server_node.vault, pledge)
        .with_owners(&itsanas_net::Unscoped);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &server_node.device, &shutdown);
        });

        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

#[test]
fn a_failing_assertion_inside_a_server_scope_fails_rather_than_hangs() {
    // This test passing means it *finished*. There is no assertion that can
    // catch the failure it guards against, because the failure is that nothing
    // returns: `thread::scope` joins the accept loop, the accept loop waits for
    // a shutdown flag that the panic skipped past, and the suite sits there
    // until the harness gives up sixty seconds later and calls it a hang.
    //
    // Every red-team test in this file runs inside `with_server`. For as long
    // as this was broken, a test that caught an attack reported a timeout, and
    // a timeout is what everybody retries and nobody reads.
    let node = node(&alice(), 60);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_server(&node, Pledge::gigabytes(1), |_address| {
            panic!("a failing assertion, as an assertion would");
        })
    }));
    assert!(outcome.is_err(), "the panic did not even reach the caller");
}

/// Pull everything `to` has not seen from `from`.
fn pull_all(to: &Node, from: &Node) {
    with_server(from, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &to.device, to.store.owner(), None).expect("dial");
        session::pull_scoped(
            &to.store,
            &to.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("pull");
    });
}

/// Pull only the log, so `to` lists what `from` has and holds none of it.
fn pull_listing(to: &Node, from: &Node) {
    with_server(from, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &to.device, to.store.owner(), None).expect("dial");
        session::pull_scoped(&to.store, &to.vault, &mut client, session::Scope::Metadata)
            .expect("pull");
    });
}

/// A file large enough to be split into many chunks.
///
/// Single-chunk fixtures make the audit tests pass for the wrong reason: with
/// one record on the ledger, every way of choosing what to challenge picks the
/// same thing, so a selection rule that is broken for a real store looks
/// correct. Twice this happened in this file before anyone noticed.
fn a_file_of_many_chunks(seed: u8, bytes: usize) -> Vec<u8> {
    // Not compressible and not repetitive: a repeating pattern would let the
    // content-defined chunker cut it into a handful of identical pieces, which
    // would collapse back into the single-record case this exists to avoid.
    let mut out = Vec::with_capacity(bytes);
    let mut state = u64::from(seed).wrapping_add(0x9E37_79B9_7F4A_7C15);
    while out.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out
}

mod audit;
mod convergence;
mod edits;
mod exit_criterion;
mod fetching;
mod holders;
mod hostile_hosts;
mod hosting;
mod ledger_walks;
mod metadata;
mod older_peers;
mod reciprocal;
mod recovery;
mod refusals;
mod versions;
