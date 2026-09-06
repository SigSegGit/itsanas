//! What survives the power going out.
//!
//! MVP acceptance test J, for the half of it a test can reach.
//!
//! # What this proves, and what it cannot
//!
//! It kills the process mid-write, a dozen times, at measured points inside a
//! real write. That covers what actually kills processes in practice: an
//! out-of-memory kill, a panic, a force-quit, a container being reaped. After
//! every one of them the store must still be readable and must need no repair.
//!
//! **It says nothing about a power cut.** `TerminateProcess` and `SIGKILL`
//! discard the *process*, not the operating system's page cache — bytes the
//! process wrote are already the kernel's problem and the kernel is still
//! running. Only losing power, or the kernel itself, discards those.
//!
//! That was checked rather than assumed: the whole suite was run again with
//! `blob.rs`'s per-chunk `sync_all` removed, and passed identically. So this
//! test cannot distinguish a store that flushes from one that does not, and
//! **it does not settle whether that `fsync` earns its cost** — a factor of two
//! on write throughput, measured by `itsanas bench`.
//!
//! The experiment that would settle it takes ten seconds and a plug: start a
//! large write on the Raspberry Pi, pull the power, boot, and run
//! `itsanas doctor --deep`. Recorded in `docs/MVP.md` as the one measurement
//! nobody can make from a laptop.
//!
//! # The invariant
//!
//! After any crash, at any moment, one thing must hold:
//!
//! > **A file the store lists can be read back and matches its recorded hash.**
//!
//! A file that never appeared is fine — the write did not finish, and no
//! caller was told otherwise. Orphaned blobs are fine and expected: a crash
//! between writing a chunk and committing its index entry leaves bytes nobody
//! references, which garbage collection reclaims. What must never happen is a
//! *listed* file whose content is missing or wrong, because that is the store
//! lying about what it holds.
//!
//! # Why this is `#[ignore]`d
//!
//! Every invocation pays a full production Argon2id derivation — about four
//! seconds in a debug build — and this spawns a dozen. Too slow for every
//! push, too important never to run, which is exactly what the `slow-tests`
//! job exists for.
//!
//! ```bash
//! cargo test --workspace --all-features -- --ignored
//! ```

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How many sealed chunks are on disk right now.
///
/// Used to tell "the process died during key derivation" from "the process
/// died mid-write", which is the difference between this test meaning
/// something and not.
fn count_blobs(root: &Path) -> usize {
    fn walk(directory: &Path, found: &mut usize) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "blob") {
                *found += 1;
            }
        }
    }
    let mut found = 0;
    walk(root, &mut found);
    found
}

const PASSPHRASE: &str = "a passphrase for the crash tests";

/// Where in a write to kill, as a fraction of one measured, uncut run.
///
/// **Measured rather than guessed, and the first version of this test guessed.**
/// It killed at fixed millisecond delays, passed, and proved nothing: every
/// invocation pays a full production Argon2id derivation first — around four
/// seconds in a debug build, under one in release — so all eight kills landed
/// during key derivation, before the store had been opened. Zero blobs were
/// written in any round. A green test that exercised nothing.
///
/// So the run is timed once, and the kills are placed as fractions of it.
/// Fractions above the derivation share land in the part that matters:
/// between writing a chunk and committing its index entry, and between
/// committing and sealing the log segment. Neither happens at a predictable
/// moment, which is why there are several.
///
/// `saw_partial_work` below is the guard that stops this going quietly vacuous
/// again if the timings shift.
const KILL_AT: [f64; 10] = [0.55, 0.68, 0.76, 0.82, 0.86, 0.90, 0.93, 0.96, 0.98, 1.02];

fn itsanas(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_itsanas"));
    command
        .arg("--home")
        .arg(home)
        .env("ITSANAS_PASSPHRASE", PASSPHRASE)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Deterministic, incompressible bytes — the pessimistic case for chunking.
fn corpus(bytes: usize, salt: u8) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"itsanas crash corpus");
    hasher.update(&[salt]);
    let mut out = vec![0u8; bytes];
    hasher.finalize_xof().fill(&mut out);
    out
}

/// How slow one whole write has to be before a kill can land inside it.
///
/// Below this the kill fractions all land after the write has finished and the
/// test proves nothing, which is what the assertion at the end of `calibrate`
/// refuses to let happen.
const FLOOR: Duration = Duration::from_millis(400);

/// The largest payload worth trying before giving up and saying so.
const CAP: usize = 48_000_000;

/// Find a payload big enough that one write takes long enough to interrupt,
/// and time it.
///
/// The size used to be the constant 3 MB, picked against a debug build. Then
/// this test was moved to a release build to fit a one-minute budget, a whole
/// write on a CI runner finished in under 200 ms, and the guard below --
/// which exists precisely to stop this test going vacuous -- refused to
/// continue. It was right to. The number was never about bytes: it is about a
/// write being slow enough that a kill can land inside it, and that depends on
/// the machine and the optimisation level, neither of which a constant knows
/// about.
///
/// Each attempt uses a different salt. With the same salt a larger corpus
/// shares its opening chunks with the previous one, they deduplicate, and the
/// write gets *faster* as the file grows -- which would drive this loop to the
/// cap on a machine that never needed to get there.
fn calibrate(home: &Path, scratch: &Path) -> (usize, Duration) {
    let timing_source = scratch.join("timing.bin");
    let mut payload = 3_000_000usize;
    let mut salt = 90u8;

    let whole_write = loop {
        std::fs::write(&timing_source, corpus(payload, salt)).expect("write timing source");
        let started = Instant::now();
        assert!(
            itsanas(home)
                .args(["put", &format!("timing/reference-{payload}.bin")])
                .arg(&timing_source)
                .status()
                .expect("run put")
                .success()
        );
        let elapsed = started.elapsed();
        if elapsed >= FLOOR || payload >= CAP {
            break elapsed;
        }
        payload *= 2;
        salt = salt.wrapping_add(1);
    };

    assert!(
        whole_write >= FLOOR,
        concat!(
            "even at {} bytes a whole write takes {:?}, under the {:?} this test ",
            "needs in order to place a kill inside one. The kill fractions would ",
            "all land after the write had finished, and this would prove nothing."
        ),
        payload,
        whole_write,
        FLOOR
    );

    (payload, whole_write)
}

