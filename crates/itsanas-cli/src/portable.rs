//! A copy of ITSaNAS that lives on a drive and runs wherever it is plugged
//! in: `itsanas portable DIR`, or the Settings page.
//!
//! What it is, said in its own README so nobody has to guess:
//!
//! - **One program, for one system.** The copy is this very binary; a drive
//!   carried between Windows and a Mac opens only on the kind of machine that
//!   made it. A second system's copy goes in by running its own `itsanas
//!   portable` on the same drive, beside the first.
//! - **Explicit, not detected.** The launcher passes `--home` next to itself.
//!   Nothing in the program looks for a marker beside its own file to change
//!   where its keys are: a rule like that redirects an installed copy too.
//! - **A client, never a host.** A drive in a different house every week is
//!   no place to keep other people's data, and counting it as one would
//!   distort durability. Its home holds a `portable` file, and a daemon
//!   started on such a home refuses to host anybody (`daemon.rs`).
//! - **The keys travel with it.** They are sealed by the passphrase; a lost
//!   drive is an offline attack on that passphrase, which Argon2 slows and
//!   does not stop. The README and the command both say so.

use std::path::{Path, PathBuf};

use crate::error::{CliError, Result};

/// The directory made on the drive.
pub const DIR: &str = "ITSaNAS";

/// The file in a portable home that makes its daemon a client only.
pub const MARKER: &str = crate::config::PORTABLE_MARKER;

/// Whether the node at `home` is a portable one.
pub fn is_portable(home: &Path) -> bool {
    home.join(MARKER).is_file()
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CliError + '_ {
    move |source| CliError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Refuse `path` when it is a link. A hostile key could hold one where the
/// copy writes -- `README.txt` to `~/.bashrc`, `ITSaNAS` or `home` to `~` --
/// and every write through it would land on the person's own files.
/// `symlink_metadata` reads the link, never what it points at. Missing is
/// fine: it will be made.
///
/// # Errors
///
/// When `path` is a link (a junction on Windows reads as one too).
pub(crate) fn no_link(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || is_reparse(&meta) => {
            Err(CliError::Usage(format!(
                "{} on this drive is a link to somewhere else, which a copy would write \
                 through; nothing was written there. Remove it, or use another drive",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(path)(error)),
    }
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse(_: &std::fs::Metadata) -> bool {
    false
}

/// [`no_link`] for `dir` and everything directly in it.
///
/// # Errors
///
/// When `dir` or one of its entries is a link.
pub(crate) fn no_links_in(dir: &Path) -> Result<()> {
    no_link(dir)?;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io(dir)(error)),
    };
    for entry in entries {
        no_link(&entry.map_err(io(dir))?.path())?;
    }
    Ok(())
}

/// Write `path` whole, through a new file beside it renamed over it: the
/// rename replaces whatever is at `path` (a link included) rather than
/// writing through it, and the new file is made with `create_new`, which
/// refuses a link planted at the temporary name rather than following it.
fn replace(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staging = path.with_file_name(format!(".{name}.itsanas-new"));
    // A leftover of a run that stopped here. Removing a link removes the
    // link, never what it points at.
    let _ = std::fs::remove_file(&staging);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            write(&mut file)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&staging, path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(io(path)(error));
    }
    Ok(())
}

/// [`replace`], with text.
fn replace_text(path: &Path, text: &str) -> Result<()> {
    replace(path, |file| {
        std::io::Write::write_all(file, text.as_bytes())
    })
}

fn system() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn readme() -> String {
    "ITSaNAS, portable copy, for every computer\r\n\
     \r\n\
     Windows: double-click ITSaNAS.cmd\r\n\
     Mac:     double-click itsanas.command (or `sh itsanas.command` in the Terminal)\r\n\
     Linux:   sh itsanas.sh\r\n\
     \r\n\
     With no arguments it opens the setup page, or, once an account is on this key,\r\n\
     runs it in that window: closing the window stops it. With arguments it runs\r\n\
     that command, e.g. `sh itsanas.command status`.\r\n\
     \r\n\
     Your keys live in the home folder next to it, sealed by your passphrase.\r\n\
     Whoever finds this drive can try passphrases offline for as long as they\r\n\
     like: use a long one, and keep your 24 words away from the drive.\r\n\
     \r\n\
     This copy syncs your files and hosts nobody else's: a drive that moves\r\n\
     from house to house is not a safe place for other people's data.\r\n"
        .to_owned()
}

/// The Windows launcher. No parenthesised block: a `)` in an argument
/// would end it. With no arguments: the account on the key, run in this
/// window (`daemon`, which asks its passphrase here), or `setup` when the key
/// holds none yet.
const CMD: &str = "@echo off\r\n\
    if \"%~1\"==\"\" goto plain\r\n\
    \"%~dp0bin\\itsanas-x86_64-pc-windows-msvc.exe\" --home \"%~dp0home\" %*\r\n\
    exit /b %errorlevel%\r\n\
    :plain\r\n\
    if exist \"%~dp0home\\keystore.bin\" goto account\r\n\
    \"%~dp0bin\\itsanas-x86_64-pc-windows-msvc.exe\" --home \"%~dp0home\" setup\r\n\
    exit /b %errorlevel%\r\n\
    :account\r\n\
    \"%~dp0bin\\itsanas-x86_64-pc-windows-msvc.exe\" --home \"%~dp0home\" daemon\r\n";

/// The Mac and Linux launcher (`itsanas.command`, double-clicked in the
/// Finder, and `itsanas.sh`): picks this computer's program. A key mounted
/// without the right to run programs (FAT32 on Linux, `noexec`) runs a copy
/// from the temporary folder instead. With no arguments: the account on the
/// key in the foreground (`daemon`), or `setup` when the key holds none.
const SH: &str = r#"#!/bin/sh
# ITSaNAS portable: picks the program for this computer, keeps the home on the key.
here=$(cd "$(dirname "$0")" && pwd)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) b=itsanas-aarch64-apple-darwin ;;
  Darwin-x86_64) b=itsanas-x86_64-apple-darwin ;;
  Linux-x86_64) b=itsanas-x86_64-unknown-linux-gnu ;;
  Linux-aarch64|Linux-arm64) b=itsanas-aarch64-unknown-linux-gnu ;;
  *) echo "ITSaNAS: no program on this key for $(uname -s) $(uname -m)"; exit 1 ;;
