//! `copy-to-key` against a real account, a temporary "key" and stand-ins
//! for the release and the coordinator: no network, no window.

use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::{
    FOLDER, Passphrases, STATUS, check_target, content_in, copy, enrol_at_start, read_passphrases,
    room, weak_passphrase,
};
use crate::{config::Config, error::CliError, node::Node, portable};

const MACHINE: &str = "the passphrase of this laptop";
const KEY: &str = "a long passphrase for the key alone";
const PROGRAM: &str = "itsanas-x86_64-unknown-linux-gnu";

/// A machine with an account, its daemon running (the store held), and a
/// drive to copy it to.
struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    drive: PathBuf,
    phrase: Zeroizing<String>,
    node: Node,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("laptop");
        let (mut node, phrase) = Node::create(&home, MACHINE, "camille").expect("account");
        node.config.coordinator = Some("127.0.0.1:9".to_owned());
        node.config.coordinator_device = Some("ab".repeat(32));
        node.config.folder = Some(dir.path().join("Documents"));
        node.save_config().expect("config");
        let drive = dir.path().join("KEY");
        std::fs::create_dir_all(&drive).expect("drive");
        Self {
            home,
            drive,
            phrase: Zeroizing::new(phrase.as_str().to_owned()),
            node,
            _dir: dir,
        }
    }

    fn root(&self) -> PathBuf {
        self.drive.join(portable::DIR)
    }

    fn key_home(&self) -> PathBuf {
        self.root().join("home")
    }

    fn copy_with(
        &self,
        dir: &Path,
        machine: &str,
        key: &str,
        register: impl FnOnce(&Node) -> crate::error::Result<()>,
    ) -> crate::error::Result<String> {
        let passes = Passphrases {
            machine: Zeroizing::new(machine.to_owned()),
            key: Zeroizing::new(key.to_owned()),
        };
        copy(&self.home, dir, &passes, Some(1024), served, register)
    }

    fn copy(&self, machine: &str, key: &str) -> crate::error::Result<String> {
        self.copy_with(&self.drive, machine, key, offline)
    }

    /// Every file under the drive, by path.
    fn files_on_the_drive(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.drive, &mut out);
        out
    }
}

#[allow(clippy::unnecessary_wraps)] // the shape `make_with` takes
fn served(bin: &Path) -> crate::error::Result<Vec<String>> {
    std::fs::create_dir_all(bin).expect("bin");
    std::fs::write(bin.join(PROGRAM), "a program").expect("program");
    Ok(vec![PROGRAM.to_owned()])
}

fn offline(_: &Node) -> crate::error::Result<()> {
    Err(CliError::Usage(
        "127.0.0.1:9: connection refused".to_owned(),
    ))
}

