//! Every rule about what a peer may and may not obtain, tested without a
//! network. The fixtures live here; the tests sit in one file per concern.

use super::storing::overage;
use super::*;

use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_store::ChunkerConfig;

use crate::protocol::challenge_holds;

struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
}

fn node(master: &MasterSecret, device_seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(master),
        DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32])),
        ChunkerConfig::default(),
    )
    .expect("store");
    let vault = Vault::open(dir.path().join("vault")).expect("vault");

    Node {
        _dir: dir,
        store,
        vault,
    }
}

fn service(node: &Node) -> PeerService<'_> {
    PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1)).with_owners(&Unscoped)
}

fn alice() -> MasterSecret {
    MasterSecret::from_bytes([0xA1; 32])
}

mod access;
mod accounts;
mod answers;
mod notices;
mod storing;
