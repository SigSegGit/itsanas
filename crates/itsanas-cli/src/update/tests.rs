//! The self-update against a fake release served from a temporary directory:
//! no network, no real program replaced. Each red-team test checks that the
//! "installed" program is byte for byte what it was.

use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
};

use itsanas_release::{FileEntry, Manifest, ReleaseKey, Trust, Version};

use super::{Daily, Fetch, Found, NOTICE_FILE, Updater, aside, daily, replace, this_target};
use crate::config::Updates;

const TARGET: &str = "x86_64-unknown-linux-gnu";
const OLD_PROGRAM: &[u8] = b"the program that runs today, release 0.1.0";
const NEW_PROGRAM: &[u8] = b"the program of release 0.2.0, a little longer than the old";

/// Serves `root/<path>` as GitHub would serve `releases/<path>`, and records
/// what was asked.
struct Served {
    root: PathBuf,
    asked: RefCell<Vec<String>>,
}

impl Fetch for Served {
    fn get(&self, path: &str, to: &Path, max_bytes: u64) -> Result<(), String> {
        self.asked.borrow_mut().push(path.to_owned());
        let from = self.root.join(path);
        let bytes = std::fs::read(&from).map_err(|e| format!("404 {path}: {e}"))?;
        if bytes.len() as u64 > max_bytes {
            return Err(format!("{path} is larger than {max_bytes} bytes"));
        }
        std::fs::write(to, bytes).map_err(|e| e.to_string())
    }
}

/// A machine with release 0.1.0 installed, and a server.
struct World {
    _dir: tempfile::TempDir,
    exe: PathBuf,
    home: PathBuf,
    served: Served,
    key: ReleaseKey,
}

fn version(text: &str) -> Version {
    Version::parse(text).expect("a version")
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let bin = dir.path().join("bin");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&bin).expect("bin");
        std::fs::create_dir_all(&home).expect("home");
        let exe = bin.join("itsanas");
        std::fs::write(&exe, OLD_PROGRAM).expect("old program");
        let world = Self {
            exe,
            home,
            served: Served {
                root: dir.path().join("served"),
                asked: RefCell::new(Vec::new()),
            },
            key: ReleaseKey::generate().expect("key"),
            _dir: dir,
        };
        // The running program's own release, which proves it came from one.
        world.publish_as("0.1.0", OLD_PROGRAM, &world.key, false);
        world
    }

    /// Publish `program` as release `v`, signed by `key`; `latest` makes it
    /// the release GitHub calls latest.
    fn publish_as(&self, v: &str, program: &[u8], key: &ReleaseKey, latest: bool) -> Vec<u8> {
        let release = self.served.root.join(format!("download/v{v}"));
        std::fs::create_dir_all(&release).expect("release dir");
        let binary = release.join(format!("itsanas-{TARGET}"));
        std::fs::write(&binary, program).expect("binary");
        let manifest = Manifest {
            version: version(v),
            files: vec![FileEntry::measure(TARGET, &binary).expect("measure")],
            next_key: None,
        };
        let text = manifest.to_text().into_bytes();
        let (_, signature) = key.sign_manifest(&text).expect("sign");
        let mut dirs = vec![release];
        if latest {
            dirs.push(self.served.root.join("latest/download"));
        }
        for dir in dirs {
            std::fs::create_dir_all(&dir).expect("dir");
            std::fs::write(dir.join("manifest.txt"), &text).expect("manifest");
            std::fs::write(dir.join("manifest.txt.sig"), &signature).expect("signature");
        }
        text
    }

    fn publish(&self, v: &str, program: &[u8]) -> Vec<u8> {
        self.publish_as(v, program, &self.key, true)
    }

    fn updater(&self) -> Updater<'_> {
        Updater {
            fetch: &self.served,
            trust: Some(Trust::from_pinned(Some(self.key.public_bytes())).expect("trust")),
            running: version("0.1.0"),
            target: Some(TARGET),
            exe: self.exe.clone(),
        }
    }

    /// The installed program is exactly what it was, nothing set aside, no
    /// download left beside it.
    fn assert_untouched(&self, why: &str) {
        assert_eq!(
            std::fs::read(&self.exe).expect("the program is still there"),
            OLD_PROGRAM,
            "{why}: the running program was replaced by a release that should have been refused"
        );
        assert!(
            !aside(&self.exe).exists(),
            "{why}: the program was moved aside for a refused release"
        );
        let leftovers: Vec<_> = std::fs::read_dir(self.exe.parent().expect("bin"))
            .expect("bin")
            .map(|e| e.expect("entry").file_name())
            .filter(|name| name != "itsanas")
            .collect();
        assert!(
            leftovers.is_empty(),
            "{why}: a refused download was left beside the program: {leftovers:?}"
        );
    }

    /// Check, then install what was found: what `itsanas update` does.
    fn update(&self) -> crate::error::Result<Version> {
        let updater = self.updater();
        match updater.check()? {
            Found::Available(manifest) => updater.install(&manifest),
            other => Err(crate::error::CliError::Usage(format!("found {other:?}"))),
        }
    }
}