fn refusal(result: crate::error::Result<String>) -> String {
    match result {
        Ok(said) => panic!("copied where it must refuse: {said}"),
        Err(error) => error.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

#[test]
fn a_keys_passphrase_is_long_new_and_not_a_pattern() {
    let weak = |key: &str| weak_passphrase(key, MACHINE, "camille");
    assert_eq!(weak(KEY), None);
    assert_eq!(weak("correct horse battery"), None);
    for refused in [
        "short one",
        MACHINE,
        "aaaaaaaaaaaaaaaa",
        "abcabcabcabc",
        "123456789012",
        "987654321098",
        "abcdefghijklmn",
        "Camille2026!",
        "camillecamille12",
    ] {
        assert!(weak(refused).is_some(), "{refused:?} was accepted");
    }
    assert!(weak("short").is_some_and(|why| why.contains("12")));
    assert!(weak(MACHINE).is_some_and(|why| why.contains("this machine's")));
    // A username too short to mean anything does not eat the passphrase.
    assert_eq!(weak_passphrase(KEY, MACHINE, "a"), None);
}

#[test]
fn the_key_must_hold_the_programs_and_the_files() {
    let mib = 1024 * 1024;
    assert!(room(Some(40 * mib), Some(100 * mib), Some(200 * mib)).is_ok());
    let short = room(Some(40 * mib), Some(100 * mib), Some(120 * mib)).expect_err("too small");
    assert!(
        short.contains("120.0 MiB free") && short.contains("140.0 MiB"),
        "{short}"
    );
    // The programs' size unknown: about 45 MiB is counted for them.
    assert!(room(None, None, Some(30 * mib)).is_err());
    let unknown = room(Some(mib), None, Some(30 * mib)).expect("room");
    assert!(unknown.contains("not known"), "{unknown}");
    // A drive that says nothing is refused: free space is readable on every
    // system this ships for, so a drive that cannot say is one that is
    // failing, or gone, under the copy.
    let silent = room(Some(mib), Some(mib), None).expect_err("a drive that says nothing");
    assert!(
        silent.contains("could not read the free space") && silent.contains("2.0 MiB"),
        "{silent}"
    );
}

#[test]
fn the_accounts_size_is_read_from_the_daemons_status() {
    let status = "account\n  username  camille\n\ndisk used by this node\n  your content    1.2 GiB\n  vault           3.0 GiB\n";
    let read = content_in(status).expect("read");
    let gib = 1u64 << 30;
    assert!(
        read >= gib + gib / 5 && read <= gib + gib * 3 / 10,
        "{read}"
    );
    assert_eq!(content_in("  your content    512 B\n"), Some(512));
    assert_eq!(content_in("no such line"), None);
}

#[test]
fn the_background_copy_reads_two_passphrases_and_nothing_else() {
    let passes = read_passphrases(&b"machine one\nkey two\n"[..]).expect("read");
    assert_eq!(passes.machine.as_str(), "machine one");
    assert_eq!(passes.key.as_str(), "key two");
    assert_eq!(format!("{passes:?}"), "Passphrases(redacted)");
    assert!(read_passphrases(&b"only one\n"[..]).is_err());
}

#[test]
fn the_tray_opens_settings_at_the_keys_section() {
    use clap::Parser as _;
    let url = "http://127.0.0.1:1/#t=00".to_owned();
    assert_eq!(
        crate::setup::web::section_url(url.clone(), Some("key")),
        "http://127.0.0.1:1/#t=00&section=key"
    );
    assert_eq!(crate::setup::web::section_url(url.clone(), None), url);
    assert!(crate::Cli::try_parse_from(["itsanas", "settings", "--section", "key"]).is_ok());
    assert!(
        crate::Cli::try_parse_from(["itsanas", "settings", "--section", "x&t=1"]).is_err(),
        "a section is a name the page knows, never text added to its address"
    );
}

// ---------------------------------------------------------------------------
// The copy
// ---------------------------------------------------------------------------

/// The whole run, offline: the key holds the programs, the account in its
/// home with the coordinator, its synced folder beside it written so it
/// moves with the drive, and it says it will enrol itself. The status file
/// says done.
#[test]
fn a_copy_made_offline_is_complete_and_enrols_later() {
    let world = World::new();
    let said = world.copy(MACHINE, KEY).expect("copied");
    assert!(said.contains("enrols itself"), "{said}");
    assert!(
        said.contains("ITSaNAS.cmd") && said.contains("itsanas.sh"),
        "{said}"
    );

    let root = world.root();
    assert!(root.join("bin").join(PROGRAM).is_file());
    assert!(portable::is_portable(&world.key_home()));
    assert!(root.join(FOLDER).is_dir());
    let text = std::fs::read_to_string(Node::config_path(&world.key_home())).expect("config");
    assert!(text.contains("folder = ../Fichiers"), "{text}");
    let config = Config::load(&Node::config_path(&world.key_home())).expect("config");
    assert_eq!(config.coordinator, world.node.config.coordinator);
    assert_eq!(
        config.coordinator_device,
        world.node.config.coordinator_device
    );
    assert_eq!(config.folder, Some(root.join(FOLDER)));
    assert!(
        !world.key_home().join(crate::REGISTERED).exists(),
        "not enrolled, so not marked"
    );

    let status = std::fs::read_to_string(world.home.join(STATUS)).expect("status");
    assert!(
        status.starts_with("{\"step\":\"done\",\"done\":true,"),
        "{status}"
    );
}

/// A WRONG PASSPHRASE: typed for this machine, it opens nothing, and
/// nothing is written on the drive -- not the programs, not a home.
#[test]
fn red_team_a_wrong_machine_passphrase_writes_nothing_on_the_key() {
    let world = World::new();
    let said = refusal(world.copy("not this machine's passphrase", KEY));
    assert!(matches!(
        world.copy("not this machine's passphrase", KEY),
        Err(CliError::Unlock)
    ));
    assert!(
        std::fs::read_dir(&world.drive)
            .expect("drive")
            .next()
            .is_none(),
        "something was written on the key after a wrong passphrase ({said})"
    );
    let status = std::fs::read_to_string(world.home.join(STATUS)).unwrap_or_default();
    assert!(
        !status.contains("not this machine's"),
        "the passphrase is in the status"
    );
}

/// TWO ACCOUNTS ON ONE KEY: a key that already holds an account is never
/// written over, whoever's it is.
#[test]
fn red_team_an_account_on_the_key_is_never_overwritten() {
    let world = World::new();
    let home = world.key_home();
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(home.join("keystore.bin"), b"somebody's sealed keys").expect("keystore");
    let said = refusal(world.copy(MACHINE, KEY));
    assert!(said.contains("already holds an account"), "{said}");
    assert_eq!(
        std::fs::read(home.join("keystore.bin")).expect("keystore"),
        b"somebody's sealed keys"
    );
    assert!(
        !world.root().join("bin").exists(),
        "the programs were written anyway"
    );
}

/// The key's keystore opens with the key's passphrase, and NOT with this
/// machine's: one guessed passphrase does not open both.
#[test]
fn red_team_the_key_opens_with_its_own_passphrase_only() {
    let world = World::new();
    world.copy(MACHINE, KEY).expect("copied");
    let home = world.key_home();
    assert!(itsanas_node::node::Identity::open(&home, KEY).is_ok());
    assert!(matches!(
        itsanas_node::node::Identity::open(&home, MACHINE),
        Err(CliError::Unlock)
    ));
}

/// The key is the same account on a device of its own: losing it withdraws
/// one enrolment, not this machine's.
#[test]
fn red_team_the_key_is_this_account_on_a_device_of_its_own() {
    let world = World::new();
    world.copy(MACHINE, KEY).expect("copied");
    let key = itsanas_node::node::Identity::open(&world.key_home(), KEY).expect("open");
    assert_eq!(key.user.user_id(), world.node.store.owner());
    assert_ne!(key.device.device_id(), world.node.store.device_id());
    assert_eq!(key.config.username, "camille");
}

/// THE WORDS ON THE KEY: no file on it holds the 24 words, nor the master
/// secret in clear, nor either passphrase -- and neither does the status
/// file the page reads.
#[test]
fn red_team_no_file_on_the_key_holds_the_24_words() {
    let world = World::new();
    world.copy(MACHINE, KEY).expect("copied");
    let master = itsanas_crypto::MasterSecret::from_recovery_phrase(&world.phrase).expect("master");
    let words: Vec<&str> = world.phrase.split_whitespace().collect();
    let mut secrets: Vec<Vec<u8>> = vec![
        world.phrase.as_bytes().to_vec(),
        master.expose().to_vec(),
        MACHINE.as_bytes().to_vec(),
        KEY.as_bytes().to_vec(),
    ];
    // Any three words in a row, whatever separates them.
    for three in words.windows(3) {
        for separator in [" ", "\n", "\r\n", ","] {
            secrets.push(three.join(separator).into_bytes());
        }
    }
    let mut files = world.files_on_the_drive();
    assert!(files.len() >= 6, "{files:?}");
    files.push(world.home.join(STATUS));
    for file in files {
        let bytes = std::fs::read(&file).expect("read");
        for secret in &secrets {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_slice()),
                "{} holds a secret in clear",
                file.display()
            );
        }
    }
}

