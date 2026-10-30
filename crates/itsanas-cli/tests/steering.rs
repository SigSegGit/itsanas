//! The daemon loop, driven for real: a paused node takes in no file.
//!
//! `control.rs` tests the decisions -- what is due, what a pause allows -- as
//! pure functions. What acts on them is `sync_loop`, and no other test runs
//! it: the guard that keeps a paused daemon away from the folder could be
//! deleted and every unit test would still pass (the CI reviewer's finding on
//! #243, and the gap HANDOVER §8 0w (1) named). This test starts the real
//! binary on a throwaway node and watches what it reports.
//!
//! What it watches is the snapshot the daemon writes after a round, when the
//! folder changes, and at most every half minute otherwise (`snapshot_due`;
//! `status.snapshot`, the file `itsanas status` and the tray read): its
//! stamp says loops went by, its `files` line says what the store holds.
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
//! result while the drain ran anyway (#243's review). And the other way round:
//! a pause with an end must end by itself, with nobody running `resume`.
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

/// A second device of the account `init` printed the 24 words of, restored
/// from them as a numbered grid, which `login` reads as it is.
fn second_device(base: &Path, printed: &str) -> PathBuf {
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
    let phrase = base.join("phrase.txt");
    std::fs::write(&phrase, &grid).expect("phrase file");
    let other = base.join("b-home");
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
    other
}

/// Wait until the snapshot's first line satisfies `wanted`, and return it.
fn wait_for_header(
    home: &Path,
    daemon: &Daemon,
    what: &str,
    wanted: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + PATIENCE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let text = std::fs::read_to_string(home.join("status.snapshot")).unwrap_or_default();
        text.lines().next().unwrap_or("").clone_into(&mut last);
        if wanted(&last) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!(
        "{what}: after {PATIENCE:?} the snapshot's header said {last:?}\n--- daemon log\n{}",
        daemon.log()
    );
}

/// The words after `reached` in a header, if it carries a contact time.
fn contact_time(header: &str) -> Option<u64> {
    let words: Vec<&str> = header.split_whitespace().collect();
    let at = words.iter().position(|word| *word == "reached")?;
    words.get(at + 1)?.parse().ok()
}

/// `isolated` ages what the header's `reached` says, and only `sync_loop`
/// stamps it, from what the round reached: every unit test builds that by
/// hand, so the stamp could be deleted with all of them green and the alert
/// dead (the review of #345). A daemon whose configured peer, a machine of
/// its account, answers its round must write `reached T`; the peer, which
/// was only dialled into and dialled nobody, must write `reached never
/// since T` -- an inbound connection is not a contact. Sabotage: drop the
/// stamp in `sync_loop`, or stamp every round.
#[test]
#[ignore = "three Argon2id derivations and two running daemons; the release job runs it"]
fn red_team_a_round_that_reaches_the_account_stamps_the_contact() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, _folder, printed) = node_with_folder(base.path(), "a");
    let other = second_device(base.path(), &printed);
    let peer = Daemon::start(&other, base.path().join("b.log"));
    run(&home, &["peer", "add", &peer.address()]);
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let daemon = Daemon::start(&home, base.path().join("a.log"));

    let text = wait_for_header(
        &home,
        &daemon,
        "a round that reached the account's other machine left no contact on the \
         header: `isolated` turns that laptop orange six hours later, synced all along",
        |header| contact_time(header).is_some(),
    );
    let header = text.lines().next().unwrap_or("");
    assert!(
        contact_time(header).is_some_and(|at| at >= started),
        "the contact on the header predates the daemon: {header}"
    );
    let theirs = wait_for_header(&other, &peer, "the peer never wrote a snapshot", |header| {
        header.contains(" reached ")
    });
    let theirs = theirs.lines().next().unwrap_or("");
    assert!(
        theirs.contains(" reached never since "),
        "a machine only dialled into, which reached nobody itself, wrote a contact: \
         {theirs}\n--- its log\n{}",
        peer.log()
    );
}

