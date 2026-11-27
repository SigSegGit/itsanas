//! End-to-end tests for the local store.
//!
//! These are the M2 exit criteria from `docs/ROADMAP.md`, plus the adversarial
//! cases that would make the guarantee in `README.md` a lie if they regressed.
//! Every test here is meant to fail loudly for a specific, nameable reason —
//! if one of them starts passing for a different reason than it was written
//! for, it is a bad test and should be rewritten.

use std::path::Path;

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::{ChunkerConfig, Store};

mod budget;
mod collection;
mod corpus;
mod durability;
mod holdings;
mod integrity;
mod listing;
mod log;
mod opening;
mod release;

/// Open a store, permitting the published fixture identities.
///
/// Most of these tests deliberately use Alice and Bob, whose recovery phrases
/// are printed in the documentation. `Store::open` refuses those identities —
/// see `the_published_test_identities_are_refused_by_the_normal_constructor` —
/// so the fixtures go through the explicitly-named testing constructor.
fn store_for(master: &MasterSecret, root: &Path) -> Store {
    Store::open_for_testing(
        root,
        UserKeys::derive(master),
        DeviceKeys::generate().expect("device key"),
        ChunkerConfig::default(),
    )
    .expect("store opens")
}
