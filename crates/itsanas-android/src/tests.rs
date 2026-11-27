//! The boundary itself: the names Kotlin reads, the native calls on both sides,
//! and the coordinator a phone node joins. The rest, by what it covers, in
//! `tests/`.

mod limits;
mod logout;
mod rounds;
mod status;
mod unlock;
mod writes;

use super::*;
use itsanas_cli::account::{logout_check, logout_into, restore_account};
use itsanas_cli::limits::set_limits_on;
use itsanas_cli::testing::{NOW, PASSPHRASE, live, phone, serving, serving_on, with_directory};
use itsanas_node::NodeError;
use itsanas_node::coordinator;

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

const GIB: u64 = 1 << 30;
const NEW_PASSPHRASE: &str = "a new passphrase for this phone";

fn network_at(address: std::net::SocketAddr) -> Network {
    Network {
        address: address.to_string(),
        device: None,
    }
}

fn answer(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("JSON")
}

fn gib(n: i64) -> i64 {
    n * (1 << 30)
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
/// `Java_fr_ngas_itsanas_Native_passphraseRule`.
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
        declared.contains(&"passphraseRule"),
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
        NOW,
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
    for (what, node) in [("unlocked", &**opened), ("restored", &restored)] {
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