/// A vault the daemon's probe cannot write, through the running daemon: the
/// probe is called from `sync_loop` once a round, and a unit test that
/// hands the word to the snapshot writer passes with that call deleted (the
/// review of #345). The vault's root made read-only after `init` -- its
/// database file stays writable, so the daemon starts -- must reach the
/// header as ` unwritable` and the snapshot `status` prints as CANNOT KEEP.
/// Sabotage: drop the `probe_vault` call.
#[cfg(unix)]
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn red_team_a_running_daemon_says_when_its_vault_takes_no_write() {
    use std::os::unix::fs::PermissionsExt as _;
    let base = tempfile::tempdir().expect("temp dir");
    let (home, _folder, _) = node_with_folder(base.path(), "a");
    run(&home, &["pledge", "100M"]);
    // A vault in use already holds this account's directories: `status`
    // makes them, and without them the daemon's own `status` -- the
    // snapshot -- fails at the read-only root before any word is written.
    run(&home, &["status"]);
    let vault = home.join("vault");
    let set = |mode| {
        std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(mode)).expect("chmod");
    };
    set(0o555);
    // Root writes through 0o555: nothing to refuse it (CI is not root).
    if std::fs::write(vault.join("as-root"), b"x").is_ok() {
        set(0o755);
        return;
    }
    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for_header(
            &home,
            &daemon,
            "a vault that takes no write never reached the header: the icon stays green \
             while every chunk sent to it fails",
            |header| header.split_whitespace().any(|word| word == "unwritable"),
        )
    }));
    // Writable again before anything fails, or the temp dir is left behind.
    set(0o755);
    let text = found.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    assert!(
        text.contains("CANNOT KEEP    the vault at"),
        "the snapshot `status` prints does not say the vault takes nothing:\n{text}"
    );
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

/// The idle bench of 2026-10-27: an idle daemon rewrote its snapshot at every
/// two-second pass of its loop, most of an idle node's disk writes. Through
/// the real loop: once settled, a daemon with nothing to do leaves the stamp
/// alone for ten seconds -- five passes -- and a file dropped in the folder is
/// in the snapshot within seconds, not at the next half-minute heartbeat.
/// Sabotage: the snapshot written at every pass; a folder change not passed
/// on as news.
#[test]
#[ignore = "two Argon2id derivations and a running daemon; the release job runs it"]
fn red_team_an_idle_daemon_leaves_its_snapshot_alone_between_rounds() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "idle");
    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    wait_for(&home, &daemon, "the daemon never reported", |_| true);

    std::thread::sleep(Duration::from_secs(3));
    let (settled, _) = snapshot(&home).expect("a snapshot");
    std::thread::sleep(Duration::from_secs(10));
    let (later, _) = snapshot(&home).expect("a snapshot");
    assert_eq!(
        later,
        settled,
        "an idle daemon rewrote its snapshot within ten seconds: every pass of the \
         loop writes it, most of an idle node's disk writes\n--- daemon log\n{}",
        daemon.log()
    );

    let dropped = Instant::now();
    std::fs::write(folder.join("news.txt"), b"something to say").expect("write");
    wait_for(
        &home,
        &daemon,
        "a file dropped in was never reported",
        |(_, files)| files == 1,
    );
    assert!(
        dropped.elapsed() < Duration::from_secs(12),
        "a file dropped in took {:?} to reach the snapshot: a change waited for the \
         heartbeat instead of being written at once\n--- daemon log\n{}",
        dropped.elapsed(),
        daemon.log()
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

    let other = second_device(base.path(), &printed);
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

/// The tray's "pause for 1 hour", shortened to four seconds: the control file
/// is written as `itsanas pause --for` writes it, and nobody runs `resume`.
/// When the end passes, the running daemon must notice by the clock alone and
/// take in the file written meanwhile -- a pause that never ends by itself is
/// somebody who believes their files are safe.
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn red_team_a_timed_pause_ends_by_itself_without_resume() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "a");
    run(&home, &["pause"]);
    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    wait_for(&home, &daemon, "the paused daemon never reported", |_| true);

    std::fs::write(folder.join("written-while-paused.txt"), b"wait for me")
        .expect("write into the folder");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let until = now + 4;
    std::fs::write(
        home.join("control"),
        format!("paused {now}\nuntil {until}\n"),
    )
    .expect("write the control file");

    let (stamp, _) = wait_for(
        &home,
        &daemon,
        "a pause whose end had passed never let the daemon take the file in",
        |(_, files)| files == 1,
    );
    assert!(
        stamp >= until,
        "the file was taken in {}s before the pause's end: the end was not honoured\n\
         --- daemon log\n{}",
        until - stamp,
        daemon.log()
    );
    assert!(
        daemon.log().contains("pause over, syncing resumed"),
        "the log never said the pause ended, so a person reading it cannot tell why \
         syncing started again\n--- daemon log\n{}",
        daemon.log()
    );
}