esac
exe="$here/bin/$b"
if [ ! -x "$exe" ]; then
  # A fresh private directory: a predictable name in a shared /tmp could be
  # made first by another user, with their program in it.
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/itsanas-portable.XXXXXX") || exit 1
  cp "$exe" "$tmp/$b" && chmod 700 "$tmp/$b" && exe="$tmp/$b" || exit 1
fi
if [ $# -eq 0 ]; then
  if [ -f "$here/home/keystore.bin" ]; then set -- daemon; else set -- setup; fi
fi
exec "$exe" --home "$here/home" "$@"
"#;

/// The launchers, by name.
const LAUNCHERS: [(&str, &str); 3] = [
    ("ITSaNAS.cmd", CMD),
    ("itsanas.command", SH),
    ("itsanas.sh", SH),
];

/// Make (or refresh) the portable copy in `dir`, with the programs of every
/// system of this version's signed release; what to tell the person.
///
/// Refreshing replaces the programs and the launchers, never the home: the
/// keys on the drive are not touched.
///
/// # Errors
///
/// A path that is not a full one, a drive that is not there, or the disk.
pub fn make(dir: &Path) -> Result<String> {
    make_with(dir, |bin| {
        let curl = crate::update::Curl;
        crate::update::Updater::of_this_build(&curl)?.release_binaries(bin)
    })
}

/// [`make`], with where the release's programs come from passed in.
pub(crate) fn make_with(
    dir: &Path,
    fetch: impl FnOnce(&Path) -> Result<Vec<String>>,
) -> Result<String> {
    let dir = crate::jobs::check_path(dir).map_err(CliError::Usage)?;
    // A drive letter that is not plugged in gave the system's own words
    // ("the specified path is not found", os error 3): say which.
    if crate::drives::existing_ancestor(&dir).is_none() {
        return Err(CliError::Usage(format!(
            "{} does not exist: no drive or folder there (is the key plugged in, and is \
             this its letter?)",
            dir.display()
        )));
    }
    let root: PathBuf = dir.join(DIR);
    let home = root.join("home");
    let bin = root.join("bin");
    // Nothing is written through a link the drive holds: every name written
    // below is checked, right before the first write.
    no_link(&root)?;
    no_link(&home)?;
    no_link(&home.join(MARKER))?;
    no_links_in(&bin)?;
    for name in LAUNCHERS
        .iter()
        .map(|(name, _)| *name)
        .chain(["README.txt"])
    {
        no_link(&root.join(name))?;
    }
    std::fs::create_dir_all(&home).map_err(io(&home))?;
    // First: a copy that stops halfway must never leave a program that would
    // host other people's data from a drive that travels.
    replace_text(&home.join(MARKER), "this node hosts nobody: it travels\n")?;

    // Every system's program, so the key starts on any computer, not only
    // on the kind that made it (Nicolas, 2026-10-22). Without the network or
    // a release (a build from source), this one program, and it says so.
    let systems = match fetch(&bin) {
        Ok(names) => format!(
            "every system ({} programs, each checked against the signed release)",
            names.len()
        ),
        Err(why) => {
            let me = std::env::current_exe().map_err(io(Path::new("itsanas")))?;
            let name = format!(
                "itsanas-{}{}",
                crate::update::this_target().unwrap_or("this-system"),
                std::env::consts::EXE_SUFFIX
            );
            std::fs::create_dir_all(&bin).map_err(io(&bin))?;
            no_links_in(&bin)?;
            let mut program = std::fs::File::open(&me).map_err(io(&me))?;
            replace(&bin.join(&name), |file| {
                std::io::copy(&mut program, file)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
                }
                Ok(())
            })?;
            format!(
                "{} only, because the other systems' programs could not be fetched ({why})",
                system()
            )
        }
    };
    // The layout before every system had its program.
    for old in ["itsanas.exe", "itsanas", "itsanas.copy"] {
        let _ = std::fs::remove_file(root.join(old));
    }

    for (name, text) in LAUNCHERS {
        // The mode is set through the new file's own handle, before it takes
        // the launcher's name: a chmod by name would follow a link.
        replace(&root.join(name), |file| {
            std::io::Write::write_all(file, text.as_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(())
        })?;
    }
    replace_text(&root.join("README.txt"), &readme())?;

    let mut said = format!(
        "portable copy written to {} for {systems}: ITSaNAS.cmd on Windows, itsanas.command on \
         a Mac, itsanas.sh on Linux. Its keys will be sealed by your passphrase only, so choose \
         a long one, and it syncs your files without hosting anybody's",
        root.display()
    );
    if let Some(warning) = crate::drives::warning_for(&root, "the portable copy") {
        said = format!("{said}. Note: {warning}");
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: [&str; 5] = [
        "itsanas-x86_64-unknown-linux-gnu",
        "itsanas-aarch64-unknown-linux-gnu",
        "itsanas-x86_64-pc-windows-msvc.exe",
        "itsanas-aarch64-apple-darwin",
        "itsanas-x86_64-apple-darwin",
    ];

    #[allow(clippy::unnecessary_wraps)] // the shape `make_with` takes
    fn served(bin: &Path) -> Result<Vec<String>> {
        std::fs::create_dir_all(bin).expect("bin");
        for name in RELEASE {
            std::fs::write(bin.join(name), name).expect("program");
        }
        Ok(RELEASE.map(str::to_owned).to_vec())
    }

    #[test]
    fn red_team_a_portable_copy_starts_on_every_system_and_hosts_nobody() {
        let drive = tempfile::tempdir().expect("drive");
        let said = make_with(drive.path(), served).expect("made");
        let root = drive.path().join(DIR);
        assert!(said.contains("every system"), "{said}");
        for name in RELEASE {
            assert!(
                root.join("bin").join(name).is_file(),
                "no {name} on the key"
            );
        }
        // Each launcher names the program it starts, for every release target.
        assert!(
            CMD.contains(RELEASE[2]),
            "the Windows launcher starts another program"
        );
        for name in RELEASE.iter().filter(|name| **name != RELEASE[2]) {
            assert!(SH.contains(name), "no launcher picks {name}");
        }
        for launcher in ["ITSaNAS.cmd", "itsanas.command", "itsanas.sh"] {
            let text = std::fs::read_to_string(root.join(launcher)).expect("launcher");
            assert!(text.contains("--home"), "{launcher} names no home: {text}");
        }
        assert!(
            is_portable(&root.join("home")),
            "the home is not client only"
        );
        let readme = std::fs::read_to_string(root.join("README.txt")).expect("readme");
        assert!(
            readme.contains("passphrase") && readme.contains("Mac"),
            "{readme}"
        );

        // Made again, over the old one-system layout: the keys stay, the old
        // program at the root goes.
        let keys = root.join("home").join("keystore");
        std::fs::write(&keys, b"sealed").expect("keys");
        std::fs::write(root.join("itsanas.exe"), b"old layout").expect("old");
        make_with(drive.path(), served).expect("refreshed");
        assert_eq!(std::fs::read(&keys).expect("keys"), b"sealed");
        assert!(
            !root.join("itsanas.exe").exists(),
            "the old layout was left"
        );
    }

    #[test]
    fn without_the_release_the_copy_says_it_serves_this_system_only() {
        let drive = tempfile::tempdir().expect("drive");
        let said =
            make_with(drive.path(), |_| Err(CliError::Usage("offline".to_owned()))).expect("made");
        assert!(said.contains("only") && said.contains("offline"), "{said}");
        let bin = drive.path().join(DIR).join("bin");
        assert_eq!(std::fs::read_dir(bin).expect("bin").count(), 1);
    }

    /// Double-clicked with an account on the key, the launcher runs it in
    /// its window (closing the window stops it); with none, it opens setup.
    /// Run for real through `sh`, with a stand-in program that prints what
    /// it was given. The Windows launcher says the same in its own words.
    #[cfg(unix)]
    #[test]
    fn the_launcher_runs_the_account_on_the_key_or_opens_setup() {
        if !matches!(std::env::consts::OS, "linux" | "macos") {
            return;
        }
        let drive = tempfile::tempdir().expect("drive");
        // The stand-in under every unix name: the launcher asks `uname`, and
        // under an emulator (the ARM CI job runs aarch64 tests in qemu) the
        // `sh` it runs is the host's, which answers x86_64.
        make_with(drive.path(), |bin| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::create_dir_all(bin).expect("bin");
            let names = [
                "itsanas-x86_64-unknown-linux-gnu",
                "itsanas-aarch64-unknown-linux-gnu",
                "itsanas-aarch64-apple-darwin",
                "itsanas-x86_64-apple-darwin",
            ];
            for name in names {
                let program = bin.join(name);
                std::fs::write(&program, "#!/bin/sh\necho \"$@\"\n").expect("program");
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                    .expect("executable");
            }
            Ok(names.iter().map(|name| (*name).to_owned()).collect())
        })
        .expect("made");
        let root = drive.path().join(DIR);
        let launched = || {
            let output = std::process::Command::new("sh")
                .arg(root.join("itsanas.sh"))
                .output()
                .expect("sh");
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        let home = root.join("home");
        assert!(launched().ends_with("/home setup"), "{}", launched());
        std::fs::write(home.join("keystore.bin"), b"sealed").expect("account");
        assert!(launched().ends_with("/home daemon"), "{}", launched());

        assert!(
            CMD.contains("if exist \"%~dp0home\\keystore.bin\" goto account")
                && CMD.contains("--home \"%~dp0home\" daemon"),
            "{CMD}"
        );
    }

    /// A HOSTILE KEY: a link planted on it, where the copy writes, would
    /// have it overwrite (or chmod) whatever the link points at -- a
    /// `README.txt` to `~/.bashrc`, an `ITSaNAS` or a `home` to `~`. Each is
    /// refused, and what the link points at is left as it was.
    #[cfg(unix)]
    #[test]
    fn red_team_a_portable_copy_never_writes_through_a_link_on_the_key() {
        use std::os::unix::fs::PermissionsExt as _;
        // Each name, and whether it points at the victim's file or folder.
        for (planted, at_file) in [
            ("ITSaNAS", false),
            ("ITSaNAS/README.txt", true),
            ("ITSaNAS/itsanas.sh", true),
            ("ITSaNAS/ITSaNAS.cmd", true),
            ("ITSaNAS/home", false),
            ("ITSaNAS/home/portable", true),
            ("ITSaNAS/bin", false),
            ("ITSaNAS/bin/itsanas-x86_64-unknown-linux-gnu.part", true),
        ] {
            let drive = tempfile::tempdir().expect("drive");
            let victim = tempfile::tempdir().expect("victim");
            let bashrc = victim.path().join(".bashrc");
            std::fs::write(&bashrc, "the victim's own").expect("victim file");
            let link = drive.path().join(planted);
            std::fs::create_dir_all(link.parent().expect("parent")).expect("parents");
            let points_at = if at_file {
                bashrc.clone()
            } else {
                victim.path().to_path_buf()
            };
            std::os::unix::fs::symlink(&points_at, &link).expect("link");
            let made = make_with(drive.path(), served);
            assert!(made.is_err(), "{planted} as a link was written through");
            assert_eq!(
                std::fs::read_to_string(&bashrc).expect("victim file"),
                "the victim's own",
                "{planted}: the file the link points at was changed"
            );
            let left: Vec<_> = std::fs::read_dir(victim.path())
                .expect("victim")
                .flatten()
                .map(|entry| entry.file_name())
                .collect();
            assert_eq!(
                left,
                [".bashrc"],
                "{planted}: written into the victim's folder"
            );
            let permissions = std::fs::metadata(&bashrc).expect("meta").permissions();
            assert_ne!(
                permissions.mode() & 0o777,
                0o755,
                "{planted}: the victim's file was made executable"
            );
        }
    }

    #[test]
    fn a_portable_copy_needs_a_full_path() {
        assert!(make(Path::new("relative")).is_err());
    }
}
