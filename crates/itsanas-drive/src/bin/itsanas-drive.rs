//! Show an ITSaNAS account as a folder, and keep syncing while it is showing.
//!
//! # Why this is its own program
//!
//! It links `ProjectedFSLib.dll`, which does not exist until somebody turns the
//! Windows optional feature on. A binary that links a missing DLL does not fail
//! gracefully — it does not start at all, `STATUS_DLL_NOT_FOUND`, before
//! `main`. Measured by wiring this into `itsanas.exe` and watching the command
//! line stop working on a machine where the feature was off. So the daemon and
//! the command line link none of it, and this program is the only thing that
//! can be broken by a missing feature.
//!
//! # One process, because the store admits one writer
//!
//! The projection answers the file manager from several Windows threads and a
//! sync round runs between them, all under one lock. Two processes cannot share
//! a node: the second is refused at the door. So this syncs too, on the
//! interval `itsanas-policy` chooses, exactly as the daemon does — and while it
//! is running, the daemon must not be.

fn main() {
    #[cfg(windows)]
    {
        if let Err(complaint) = windows::run() {
            eprintln!("itsanas-drive: {complaint}");
            std::process::exit(1);
        }
    }

    #[cfg(not(windows))]
    eprintln!(
        "itsanas-drive: showing an account as a folder needs the Windows \
         Projected File System. On Linux the equivalent is FUSE and it is not \
         built; `itsanas folder <path>` keeps a real directory in step, which \
         is what the Pi and the VM use."
    );
}

#[cfg(windows)]
mod windows {
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use itsanas_drive::{Entry, Known, listing, logical, projfs};
    use itsanas_node::Node;
    use itsanas_store::Presence;

    /// Everything the account knows, as the projection needs it.
    struct Account {
        node: Arc<Mutex<Node>>,
    }

    impl Account {
        /// The listing, freshly derived. Not cached: the catalogue is derived
        /// from the log rather than recorded, so it cannot be stale, and a
        /// cache here would be the one thing in the path that could.
        fn files(&self) -> Vec<(String, u64, u64, bool)> {
            let Ok(node) = self.node.lock() else {
                return Vec::new();
            };
            let Ok(catalogue) = itsanas_store::catalogue(&node.store, &node.vault) else {
                return Vec::new();
            };
            catalogue
                .files
                .into_iter()
                .map(|file| {
                    (
                        file.path,
                        file.size,
                        file.modified_unix,
                        file.presence == Presence::Local,
                    )
                })
                .collect()
        }

        fn entries(&self, directory: &str) -> Vec<Entry> {
            let files = self.files();
            listing(
                files.iter().map(|(path, size, modified, here)| Known {
                    path,
                    size: *size,
                    modified_unix: *modified,
                    here: *here,
                }),
                directory,
            )
        }

        /// The bytes of one file, fetching it first if this machine does not
        /// hold it.
        ///
        /// Blocking, and that is the honest limit of this version: opening a
        /// file that is not here takes as long as downloading it, and the file
        /// manager shows its ordinary "opening" state throughout. Fine for a
        /// document; unpleasant for a film over a slow link.
        fn content(&self, path: &str) -> io::Result<Vec<u8>> {
            let node = self
                .node
                .lock()
                .map_err(|_| io::Error::other("the node lock was poisoned by a panic"))?;

            if let Ok(Some(bytes)) = node.store.read_file(path) {
                return Ok(bytes);
            }

            let chunks = itsanas_store::chunks_for(&node.store, &node.vault, path)
                .map_err(io::Error::other)?
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_owned()))?;
            let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