/// A vault on a USB disk lent to the network: unplugged under a running
/// daemon, the daemon must not write onto the bare mount point and must not
/// exit (a laptop has no service manager to restart it). It keeps syncing
/// this account's own files meanwhile, and hosts again by itself, within
/// seconds, when the disk comes back.
#[cfg(unix)]
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn red_team_an_unplugged_vault_disk_pauses_the_daemon_and_its_return_resumes_it() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "usb");
    let disk = base.path().join("usb-disk");
    std::fs::create_dir(&disk).expect("the disk");
    let vault = disk.join("vault");
    run(
        &home,
        &["vault", vault.to_str().expect("utf-8"), "--same-disk"],
    );

    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    daemon.address();
    let (_, before) = wait_for(&home, &daemon, "the daemon never reported", |_| true);

    let away = base.path().join("usb-disk-unplugged");
    std::fs::rename(&disk, &away).expect("unplug");
    let said = |what: &str| {
        let deadline = Instant::now() + PATIENCE;
        while !daemon.log().contains(what) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(
            daemon.log().contains(what),
            "the daemon never said {what:?}\n--- daemon log\n{}",
            daemon.log()
        );
    };
    said("syncs its own files and hosts nobody");
    assert!(
        !vault.exists(),
        "a vault was recreated on the bare mount point: {:?}\n--- daemon log\n{}",
        walkdir_list(&disk),
        daemon.log()
    );

    // Away, its own files still go in.
    std::fs::write(folder.join("written-while-away.txt"), b"still syncing").expect("file");
    wait_for(
        &home,
        &daemon,
        "a file written while the disk was away was not taken in",
        |(_, files)| files > before,
    );

    std::fs::rename(&away, &disk).expect("plug back");
    said("hosting again");
    let deadline = Instant::now() + PATIENCE;
    while daemon.log().matches("serving").count() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        daemon.log().matches("serving").count() >= 3,
        "the daemon did not serve again on its real vault\n--- daemon log\n{}",
        daemon.log()
    );
}

#[cfg(unix)]
fn walkdir_list(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
    }
    out
}

/// The same unplug, on every platform: Windows will not rename a directory
/// holding an open database, so the disk's absence is made by taking away
/// the one file the daemon decides on, the vault's marker. This proves the
/// decision and the switch both ways; a real disk pulled mid-write is the
/// manual test's (HANDOVER §8 8.0c).
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn red_team_a_vault_whose_marker_goes_away_is_left_alone_until_it_returns() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "marker");
    let vault = base.path().join("disk").join("vault");
    run(
        &home,
        &["vault", vault.to_str().expect("utf-8"), "--same-disk"],
    );

    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    daemon.address();
    let (_, before) = wait_for(&home, &daemon, "the daemon never reported", |_| true);
    let said = |what: &str, times: usize| {
        let deadline = Instant::now() + PATIENCE;
        while daemon.log().matches(what).count() < times && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(
            daemon.log().matches(what).count() >= times,
            "the daemon never said {what:?} {times} time(s)\n--- daemon log\n{}",
            daemon.log()
        );
    };

    let marker = vault.join(".itsanas-vault");
    let aside = base.path().join("marker-aside");
    std::fs::rename(&marker, &aside).expect("take the marker away");
    said("syncs its own files and hosts nobody", 1);

    std::fs::write(folder.join("written-while-away.txt"), b"still syncing").expect("file");
    wait_for(
        &home,
        &daemon,
        "a file written while the vault was away was not taken in",
        |(_, files)| files > before,
    );
    assert!(
        !marker.exists(),
        "the away daemon wrote a marker into the vault it had lost\n--- daemon log\n{}",
        daemon.log()
    );

    std::fs::rename(&aside, &marker).expect("put the marker back");
    said("hosting again", 1);
    said("serving", 3);
}

