//! What an idle sync costs a phone on battery and a metered plan: the bytes
//! on the wire, coordinator included, and the memory kept, over many rounds
//! with nothing to do. A leak that grows a little each round, or a round that
//! re-sends what nothing asked for, never shows in a test of one round; it
//! shows on the phone's bill and in its battery, weeks later.
//!
//! The phone, the computer it syncs with and the coordinator run in this one
//! process, so memory either of them keeps per round counts here. Resident
//! memory is read from `/proc` (Linux, where CI runs the suite); elsewhere the
//! memory half is skipped and the wire half still runs.

#![cfg(feature = "testing")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use itsanas_cli::phone::{put_file, sync_node, unix_now};
use itsanas_cli::testing;

/// Rounds before measuring: the first ones enrol, exchange claims and fill
/// caches, which is work, not idleness.
const WARM_ROUNDS: usize = 12;
/// Rounds measured.
const ROUNDS: usize = 120;
/// Bytes on the wire, both ways and both links, that one idle round may
/// cost. Measured 2026-11-23: see `docs/TESTING.md`.
const BYTES_PER_IDLE_ROUND: u64 = 20 * 1024;
/// The same on a metered connection, where only the log goes (`metadata_only`).
const BYTES_PER_METERED_ROUND: u64 = 8 * 1024;
/// Metered rounds measured.
const METERED_ROUNDS: usize = 20;
/// Resident memory the measured rounds may add, all of it: allocator noise
/// and caches that settle, never a few KB per round (a 16 KB-a-round leak
/// is 1.9 MB here).
const MEMORY_GROWTH: u64 = 1024 * 1024;

/// A relay to `target` that counts every byte it carries, both ways.
fn metered(target: SocketAddr) -> (SocketAddr, Arc<AtomicU64>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the relay");
    let at = listener.local_addr().expect("relay address");
    let bytes = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&bytes);
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let Ok(server) = TcpStream::connect(target) else {
                continue;
            };
            let (Ok(client_in), Ok(server_in)) = (client.try_clone(), server.try_clone()) else {
                continue;
            };
            for (from, to) in [(client_in, server), (server_in, client)] {
                let counted = Arc::clone(&counted);
                std::thread::spawn(move || pipe(from, to, &counted));
            }
        }
    });
    (at, bytes)
}

fn pipe(mut from: TcpStream, mut to: TcpStream, counted: &AtomicU64) {
    let mut buffer = [0u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer) {
        if read == 0 || to.write_all(&buffer[..read]).is_err() {
            break;
        }
        counted.fetch_add(read as u64, Ordering::Relaxed);
    }
    let _ = to.shutdown(std::net::Shutdown::Both);
}

/// This process's resident memory, where the platform says it simply.
fn resident() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

#[test]
fn an_idle_phone_sync_costs_few_bytes_and_keeps_no_memory() {
    testing::with_directory(|coordinator, _| {
        let dir = tempfile::tempdir().expect("tempdir");
        let (desk, phrase) = testing::phone(&dir.path().join("desk"), coordinator, "camille", None);
        let (mut phone, _) = testing::phone(
            &dir.path().join("phone"),
            coordinator,
            "camille",
            Some(&phrase),
        );
        let source = dir.path().join("photo.jpg");
        std::fs::write(&source, vec![7u8; 200_000]).expect("source");
        put_file(&desk, "photo.jpg", source.to_str().expect("utf-8")).expect("put");
        let (to_coordinator, coordinator_bytes) = metered(coordinator);
        phone.config.coordinator = Some(to_coordinator.to_string());

        testing::serving(&desk, |at| {
            let (to_desk, desk_bytes) = metered(at);
            phone.config.peers.push(to_desk.to_string());
            let wire =
                || coordinator_bytes.load(Ordering::Relaxed) + desk_bytes.load(Ordering::Relaxed);
            for _ in 0..WARM_ROUNDS {
                sync_node(&phone, false, unix_now()).expect("a warm-up round");
            }
            let (bytes_before, memory_before) = (wire(), resident());
            for round in 0..ROUNDS {
                let answer = sync_node(&phone, false, unix_now()).expect("an idle round");
                let said: serde_json::Value = serde_json::from_str(&answer).expect("json");
                assert_eq!(
                    (said["sent"].as_u64(), said["adopted"].as_u64()),
                    (Some(0), Some(0)),
                    "idle round {round} moved content with nothing changed: {answer}"
                );
            }
            let per_round = (wire() - bytes_before) / ROUNDS as u64;
            let metered_before = wire();
            for _ in 0..METERED_ROUNDS {
                sync_node(&phone, true, unix_now()).expect("an idle metered round");
            }
            let per_metered = (wire() - metered_before) / METERED_ROUNDS as u64;
            println!("idle round: {per_round} B on Wi-Fi, {per_metered} B metered");
            for (cost, budget, how) in [
                (per_round, BYTES_PER_IDLE_ROUND, "on Wi-Fi"),
                (per_metered, BYTES_PER_METERED_ROUND, "metered"),
            ] {
                assert!(
                    cost <= budget,
                    "an idle sync {how} costs {cost} bytes a round, over {budget}: a phone syncing every 15 minutes pays {} MB a month for nothing",
                    cost * 4 * 24 * 30 / 1_000_000
                );
            }
            if let (Some(before), Some(after)) = (memory_before, resident()) {
                let grown = after.saturating_sub(before);
                println!("{ROUNDS} idle rounds: resident memory grew {grown} B");
                assert!(
                    grown <= MEMORY_GROWTH,
                    "{ROUNDS} idle rounds kept {grown} bytes of memory, over {MEMORY_GROWTH}: a leak of about {} B a round, which a phone or a Pi pays until it runs out",
                    grown / ROUNDS as u64
                );
            }
        });
    });
}