fn refusal(result: crate::error::Result<impl std::fmt::Debug>) -> String {
    match result {
        Ok(found) => panic!("accepted ({found:?}) a release that must be refused"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_signed_newer_release_replaces_the_program() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    let installed = world
        .update()
        .expect("a newer release signed by the key installs");
    assert_eq!(installed, version("0.2.0"));
    assert_eq!(
        std::fs::read(&world.exe).expect("program"),
        NEW_PROGRAM,
        "the update said it installed and the old program still runs: members never get fixes"
    );
}

#[test]
fn red_team_an_update_signed_by_another_key_is_refused() {
    let world = World::new();
    let stranger = ReleaseKey::generate().expect("key");
    world.publish_as("0.2.0", NEW_PROGRAM, &stranger, true);
    let said = refusal(world.update());
    assert!(
        said.contains("not signed"),
        "whoever can upload to the release page could push a program onto every machine: {said}"
    );
    world.assert_untouched("another key");
}

#[test]
fn red_team_a_modified_manifest_with_a_valid_signature_is_refused() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    // Point the signed manifest at a different binary: same signature file,
    // one line changed.
    let latest = world.served.root.join("latest/download/manifest.txt");
    let text = std::fs::read_to_string(&latest).expect("manifest");
    let evil = b"a different program of exactly the same length as release 0.2.0's";
    let forged = FileEntry {
        target: TARGET.to_owned(),
        name: format!("itsanas-{TARGET}"),
        size: evil.len() as u64,
        blake3: *blake3::hash(evil).as_bytes(),
        sha256: [7; 32],
    };
    let changed = Manifest {
        version: version("0.2.0"),
        files: vec![forged],
        next_key: None,
    }
    .to_text();
    assert_ne!(text, changed);
    std::fs::write(&latest, changed).expect("tamper");
    let said = refusal(world.update());
    assert!(
        said.contains("not signed"),
        "a signature that does not cover the bytes read lets anyone rewrite the hashes: {said}"
    );
    world.assert_untouched("modified manifest");
}

#[test]
fn red_team_an_older_release_is_never_installed() {
    let world = World::new();
    world.publish("0.0.9", NEW_PROGRAM);
    assert_eq!(
        world.updater().check().expect("check"),
        Found::UpToDate(version("0.1.0")),
        "an old signed release with a known bug offered as an update could be replayed"
    );
    // And install refuses it on its own, whoever calls it.
    let old =
        Manifest::parse(&String::from_utf8(world.publish("0.0.9", NEW_PROGRAM)).expect("text"))
            .expect("manifest");
    let said = refusal(world.updater().install(&old));
    assert!(
        said.contains("not newer"),
        "a downgrade was installed: {said}"
    );
    world.assert_untouched("downgrade");
}

#[test]
fn red_team_a_download_whose_hash_differs_is_refused() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    let binary = world
        .served
        .root
        .join(format!("download/v0.2.0/itsanas-{TARGET}"));
    let mut swapped = NEW_PROGRAM.to_vec();
    swapped[0] ^= 1;
    std::fs::write(&binary, swapped).expect("swap");
    let said = refusal(world.update());
    assert!(
        said.contains("not the file that was signed"),
        "a binary swapped on the server after signing would run on every machine: {said}"
    );
    world.assert_untouched("hash mismatch");
}

#[test]
fn red_team_a_truncated_update_is_never_put_in_place() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    let binary = world
        .served
        .root
        .join(format!("download/v0.2.0/itsanas-{TARGET}"));
    std::fs::write(&binary, &NEW_PROGRAM[..NEW_PROGRAM.len() / 2]).expect("truncate");
    let said = refusal(world.update());
    assert!(
        said.contains("incomplete"),
        "half a program put in place is a machine that no longer starts: {said}"
    );
    world.assert_untouched("truncated");
}