/// A WEAK KEY: a short passphrase, this machine's own, or a pattern is
/// refused before anything is written on the drive.
#[test]
fn red_team_a_weak_or_reused_key_passphrase_is_refused_before_anything_is_written() {
    let world = World::new();
    for weak in ["short", MACHINE, "123456789012", "camille12345"] {
        let said = refusal(world.copy(MACHINE, weak));
        assert!(said.contains("Nothing was written"), "{weak:?}: {said}");
        assert!(
            std::fs::read_dir(&world.drive)
                .expect("drive")
                .next()
                .is_none(),
            "{weak:?} was refused after something was written"
        );
    }
}

/// THE COPY THAT COPIES ITSELF: a key inside the account's home, vault or
/// synced folder is refused before any question is asked.
#[test]
fn red_team_a_key_inside_the_accounts_home_or_folder_is_refused() {
    let world = World::new();
    let config = Config::load(&Node::config_path(&world.home)).expect("config");
    let folder = config.folder.clone().expect("folder");
    let vault = Node::vault_path(&world.home, &config);
    for inside in [
        world.home.join("sub"),
        folder.join("sub"),
        vault.clone(),
        folder,
    ] {
        let said = check_target(&world.home, &config, &inside).expect_err("refused");
        assert!(said.to_string().contains("inside"), "{said}");
        let said = refusal(world.copy_with(&inside, MACHINE, KEY, offline));
        assert!(
            !inside.join(portable::DIR).exists(),
            "a copy was written inside the account ({said})"
        );
    }
    assert!(check_target(&world.home, &config, Path::new("relative")).is_err());
    assert!(check_target(&world.home, &config, &world.drive).is_ok());
}

