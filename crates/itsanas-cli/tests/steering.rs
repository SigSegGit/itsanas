//! The daemon loop, driven for real: a paused node takes in no file.
//!
//! `control.rs` tests the decisions -- what is due, what a pause allows -- as
//! pure functions. What acts on them is `sync_loop`, and no other test runs
//! it: the guard that keeps a paused daemon away from the folder could be
//! deleted and every unit test would still pass (the CI reviewer's finding on
//! #243, and the gap HANDOVER §8 0w (1) named). This test starts the real
//! binary on a throwaway node and watches what it reports.
//!
//! What it watches is the snapshot the daemon writes every time round its
//! loop (`status.snapshot`, the file `itsanas status` and the tray read): its
//! stamp says a loop went by, its `files` line says what the store holds.
//!
//! # Why this is `#[ignore]`d
//!
//! `init` and the daemon each derive the keystore key, 64 MiB of Argon2id
//! that is slow on purpose and far slower in a debug build. The release run
//! of the ignored tests ("Expensive tests" in CI) pays seconds for it.
//!
//! Three ways a paused node could still move files, one test each: a daemon
//! started paused, a pause landing while a round is under way, and the
//! account's own device pushing into a paused node. The third is the one the
//! first version got wrong -- a `match` arm that ignored the vault drain's
//! result while the drain ran anyway (#243's review).
//!
//! # What it does not cover
//!
//! Windows and macOS in CI: the job that runs ignored tests is Linux only.
//! Run on Windows by hand on 2026-10-06.

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const PASSPHRASE: &str = "steering-test-passphrase";

/// Long enough for a slow CI machine to start a daemon in release, short
/// enough that a daemon which never writes a snapshot fails the test rather
/// than the runner's budget.
const PATIENCE: Duration = Duration::from_secs(40);

fn itsanas(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_itsanas"));
    command
        .arg("--home")
        .arg(home)
        .env("ITSANAS_PASSPHRASE", PASSPHRASE)
        .env_remove("ITSANAS_INSTANCE")
        .stdin(Stdio::null());
    command
}