#[test]
fn red_team_a_build_not_installed_from_a_release_never_updates_itself() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    // Built from source: its bytes are not release 0.1.0's.
    std::fs::write(&world.exe, b"built from source by a developer, not 0.1.0").expect("own");
    let said = refusal(world.update());
    assert!(
        said.contains("not installed from a release"),
        "a source build was replaced behind its developer's back: {said}"
    );
    assert_eq!(
        std::fs::read(&world.exe).expect("program"),
        b"built from source by a developer, not 0.1.0"
    );

    // And one running from a cargo target/ directory does not even look.
    let in_target = world
        .exe
        .parent()
        .expect("bin")
        .join("target")
        .join("itsanas");
    std::fs::create_dir_all(in_target.parent().expect("dir")).expect("target dir");
    let updater = Updater {
        exe: in_target,
        ..world.updater()
    };
    assert!(
        matches!(updater.check(), Ok(Found::NotFromRelease(_))),
        "a binary in target/ would be replaced by a download while being developed"
    );
}

#[test]
fn without_a_pinned_key_nothing_is_fetched() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    let updater = Updater {
        trust: None,
        ..world.updater()
    };
    assert_eq!(updater.check().expect("check"), Found::NoKey);
    assert!(
        world.served.asked.borrow().is_empty(),
        "a build with no key asked the network for releases it cannot trust"
    );
    world.assert_untouched("no key");
}

#[test]
fn red_team_a_failed_swap_puts_the_old_program_back() {
    let world = World::new();
    let new = world.exe.parent().expect("bin").join("itsanas.new");
    std::fs::write(&new, NEW_PROGRAM).expect("new");

    // The second rename (new into place) fails.
    let mut calls = 0;
    let result = replace(&world.exe, &new, &mut |from, to| {
        calls += 1;
        if calls == 2 {
            Err(io::Error::other("disk full"))
        } else {
            std::fs::rename(from, to)
        }
    });
    assert!(result.is_err(), "a failed swap was reported as installed");
    assert_eq!(
        std::fs::read(&world.exe).expect("the program is back"),
        OLD_PROGRAM,
        "a failed swap left no program where the service starts one: the machine is down"
    );
    assert!(!aside(&world.exe).exists());

    // The first rename fails: nothing moves.
    let result = replace(&world.exe, &new, &mut |_, _| {
        Err(io::Error::other("in use"))
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&world.exe).expect("program"), OLD_PROGRAM);
}

#[test]
fn the_daily_look_follows_the_setting() {
    let world = World::new();
    world.publish("0.2.0", NEW_PROGRAM);
    let updater = world.updater();

    assert_eq!(
        daily(&world.home, &updater, Updates::Off).expect("off"),
        Daily::Off
    );
    assert!(
        world.served.asked.borrow().is_empty(),
        "updates = off still asked GitHub: the member said never look"
    );

    assert_eq!(
        daily(&world.home, &updater, Updates::Notify).expect("notify"),
        Daily::Notified(version("0.2.0"))
    );
    assert_eq!(
        std::fs::read_to_string(world.home.join(NOTICE_FILE)).expect("notice"),
        "0.2.0\n",
        "notify found a release and status would not say so"
    );
    world.assert_untouched("notify must not install");

    assert_eq!(
        daily(&world.home, &updater, Updates::Auto).expect("auto"),
        Daily::Installed(version("0.2.0"))
    );
    assert_eq!(std::fs::read(&world.exe).expect("program"), NEW_PROGRAM);
    assert!(!world.home.join(NOTICE_FILE).exists());
}

/// The target an update downloads is the machine's own: a wrong answer
/// installs a binary that cannot run, or never updates a covered machine.
/// Checked against the standard library's own reading of the platform, on
/// each system CI runs (Linux, Windows and macOS).
#[test]
fn this_target_names_the_machine_it_runs_on() {
    let expected = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") if cfg!(target_env = "gnu") => Some("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") if cfg!(target_env = "gnu") => Some("aarch64-unknown-linux-gnu"),
        ("x86_64", "windows") if cfg!(target_env = "msvc") => Some("x86_64-pc-windows-msvc"),
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        _ => None,
    };
    assert_eq!(
        this_target(),
        expected,
        "this machine would download another platform's binary, or none"
    );
}