/// THE KEY MADE OFFLINE: not enrolled when it was written, it enrols itself
/// at its first daemon start, once, marks it, and a failure leaves it to
/// try again at the next start.
#[test]
fn red_team_an_unregistered_key_enrols_itself_at_its_first_start() {
    let world = World::new();
    world.copy(MACHINE, KEY).expect("copied");
    let key = Node::open(&world.key_home(), KEY).expect("open the key");
    let marker = key.home.join(crate::REGISTERED);

    let said = enrol_at_start(&key, offline).expect("tried");
    assert!(said.contains("next start"), "{said}");
    assert!(!marker.exists(), "marked enrolled when it was refused");

    let mut asked = 0;
    let said = enrol_at_start(&key, |_| {
        asked += 1;
        Ok(())
    })
    .expect("enrolled");
    assert_eq!(asked, 1);
    assert!(said.contains("127.0.0.1:9"), "{said}");
    assert!(marker.is_file(), "enrolled and not marked");

    assert!(
        enrol_at_start(&key, |_| panic!("enrolled twice")).is_none(),
        "a marked node enrols again at every start"
    );
}

/// Enrolled at once when the coordinator answers, and marked by the
/// registrar, as `register_and_announce` does.
#[test]
fn a_key_enrols_at_once_when_the_coordinator_answers() {
    let world = World::new();
    let said = world
        .copy_with(&world.drive, MACHINE, KEY, |node| {
            std::fs::write(node.home.join(crate::REGISTERED), "127.0.0.1:9").expect("marker");
            Ok(())
        })
        .expect("copied");
    assert!(said.contains("enrolled with 127.0.0.1:9"), "{said}");
}

