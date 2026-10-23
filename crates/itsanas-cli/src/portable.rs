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
    std::fs::create_dir_all(&home).map_err(io(&home))?;
    // First: a copy that stops halfway must never leave a program that would
    // host other people's data from a drive that travels.
    let path = home.join(MARKER);
    std::fs::write(&path, "this node hosts nobody: it travels\n").map_err(io(&path))?;

    // Every system's program, so the key starts on any computer, not only
    // on the kind that made it (Nicolas, 2026-10-22). Without the network or
    // a release (a build from source), this one program, and it says so.
    let bin = root.join("bin");
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
            let staging = bin.join(format!("{name}.part"));
            std::fs::copy(&me, &staging).map_err(io(&staging))?;
            std::fs::rename(&staging, bin.join(&name)).map_err(io(&bin))?;
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

    for (name, text) in [
        ("ITSaNAS.cmd", CMD),
        ("itsanas.command", SH),
        ("itsanas.sh", SH),
    ] {
        let path = root.join(name);
        std::fs::write(&path, text).map_err(io(&path))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    let path = root.join("README.txt");
    std::fs::write(&path, readme()).map_err(io(&path))?;

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
        let target = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "itsanas-x86_64-unknown-linux-gnu",
            ("linux", "aarch64") => "itsanas-aarch64-unknown-linux-gnu",
            ("macos", "aarch64") => "itsanas-aarch64-apple-darwin",
            ("macos", "x86_64") => "itsanas-x86_64-apple-darwin",
            _ => return,
        };
        let drive = tempfile::tempdir().expect("drive");
        make_with(drive.path(), |bin| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::create_dir_all(bin).expect("bin");
            let program = bin.join(target);
            std::fs::write(&program, "#!/bin/sh\necho \"$@\"\n").expect("program");
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
            Ok(vec![target.to_owned()])
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

    #[test]
    fn a_portable_copy_needs_a_full_path() {
        assert!(make(Path::new("relative")).is_err());
    }
}