/// Run a command that must succeed, and return what it printed.
fn run(home: &Path, args: &[&str]) -> String {
    let output = itsanas(home).args(args).output().expect("run itsanas");
    assert!(
        output.status.success(),
        "itsanas {args:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A node with a synced folder, made with `init`; returns what `init` printed,
/// which holds the 24 words.
fn node_with_folder(base: &Path, name: &str) -> (PathBuf, PathBuf, String) {
    let home = base.join(format!("{name}-home"));
    let folder = base.join(format!("{name}-folder"));
    std::fs::create_dir(&folder).expect("folder");
    let printed = run(&home, &["init", "--username", "steering"]);
    // `folder` reconciles once as it is set, so files written after it can
    // only be taken in by the daemon.
    run(
        &home,
        &["folder", folder.to_str().expect("utf-8 path"), "--confirm"],
    );
    (home, folder, printed)
}

/// Killed and reaped however the test ends, so a failing assertion never
/// leaves a daemon running on the CI machine.
struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    fn start(home: &Path, log: PathBuf) -> Self {
        let out = std::fs::File::create(&log).expect("daemon log");
        let err = out.try_clone().expect("daemon log");
        let child = itsanas(home)
            .args([
                "daemon",
                "--listen",
                "127.0.0.1:0",
                "--no-discovery",
                "--interval",
                "3600",
            ])
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("start the daemon");
        Self { child, log }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The address it listens on, from its log's `serving` line.
    fn address(&self) -> String {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            let found = self.log().lines().find_map(|line| {
                line.trim_start()
                    .strip_prefix("serving")
                    .map(|rest| rest.trim().to_owned())
            });
            if let Some(address) = found {
                return address;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "the daemon never said where it serves\n--- daemon log\n{}",
            self.log()
        );
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The snapshot's stamp and its `files` count, if there is a whole one.
fn snapshot(home: &Path) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(home.join("status.snapshot")).ok()?;
    let stamp = text
        .lines()
        .next()?
        .strip_prefix("snapshot ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let files = text
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("files"))?
        .trim()
        .parse()
        .ok()?;
    Some((stamp, files))
}

/// Wait until the snapshot satisfies `wanted`, or say what it last was.
fn wait_for(
    home: &Path,
    daemon: &Daemon,
    what: &str,
    wanted: impl Fn((u64, u64)) -> bool,
) -> (u64, u64) {
    let deadline = Instant::now() + PATIENCE;
    let mut last = None;
    while Instant::now() < deadline {
        last = snapshot(home);
        if let Some(seen) = last.filter(|seen| wanted(*seen)) {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!(
        "{what}: after {PATIENCE:?} the snapshot said {last:?} (stamp, files)\n--- daemon log\n{}",
        daemon.log()
    );
}

/// After two snapshots three seconds apart -- at least one whole loop that
/// would have taken a file in -- the store must still hold `files`.
fn still_holds(home: &Path, daemon: &Daemon, files: u64, why: &str) {
    let (first, _) = wait_for(home, daemon, "the daemon never reported", |_| true);
    let (_, held) = wait_for(home, daemon, "the daemon stopped looping", |(stamp, _)| {
        stamp >= first + 3
    });
    assert_eq!(held, files, "{why}\n--- daemon log\n{}", daemon.log());
}

#[test]
#[ignore = "two Argon2id derivations and a running daemon; the release job runs it"]
fn red_team_a_paused_daemon_takes_in_no_file_until_resumed() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "a");
    run(&home, &["pause"]);
    std::fs::write(folder.join("written-while-paused.txt"), b"wait for me")
        .expect("write into the folder");

    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    still_holds(
        &home,
        &daemon,
        0,
        "a paused daemon took a file into the store: pausing did not stop syncing",
    );

    run(&home, &["resume"]);
    wait_for(
        &home,
        &daemon,
        "resuming did not take in the file written while paused",
        |(_, files)| files == 1,
    );
}

/// A pause asked for while a round is under way. The round is held open by
/// a "peer" this test owns, which accepts the daemon's connection and says
/// nothing; the pause and a new file arrive meanwhile. When the peer lets go,
/// the round ends -- and the folder scans that close every round must not run.
#[test]
#[ignore = "two Argon2id derivations and a running daemon; the release job runs it"]
fn red_team_a_pause_landing_mid_round_takes_in_no_file() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "a");

    let peer = TcpListener::bind("127.0.0.1:0").expect("a port for the silent peer");
    let address = peer.local_addr().expect("its address").to_string();
    run(&home, &["peer", "add", &address]);
    let (accepted, round_open) = mpsc::channel();
    let (release, let_go) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let Ok((connection, _)) = peer.accept() else {
            return;
        };
        let _ = accepted.send(());
        let _ = let_go.recv_timeout(PATIENCE);
        drop(connection);
    });

    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    assert!(
        round_open.recv_timeout(PATIENCE).is_ok(),
        "the daemon never dialled its configured peer\n--- daemon log\n{}",
        daemon.log()
    );
    run(&home, &["pause"]);
    std::fs::write(folder.join("written-mid-round.txt"), b"wait for me")
        .expect("write into the folder");
    let _ = release.send(());
    holder.join().expect("the silent peer");

    still_holds(
        &home,
        &daemon,
        0,
        "the round under way when the pause came took a file in as it ended",
    );
    run(&home, &["resume"]);
    wait_for(
        &home,
        &daemon,
        "resuming did not take in the file written mid-round",
        |(_, files)| files == 1,
    );
}

/// The account's own device pushes into a paused node: the listener keeps
/// serving, by design, so what arrives lands in the vault -- and must wait
/// there. Adopting it into the store is syncing.
#[test]
#[ignore = "three Argon2id derivations and a running daemon; the release job runs it"]
fn red_team_a_paused_daemon_adopts_nothing_its_own_devices_push() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, _folder, printed) = node_with_folder(base.path(), "a");
    // Room for what its own device sends: a node that pledged nothing
    // refuses even its own account's push, and the test would pass on
    // nothing having arrived.
    run(&home, &["pledge", "100M"]);
    run(&home, &["pause"]);

    // The second device: the same account, restored from the 24 words `init`
    // printed as a numbered grid, which `login` reads as it is.
    let grid: String = printed
        .lines()
        .filter(|line| {
            line.split_whitespace().next().is_some_and(|first| {
                first
                    .strip_suffix('.')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    let phrase = base.path().join("phrase.txt");
    std::fs::write(&phrase, &grid).expect("phrase file");
    let other = base.path().join("b-home");
    run(
        &other,
        &[
            "login",
            "--username",
            "steering",
            "--phrase-file",
            phrase.to_str().expect("utf-8 path"),
        ],
    );
    let source = base.path().join("from-the-other-device.txt");
    std::fs::write(&source, b"pushed while paused").expect("source file");
    run(
        &other,
        &[
            "put",
            "from-the-other-device.txt",
            source.to_str().expect("utf-8 path"),
        ],
    );

    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    let pushed = run(&other, &["sync", &daemon.address()]);
    assert!(
        !pushed.contains("sent 0 B") && !pushed.contains("refused"),
        "nothing reached the paused node, so this test would prove nothing: {pushed}"
    );

    still_holds(
        &home,
        &daemon,
        0,
        "a paused daemon adopted what its own device pushed: pausing did not stop syncing",
    );
    run(&home, &["resume"]);
    wait_for(
        &home,
        &daemon,
        "resuming did not adopt what was pushed while paused",
        |(_, files)| files == 1,
    );
}