/// What the Settings page reads back is what each step wrote, quotes, line
/// breaks and all; a file that is not the copy's own reads as nothing.
#[test]
fn the_page_reads_back_each_step_the_copy_wrote() {
    let dir = tempfile::tempdir().expect("temp dir");
    let status = super::Status(dir.path());
    assert!(super::progress(dir.path()).is_none(), "no run, no progress");
    status.step(
        "programs",
        "writing \"the programs\"\nfor C:\\ and /Volumes",
    );
    let (read, _) = super::progress(dir.path()).expect("progress");
    assert_eq!(read.step, "programs");
    assert!(!read.done);
    assert_eq!(read.said, "writing \"the programs\"\nfor C:\\ and /Volumes");
    assert_eq!(read.error, None);
    status.ended(&Err(CliError::Usage("no room\u{1}left".to_owned())));
    let (read, _) = super::progress(dir.path()).expect("progress");
    assert!(read.done);
    assert_eq!(read.step, "failed");
    assert_eq!(read.error.as_deref(), Some("no room\u{1}left"));
    std::fs::write(dir.path().join(STATUS), "{\"step\":\"x\"}").expect("garbage");
    assert!(super::progress(dir.path()).is_none());
    super::forget_progress(dir.path());
    assert!(!dir.path().join(STATUS).exists());
}

/// The kind of each refusal is kept for the page, which says it in the
/// person's language.
#[test]
fn each_refusal_of_a_target_says_its_kind() {
    let world = World::new();
    let config = Config::load(&Node::config_path(&world.home)).expect("config");
    let kind = |dir: &Path| {
        super::refusal(&world.home, &config, dir)
            .err()
            .map(|refused| refused.why)
    };
    assert_eq!(kind(Path::new("KEY")), Some("path"));
    assert_eq!(kind(&world.drive.join("x\ny")), Some("path"));
    assert_eq!(kind(&world.home), Some("inside"));
    assert_eq!(kind(&world.drive), None);
    let held = world.key_home();
    std::fs::create_dir_all(&held).expect("home");
    std::fs::write(held.join("keystore.bin"), b"x").expect("keystore");
    assert_eq!(kind(&world.drive), Some("account"));
    let (fits, _) = super::assess(&world.home, &config, &world.drive, Some(1), None, None);
    assert_eq!(fits.err().map(|refused| refused.why), Some("account"));
}

// ---------------------------------------------------------------------------
// A hostile key, a target that is not what it says, two copies at once
// ---------------------------------------------------------------------------

/// A folder outside the drive, with one file the key must never touch.
#[cfg(unix)]
struct Victim {
    dir: tempfile::TempDir,
}

#[cfg(unix)]
impl Victim {
    const TEXT: &'static str = "export PATH=the victim's own";

    fn new() -> Self {
        let dir = tempfile::tempdir().expect("victim");
        std::fs::write(dir.path().join(".bashrc"), Self::TEXT).expect("bashrc");
        Self { dir }
    }

    fn bashrc(&self) -> PathBuf {
        self.dir.path().join(".bashrc")
    }

    /// Unchanged: the one file, as it was, and nothing else written there.
    fn untouched(&self, what: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::read_to_string(self.bashrc()).expect("bashrc"),
            Self::TEXT,
            "{what}: the victim's file was overwritten"
        );
        let mode = std::fs::metadata(self.bashrc())
            .expect("meta")
            .permissions()
            .mode();
        assert_ne!(
            mode & 0o111,
            0o111,
            "{what}: the victim's file was chmodded"
        );
        let names: Vec<_> = std::fs::read_dir(self.dir.path())
            .expect("victim")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [".bashrc"],
            "{what}: written into the victim's folder"
        );
    }
}

/// A HOSTILE KEY: `ITSaNAS/README.txt` a link to the person's `~/.bashrc`.
/// The copy is refused and the file is as it was.
#[cfg(unix)]
#[test]
fn red_team_a_readme_on_the_key_that_links_out_is_refused() {
    let world = World::new();
    let victim = Victim::new();
    std::fs::create_dir_all(world.root()).expect("root");
    std::os::unix::fs::symlink(victim.bashrc(), world.root().join("README.txt")).expect("link");
    let said = refusal(world.copy(MACHINE, KEY));
    assert!(said.contains("link"), "{said}");
    victim.untouched("README.txt -> ~/.bashrc");
}