/// Disk work while the daemon runs: it holds the store, so `backup export`
/// and `vault --move` hand the job to it rather than failing on the lock or
/// asking the passphrase again, and the daemon carries on afterwards, on the
/// new vault for a move.
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn disk_jobs_go_through_the_running_daemon_which_carries_on_after_them() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, folder, _) = node_with_folder(base.path(), "jobs");
    std::fs::write(folder.join("kept.txt"), b"in the backup").expect("file");
    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    daemon.address();
    wait_for(&home, &daemon, "the file was not taken in", |(_, files)| {
        files >= 1
    });

    let backup = base.path().join("backup");
    let said = run(
        &home,
        &["backup", "export", backup.to_str().expect("utf-8")],
    );
    assert!(said.contains("backed up"), "{said}");
    assert!(
        backup.join("itsanas-backup").is_file(),
        "no complete backup"
    );

    let moved = base.path().join("other-disk").join("vault");
    let said = run(
        &home,
        &[
            "vault",
            moved.to_str().expect("utf-8"),
            "--move",
            "--same-disk",
        ],
    );
    assert!(said.contains("vault moved to"), "{said}");
    assert!(
        moved.join(".itsanas-vault").is_file(),
        "no vault at the new place"
    );
    let config = std::fs::read_to_string(home.join("config")).unwrap_or_default();
    assert!(
        config.contains("other-disk"),
        "the config does not name the new vault: {config}"
    );
    let said = run(
        &home,
        &["backup", "restore", backup.to_str().expect("utf-8")],
    );
    assert!(said.contains("read "), "{said}");
    let deadline = Instant::now() + PATIENCE;
    while daemon.log().matches("serving").count() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        daemon.log().matches("serving").count() >= 2,
        "the daemon did not carry on after the move\n--- daemon log\n{}",
        daemon.log()
    );
}

/// A portable copy on a drive: its launcher keeps the home on the drive, and
/// its daemon syncs this account's files and hosts nobody, because a drive
/// that travels is no place for other people's data.
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn red_team_a_portable_copy_keeps_its_home_on_the_drive_and_hosts_nobody() {
    let base = tempfile::tempdir().expect("temp dir");
    let drive = base.path().join("drive");
    let said = run(
        &base.path().join("unused-home"),
        &["portable", drive.to_str().expect("utf-8")],
    );
    assert!(said.contains("hosting anybody"), "{said}");
    let home = drive.join("ITSaNAS").join("home");
    run(&home, &["init", "--username", "portable"]);

    #[cfg(unix)]
    {
        let output = std::process::Command::new("sh")
            .arg(drive.join("ITSaNAS").join("itsanas.sh"))
            .arg("status")
            .env("ITSANAS_PASSPHRASE", PASSPHRASE)
            .env_remove("ITSANAS_HOME")
            .output()
            .expect("the launcher");
        assert!(
            output.status.success(),
            "the launcher did not find its home on the drive: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let daemon = Daemon::start(&home, base.path().join("daemon.log"));
    daemon.address();
    let deadline = Instant::now() + PATIENCE;
    while !daemon.log().contains("hosts nobody") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        daemon.log().contains("  portable  yes"),
        "the portable daemon did not say it hosts nobody\n--- daemon log\n{}",
        daemon.log()
    );
}

/// `itsanas update` and `signout` ask the daemon itself to restart or stop
/// (hand test, 2026-10-22: the service manager left three laptop daemons on
/// the old version while `update` said it had restarted them). A restart
/// exits with the code every supervisor here starts again, a stop with 0,
/// which none of them does.
#[test]
#[ignore = "one Argon2id derivation and a running daemon; the release job runs it"]
fn red_team_a_daemon_asked_to_restart_or_stop_exits_with_the_code_that_says_so() {
    let base = tempfile::tempdir().expect("temp dir");
    let (home, _, _) = node_with_folder(base.path(), "life");
    for (word, code) in [("restart", 75), ("stop", 0)] {
        let mut daemon = Daemon::start(&home, base.path().join(format!("{word}.log")));
        daemon.address();
        std::fs::write(home.join("lifecycle"), word).expect("ask");
        let deadline = Instant::now() + PATIENCE;
        let status = loop {
            if let Some(status) = daemon.child.try_wait().expect("wait") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon never acted on `{word}`\n--- daemon log\n{}",
                daemon.log()
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        assert_eq!(
            status.code(),
            Some(code),
            "`{word}` exited with {status}\n--- daemon log\n{}",
            daemon.log()
        );
        assert!(
            !home.join("lifecycle").exists(),
            "the request was left to act again"
        );
    }
}
