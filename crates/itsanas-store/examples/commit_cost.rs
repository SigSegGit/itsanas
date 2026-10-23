//! What a redb commit costs the disk, measured: the reason an idle round is
//! cut by its number of commits and not by what each one holds.
//!
//!     cargo run --release -p itsanas-store --example commit_cost
//!
//! Linux only (`/proc/self/io`). On 2026-10-23, against a 5000-row table:
//! one small write per commit cost 24 KB each, durable or not
//! (`Durability::None` saved nothing: the pages are written either way); the
//! same 200 writes in commits of 7 cost 1.4 MB instead of 4.8, and in one
//! commit 0.6 MB. ROADMAP, "An idle node's disk writes".

use redb::{Database, Durability, TableDefinition};

const ROWS: TableDefinition<u64, &[u8]> = TableDefinition::new("rows");
const WRITES: u64 = 200;

fn written() -> u64 {
    std::fs::read_to_string("/proc/self/io")
        .ok()
        .and_then(|io| {
            io.lines()
                .find_map(|line| line.strip_prefix("write_bytes: ")?.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn sync() {
    let _ = std::process::Command::new("sync").status();
}

/// `WRITES` single-row writes, `per_commit` to a transaction, durable or not.
fn measure(per_commit: u64, durable: bool) -> u64 {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Database::create(dir.path().join("db.redb")).expect("create");
    let fill = db.begin_write().expect("write");
    {
        let mut rows = fill.open_table(ROWS).expect("table");
        for key in 0..5000u64 {
            rows.insert(key, &[7u8; 64][..]).expect("insert");
        }
    }
    fill.commit().expect("commit");
    sync();
    let before = written();
    let mut done = 0;
    while done < WRITES {
        let mut txn = db.begin_write().expect("write");
        if !durable {
            txn.set_durability(Durability::None);
        }
        {
            let mut rows = txn.open_table(ROWS).expect("table");
            for _ in 0..per_commit.min(WRITES - done) {
                rows.insert((done * 37) % 5000, &[1u8; 64][..])
                    .expect("insert");
                done += 1;
            }
        }
        txn.commit().expect("commit");
    }
    sync();
    written().saturating_sub(before)
}

fn main() {
    for (per_commit, durable) in [(1, true), (1, false), (7, true), (WRITES, true)] {
        let bytes = measure(per_commit, durable);
        println!(
            "{WRITES} writes, {per_commit} per commit{}: {} KB",
            if durable { "" } else { ", not durable" },
            bytes / 1024
        );
    }
}