/// A HOSTILE KEY: `ITSaNAS` itself a link to the person's home folder.
#[cfg(unix)]
#[test]
fn red_team_an_itsanas_folder_on_the_key_that_links_out_is_refused() {
    let world = World::new();
    let victim = Victim::new();
    std::os::unix::fs::symlink(victim.dir.path(), world.root()).expect("link");
    let said = refusal(world.copy(MACHINE, KEY));
    assert!(said.contains("link"), "{said}");
    victim.untouched("ITSaNAS -> ~");
}

/// A HOSTILE KEY: `home/keystore.bin` a dangling link. It counts as an
/// account on the key; the sealed master is never written through it.
#[cfg(unix)]
#[test]
fn red_team_a_dangling_keystore_link_on_the_key_is_refused() {
    let world = World::new();
    let victim = Victim::new();
    std::fs::create_dir_all(world.key_home()).expect("home");
    let target = victim.dir.path().join("planted");
    std::os::unix::fs::symlink(&target, world.key_home().join("keystore.bin")).expect("link");
    let said = refusal(world.copy(MACHINE, KEY));
    assert!(said.contains("already holds an account"), "{said}");
    assert!(
        !target.exists(),
        "the keystore was written through the link"
    );
    victim.untouched("keystore.bin -> nowhere");
}

/// A TARGET THAT IS NOT WHAT IT SAYS: `<tmp>/other/../laptop/k` is inside
/// this account's home however it is spelled, and a link that leads into
/// the synced folder is inside it too.
#[test]
fn red_team_a_target_that_leads_back_into_the_account_is_refused() {
    let world = World::new();
    let config = Config::load(&Node::config_path(&world.home)).expect("config");
    let parent = world.home.parent().expect("parent");
    std::fs::create_dir_all(parent.join("other")).expect("other");
    let dotted = parent.join("other").join("..").join("laptop").join("k");
    let said = check_target(&world.home, &config, &dotted).expect_err("refused");
    assert!(said.to_string().contains("goes up a folder"), "{said}");
    // Refused outright, even where it would land on the key itself.
    let roundabout = world.drive.join("..").join("KEY");
    let refused = super::refusal(&world.home, &config, &roundabout).expect_err("refused");
    assert_eq!(refused.why, "path", "{}", refused.said);
    let said = refusal(world.copy_with(&dotted, MACHINE, KEY, offline));
    assert!(
        !world.home.join("k").exists(),
        "written in the home ({said})"
    );

    #[cfg(unix)]
    {
        let folder = config.folder.clone().expect("folder");
        std::fs::create_dir_all(folder.join("sub")).expect("folder");
        let link = parent.join("LINKED");
        std::os::unix::fs::symlink(folder.join("sub"), &link).expect("link");
        let said = check_target(&world.home, &config, &link).expect_err("refused");
        assert!(said.to_string().contains("inside"), "{said}");
        let said = refusal(world.copy_with(&link, MACHINE, KEY, offline));
        assert!(
            !folder.join("sub").join(portable::DIR).exists(),
            "a copy was written into the synced folder ({said})"
        );
    }
}