            for target in &node.config.peers {
                let Ok(mut client) = itsanas_net::PeerClient::connect(
                    target.as_str(),
                    &node.device,
                    node.store.owner(),
                    None,
                ) else {
                    continue;
                };
                if itsanas_net::session::fetch_only(&node.store, &node.vault, &mut client, &wanted)
                    .is_err()
                {
                    continue;
                }
                if let Ok(Some(bytes)) = node.store.read_file(path) {
                    return Ok(bytes);
                }
            }

            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!("no machine that is up would serve {path}"),
            ))
        }
    }

    /// Windows file times are hundreds of nanoseconds since 1601; the account
    /// keeps seconds since 1970.
    fn to_filetime(unix: u64) -> i64 {
        const EPOCH_DIFFERENCE: i64 = 11_644_473_600;
        // Saturating throughout: a date the account cannot represent should
        // show as a strange date in a file manager, not wrap into 1601.
        i64::try_from(unix)
            .unwrap_or(i64::MAX)
            .saturating_add(EPOCH_DIFFERENCE)
            .saturating_mul(10_000_000)
    }

    fn to_info(entry: &Entry) -> projfs::Info {
        match entry {
            // A directory here is a prefix several paths share rather than
            // something the account stores, so it has no date of its own. Zero
            // shows as an absent time; a clock reading would show as a fact.
            Entry::Directory(name) => projfs::Info {
                name: name.clone(),
                is_dir: true,
                size: 0,
                written: 0,
            },
            Entry::File {
                name,
                size,
                modified_unix,
                ..
            } => projfs::Info {
                name: name.clone(),
                is_dir: false,
                size: *size,
                written: to_filetime(*modified_unix),
            },
        }
    }

    // One trait now, instead of two plus a blanket implementation plus a
    // global cache. `src/projfs.rs` owns the enumeration cursors, so this is
    // three questions and no state.
    impl projfs::Source for Account {
        fn list(&self, directory: &str) -> io::Result<Vec<projfs::Info>> {
            let directory = logical(directory);
            Ok(self.entries(&directory).iter().map(to_info).collect())
        }

        fn stat(&self, path: &str) -> io::Result<projfs::Info> {
            let wanted = logical(path);
            let name = wanted.rsplit('/').next().unwrap_or(&wanted).to_owned();
            let parent = match wanted.rfind('/') {
                Some(at) => wanted[..at].to_owned(),
                None => String::new(),
            };

            self.entries(&parent)
                .iter()
                .find(|entry| entry.name() == name)
                .map(to_info)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, wanted))
        }

        fn read(&self, path: &str, offset: u64, into: &mut [u8]) -> io::Result<()> {
            let wanted = logical(path);
            let content = self.content(&wanted)?;

            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(content.len());
            let end = start.saturating_add(into.len()).min(content.len());
            let slice = &content[start..end];
            into[..slice.len()].copy_from_slice(slice);
            Ok(())
        }
    }

    pub fn run() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        let mut home: Option<PathBuf> = None;
        let mut at: Option<PathBuf> = None;
        let mut interval = 300u64;

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--home" => home = args.next().map(PathBuf::from),
                "--interval" => {
                    interval = args
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(300);
                }
                "-h" | "--help" => {
                    println!(
                        "itsanas-drive [--home <dir>] [--interval <seconds>] <folder>\n\n\
                         Shows the whole account at <folder>. Files that are not on this\n\
                         machine appear with their name, size and date, and download when\n\
                         opened. Syncs while it is showing, so the daemon must not be running."
                    );
                    return Ok(());
                }
                other => at = Some(PathBuf::from(other)),
            }
        }

        let at = at.ok_or_else(|| {
            "give a folder to show the account at, e.g. `itsanas-drive C:\\ITSaNAS`".to_owned()
        })?;

        let home = home
            .or_else(default_home)
            .ok_or_else(|| "could not work out where this node lives; pass --home".to_owned())?;

        let passphrase = passphrase()?;
        let node = Node::open(&home, &passphrase).map_err(|error| error.to_string())?;

        std::fs::create_dir_all(&at).map_err(|error| format!("{}: {error}", at.display()))?;

        let owner = node.store.owner();
        let peers = node.config.peers.clone();
        let keeping = node.config.keeping();
        let device = itsanas_crypto::DeviceKeys::from_seed(&node.device.seed());
        let shared = Arc::new(Mutex::new(node));

        let account = Account {
            node: Arc::clone(&shared),
        };

        let _mount = projfs::mount(&at, account).map_err(|why| {
            format!(
                "{why}.\n  \
                 The projected file system is an optional Windows feature. In an\n  \
                 Administrator PowerShell:\n    \
                 Enable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS -All"
            )
        })?;

        println!("showing your account at {}", at.display());
        println!("  every file is listed; opening one that is not here downloads it");
        if peers.is_empty() {
            println!("  no machine is configured, so nothing can be downloaded yet");
        }
        println!();
        println!("Ctrl-C to stop showing it.");
        let _ = io::stdout().flush();

        loop {
            std::thread::sleep(std::time::Duration::from_secs(interval));
            for target in &peers {
                let Ok(mut client) =
                    itsanas_net::PeerClient::connect(target.as_str(), &device, owner, None)
                else {
                    continue;
                };
                let Ok(guard) = shared.lock() else {
                    return Err("the node lock was poisoned by a panic".to_owned());
                };
                match itsanas_node::round(
                    &guard.store,
                    &guard.vault,
                    &keeping,
                    &mut client,
                    itsanas_net::session::Scope::Everything,
                ) {
                    Ok((report, _)) if report.changed_anything() => {
                        println!("{target}: received {} file(s)", report.pull.adopted);
                    }
                    Ok(_) => {}
                    Err(error) => println!("{target}: failed ({error})"),
                }
            }
        }
    }

    fn default_home() -> Option<PathBuf> {
        std::env::var_os("ITSANAS_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs_home().map(|home| home.join(".itsanas")))
    }

    fn dirs_home() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }

    /// The passphrase, from the environment or from the file the installer
    /// leaves for the daemon.
    ///
    /// Never prompted: this program is started the same way the daemon is, and
    /// a prompt would be a thing nobody is there to answer.
    fn passphrase() -> Result<String, String> {
        if let Ok(value) = std::env::var("ITSANAS_PASSPHRASE") {
            return Ok(value);
        }

        let path = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|local| local.join("itsanas").join("passphrase.txt"))
            .ok_or_else(|| "no ITSANAS_PASSPHRASE and no LOCALAPPDATA".to_owned())?;

        std::fs::read_to_string(&path)
            .map(|text| text.trim().to_owned())
            .map_err(|error| {
                format!(
                    "set ITSANAS_PASSPHRASE, or leave it in {} ({error})",
                    Path::new(&path).display()
                )
            })
    }
}
