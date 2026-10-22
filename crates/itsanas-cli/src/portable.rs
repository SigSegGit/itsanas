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
pub const MARKER: &str = "portable";

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

fn readme(launcher: &str) -> String {
    format!(
        "ITSaNAS, portable copy\r\n\
         \r\n\
         This program runs on: {system}. Other systems need their own copy: run\r\n\
         `itsanas portable` from that system on this same drive.\r\n\
         \r\n\
         Start it with {launcher}. With no arguments it opens the setup page in\r\n\
         your browser; with arguments it runs that command, e.g. `{launcher} status`.\r\n\
         \r\n\
         Your keys live in the home folder next to it, sealed by your passphrase.\r\n\
         Whoever finds this drive can try passphrases offline for as long as they\r\n\
         like: use a long one, and keep your 24 words away from the drive.\r\n\
         \r\n\
         This copy syncs your files and hosts nobody else's: a drive that moves\r\n\
         from house to house is not a safe place for other people's data.\r\n\
         \r\n\
         On Linux, a drive mounted `noexec` cannot run programs; mount it without.\r\n",
        system = system()
    )
}

/// Make (or refresh) the portable copy in `dir`; what to tell the person.
///
/// Refreshing replaces the program and the launcher, never the home: the
/// keys on the drive are not touched.
///
/// # Errors
///
/// A path that is not a full one, a program that cannot find itself, or the
/// disk.
pub fn make(dir: &Path) -> Result<String> {
    let dir = crate::jobs::check_path(dir).map_err(CliError::Usage)?;
    let root: PathBuf = dir.join(DIR);
    let home = root.join("home");
    std::fs::create_dir_all(&home).map_err(io(&home))?;
    // First: a copy that stops halfway must never leave a program that would
    // host other people's data from a drive that travels.
    let path = home.join(MARKER);
    std::fs::write(&path, "this node hosts nobody: it travels\n").map_err(io(&path))?;

    let me = std::env::current_exe().map_err(io(Path::new("itsanas")))?;
    let program = root.join(format!("itsanas{}", std::env::consts::EXE_SUFFIX));
    if std::fs::canonicalize(&me).ok() != std::fs::canonicalize(&program).ok() {
        // Through a temporary name: a drive pulled mid-copy leaves the old
        // program working rather than half a new one.
        let staging = root.join("itsanas.copy");
        std::fs::copy(&me, &staging).map_err(io(&staging))?;
        std::fs::rename(&staging, &program).map_err(io(&program))?;
    }

    let launcher = if cfg!(windows) {
        let path = root.join("ITSaNAS.cmd");
        // No parenthesised block: a `)` in an argument would end it.
        let text = "@echo off\r\n\
                    if \"%~1\"==\"\" goto setup\r\n\
                    \"%~dp0itsanas.exe\" --home \"%~dp0home\" %*\r\n\
                    exit /b %errorlevel%\r\n\
                    :setup\r\n\
                    \"%~dp0itsanas.exe\" --home \"%~dp0home\" setup\r\n";
        std::fs::write(&path, text).map_err(io(&path))?;
        "ITSaNAS.cmd"
    } else {
        let path = root.join("itsanas.sh");
        let text = "#!/bin/sh\n\
                    here=$(cd \"$(dirname \"$0\")\" && pwd)\n\
                    if [ $# -eq 0 ]; then set -- setup; fi\n\
                    exec \"$here/itsanas\" --home \"$here/home\" \"$@\"\n";
        std::fs::write(&path, text).map_err(io(&path))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for file in [&path, &program] {
                std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))
                    .map_err(io(file))?;
            }
        }
        "itsanas.sh"
    };
    let path = root.join("README.txt");
    std::fs::write(&path, readme(launcher)).map_err(io(&path))?;

    let mut said = format!(
        "portable copy for {} written to {}; start it with {launcher} there. Its keys will be \
         sealed by your passphrase only, so choose a long one, and it syncs your files without \
         hosting anybody's",
        system(),
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

    #[test]
    fn a_portable_copy_has_its_program_launcher_readme_and_a_client_only_home() {
        let drive = tempfile::tempdir().expect("drive");
        let said = make(drive.path()).expect("made");
        let root = drive.path().join(DIR);
        assert!(said.contains(&root.display().to_string()), "{said}");
        let program = root.join(format!("itsanas{}", std::env::consts::EXE_SUFFIX));
        assert!(program.is_file(), "no program");
        let launcher = if cfg!(windows) {
            "ITSaNAS.cmd"
        } else {
            "itsanas.sh"
        };
        let text = std::fs::read_to_string(root.join(launcher)).expect("launcher");
        assert!(
            text.contains("--home"),
            "the launcher names no home: {text}"
        );
        assert!(
            is_portable(&root.join("home")),
            "the home is not client only"
        );
        let readme = std::fs::read_to_string(root.join("README.txt")).expect("readme");
        assert!(readme.contains(std::env::consts::OS), "{readme}");
        assert!(readme.contains("passphrase"), "{readme}");

        // Made again: the keys on the drive are kept.
        let keys = root.join("home").join("keystore");
        std::fs::write(&keys, b"sealed").expect("keys");
        make(drive.path()).expect("refreshed");
        assert_eq!(std::fs::read(&keys).expect("keys"), b"sealed");
    }

    #[test]
    fn a_portable_copy_needs_a_full_path() {
        assert!(make(Path::new("relative")).is_err());
    }
}