/// TWO COPIES TO ONE KEY AT ONCE: one wins, the other is refused, and the
/// key opens with the winner's passphrase -- not a keystore of one and a
/// config of the other.
#[test]
fn red_team_two_copies_to_one_key_at_once_leave_one_account() {
    const OTHER: &str = "another long passphrase for the key";
    let world = World::new();
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let racers: Vec<_> = [KEY, OTHER]
        .into_iter()
        .map(|key| {
            let (home, drive) = (world.home.clone(), world.drive.clone());
            let start = std::sync::Arc::clone(&start);
            std::thread::spawn(move || {
                let passes = Passphrases {
                    machine: Zeroizing::new(MACHINE.to_owned()),
                    key: Zeroizing::new(key.to_owned()),
                };
                start.wait();
                let result = copy(&home, &drive, &passes, Some(1024), served, offline);
                (key, result.map_err(|error| error.to_string()))
            })
        })
        .collect();
    let results: Vec<_> = racers
        .into_iter()
        .map(|racer| racer.join().expect("racer"))
        .collect();
    let won: Vec<_> = results
        .iter()
        .filter(|(_, result)| result.is_ok())
        .collect();
    assert_eq!(won.len(), 1, "{results:?}");
    let winner = won[0].0;
    for (_, result) in &results {
        if let Err(said) = result {
            assert!(
                said.contains("another copy to this key is under way")
                    || said.contains("already holds an account"),
                "{said}"
            );
        }
    }
    let home = world.key_home();
    assert!(itsanas_node::node::Identity::open(&home, winner).is_ok());
    assert!(
        !world.root().join(".copy-lock").exists(),
        "the lock was left"
    );
}

/// A COPY UNDER WAY: its lock on the key stops a second one before it
/// writes anything, and says so.
#[test]
fn red_team_a_copy_under_way_on_the_key_stops_a_second() {
    let world = World::new();
    std::fs::create_dir_all(world.root()).expect("root");
    std::fs::write(world.root().join(".copy-lock"), b"").expect("lock");
    let said = refusal(world.copy(MACHINE, KEY));
    assert!(
        said.contains("another copy to this key is under way"),
        "{said}"
    );
    assert!(!world.key_home().exists(), "written beside another copy");
    assert!(
        !world.root().join("bin").exists(),
        "written beside another copy"
    );
    assert!(
        world.root().join(".copy-lock").exists(),
        "the other copy's lock was removed"
    );
}

/// A COPY THAT FAILS AFTER SEALING: no sealed master is left on the key,
/// the copy says so, and a retry works.
#[test]
fn red_team_a_copy_that_fails_after_sealing_leaves_no_keystore_and_a_retry_works() {
    let world = World::new();
    // The synced folder cannot be made: the step after the keystore fails.
    std::fs::create_dir_all(world.root()).expect("root");
    std::fs::write(world.root().join(FOLDER), b"in the way").expect("in the way");
    let said = refusal(world.copy(MACHINE, KEY));
    assert!(said.contains("removed"), "{said}");
    assert!(
        !world.key_home().join("keystore.bin").exists(),
        "a sealed master was left on the key: {said}"
    );
    assert!(!Node::config_path(&world.key_home()).exists(), "{said}");
    assert!(
        !world.root().join(".copy-lock").exists(),
        "the lock was left"
    );

    std::fs::remove_file(world.root().join(FOLDER)).expect("out of the way");
    world.copy(MACHINE, KEY).expect("the retry");
    assert!(itsanas_node::node::Identity::open(&world.key_home(), KEY).is_ok());
}

/// The machine's passphrase in other clothes -- another case, inside a
/// longer one, holding the key's -- and a common base dressed with digits
/// or symbols are refused.
#[test]
fn a_keys_passphrase_is_not_the_machines_in_other_clothes_nor_a_common_base() {
    let weak = |key: &str| weak_passphrase(key, MACHINE, "camille");
    for refused in [
        "THE PASSPHRASE OF THIS LAPTOP",
        "The Passphrase Of This Laptop!",
        "my the passphrase of this laptop",
        "passphrase of this",
        "Password2026!!",
        "motdepasse1234",
        "AZERTY123456!",
        "qwerty-2026-1234!",
        "itsanas2026!!",
        "1234567!123456",
    ] {
        assert!(weak(refused).is_some(), "{refused:?} was accepted");
    }
    assert!(
        weak("the passphrase of THIS laptop")
            .is_some_and(|why| why.starts_with("is this machine's passphrase")),
        "case is no disguise"
    );
    // A short machine passphrase is no reason to refuse every key holding it.
    assert_eq!(weak_passphrase(KEY, "key", "camille"), None);
    assert_eq!(weak("password manager entries galore"), None);
}