/// Start one write, kill it partway through, and check what survived.
///
/// Returns whether chunks reached the disk before the kill — the difference
/// between interrupting real work and interrupting a key derivation.
fn kill_one_write(
    home: &Path,
    scratch: &Path,
    blobs: &Path,
    round: usize,
    delay: Duration,
    payload: usize,
) -> bool {
    let before = count_blobs(blobs);

    let source = scratch.join(format!("victim-{round}.bin"));
    std::fs::write(
        &source,
        corpus(payload, u8::try_from(round + 1).unwrap_or(255)),
    )
    .expect("write source");

    let mut child = itsanas(home)
        .args(["put", &format!("victims/round-{round}.bin")])
        .arg(&source)
        .spawn()
        .expect("spawn put");

    std::thread::sleep(delay);
    // A hard kill: TerminateProcess on Windows, SIGKILL elsewhere. No
    // unwinding, no destructors, no flush — the closest a test gets to the
    // power going out.
    let _ = child.kill();
    let _ = child.wait();

    let interrupted_real_work = count_blobs(blobs) > before;

    // `doctor --deep` reassembles every file and re-hashes it, which is the
    // only check that would notice a chunk that is present but wrong.
    let doctor = itsanas(home)
        .args(["doctor", "--deep"])
        .stdout(Stdio::piped())
        .output()
        .expect("run doctor");
    let report = String::from_utf8_lossy(&doctor.stdout);

    // Orphaned chunks are the *expected* result of a crash and are not a
    // failure — an early version of this test treated any non-zero exit as
    // damage, which is how it found that `doctor` was reporting them as one.
    // What must never appear is a file the store lists and cannot read.
    // `concat!` rather than a backslash continuation, because fmt eats those and
    // both of these messages were already damaged. Note that it costs the
    // implicit captures: a format string produced by a macro cannot name
    // `delay` and `report` from the surrounding scope, so they are passed.
    assert!(
        !report.contains("failed verification"),
        concat!(
            "a kill at {:?} left a file the store lists whose content ",
            "does not match its recorded hash:\n{}"
        ),
        delay,
        report
    );
    assert!(
        !report.contains("referenced but missing"),
        concat!(
            "a kill at {:?} left an index entry pointing at chunks that ",
            "are not on disk:\n{}"
        ),
        delay,
        report
    );
    assert!(
        !report.contains("has a gap"),
        "a kill at {delay:?} broke the operation log chain:
{report}"
    );
    assert!(
        doctor.status.success(),
        "a kill at {delay:?} left damage `doctor` calls a problem:
{report}"
    );

    interrupted_real_work
}

#[test]
#[ignore = "spawns a dozen processes, each paying a full Argon2id derivation"]
fn a_store_killed_mid_write_never_lists_a_file_it_cannot_read() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("home");

    assert!(
        itsanas(&home)
            .args(["init", "--username", "crashtest"])
            .status()
            .expect("run init")
            .success(),
        "could not create an account to crash"
    );

    // One file that is already there and must still be there afterwards. If a
    // crash during an unrelated write can damage it, that is far worse than a
    // half-written new file.
    let bystander = dir.path().join("bystander.bin");
    std::fs::write(&bystander, corpus(400_000, 0)).expect("write bystander");
    assert!(
        itsanas(&home)
            .args(["put", "safe/bystander.bin"])
            .arg(&bystander)
            .status()
            .expect("run put")
            .success()
    );

    let (payload, whole_write) = calibrate(&home, dir.path());

    let blobs = home.join("store").join("blobs");
    let mut saw_partial_work = false;

    for (round, fraction) in KILL_AT.iter().enumerate() {
        if kill_one_write(
            &home,
            dir.path(),
            &blobs,
            round,
            whole_write.mul_f64(*fraction),
            payload,
        ) {
            saw_partial_work = true;
        }
    }

    assert!(
        saw_partial_work,
        "not one of {} kills interrupted a write — every round died before any \
         chunk reached the disk, so this test proved nothing. That is exactly \
         how the first version of it passed.",
        KILL_AT.len()
    );

    // The bystander is intact after the crashes.
    let recovered = dir.path().join("recovered.bin");
    assert!(
        itsanas(&home)
            .args(["get", "safe/bystander.bin"])
            .arg(&recovered)
            .status()
            .expect("run get")
            .success(),
        "a file written before the crashes could not be read after them"
    );
    assert_eq!(
        std::fs::read(&recovered).expect("read back"),
        corpus(400_000, 0),
        "a file written before the crashes came back changed"
    );

    // And the store still accepts new work without anybody repairing it.
    let after = dir.path().join("after.bin");
    std::fs::write(&after, corpus(120_000, 200)).expect("write");
    assert!(
        itsanas(&home)
            .args(["put", "after/ok.bin"])
            .arg(&after)
            .status()
            .expect("run put")
            .success(),
        "the store needed a human before it would take another file"
    );
}
