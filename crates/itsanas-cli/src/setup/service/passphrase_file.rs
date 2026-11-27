//! The passphrase file: the one file, readable by this account alone, that
//! the service reads at start.

use std::path::Path;

use super::super::secrets::Secret;
use super::io;
#[cfg(not(unix))]
use super::{run_powershell, texts::windows_acl_script};
use crate::error::{CliError, Result};

// ---------------------------------------------------------------------------
// The passphrase file
// ---------------------------------------------------------------------------

/// The variable the Unix files carry, which systemd and the plist's `sh` read.
const ENV_PREFIX: &str = "ITSANAS_PASSPHRASE=";

/// Refuse a passphrase the service file would hand back changed.
///
/// systemd's `EnvironmentFile=` strips spaces at the ends and interprets
/// quotes and backslashes; a line break ends the value. `provision.sh` writes
/// the value raw and so has the same limit without saying so -- here it is
/// said before anything is written, rather than found as a daemon that will
/// not unlock.
/// Whether `launchctl print-disabled` lists `label` as disabled: `"label" =>
/// disabled`, or `=> true` before macOS 13. A label it does not list is enabled.
pub(crate) fn launchd_disabled(listing: &str, label: &str) -> bool {
    let key = format!("\"{label}\"");
    listing.lines().map(str::trim).any(|line| {
        line.starts_with(&key) && (line.ends_with("disabled") || line.ends_with("true"))
    })
}

pub(crate) fn fits_service_file(os: &str, passphrase: &str) -> Result<()> {
    if passphrase.contains(['\n', '\r']) {
        return Err(CliError::Usage(
            "the passphrase has a line break, which no service file can carry".to_owned(),
        ));
    }
    if os != "windows"
        && (passphrase.contains(['"', '\'', '\\']) || passphrase.trim() != passphrase)
    {
        return Err(CliError::Usage(
            "the passphrase has a quote, a backslash or a space at one end, which the \
             service's environment file would change; `itsanas passphrase` sets another"
                .to_owned(),
        ));
    }
    Ok(())
}

/// What a passphrase file holds, in either form: `ITSANAS_PASSPHRASE=value`
/// (Linux, macOS) or the bare value (Windows, with or without a UTF-8 mark).
pub(crate) fn parse_passphrase_file(text: &str) -> Option<Secret> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(value) = text
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(ENV_PREFIX))
    {
        return (!value.is_empty()).then(|| Secret::new(value.to_owned()));
    }
    // linux.sh's placeholder: "# ITSANAS_PASSPHRASE=your-passphrase-here".
    // Recognised by the commented key, not by a leading '#' alone: on Windows
    // the file is the bare passphrase, and "#Horse-Battery-9" is a passphrase.
    let commented_key = text
        .trim_start()
        .strip_prefix('#')
        .is_some_and(|rest| rest.trim_start().starts_with(ENV_PREFIX));
    if commented_key || text.is_empty() {
        return None;
    }
    Some(Secret::new(text.to_owned()))
}

/// The passphrase in `path`, if there is a usable one.
pub(crate) fn read_passphrase_file(path: &Path) -> Option<Secret> {
    let bytes = zeroize::Zeroizing::new(std::fs::read(path).ok()?);
    let text = zeroize::Zeroizing::new(String::from_utf8(bytes.to_vec()).ok()?);
    parse_passphrase_file(&text)
}

/// Write the passphrase file as the installers do: beside the old one, locked
/// down **before** the secret goes in, then moved into place, then read back.
///
/// The order is `provision.ps1`'s and was paid for: truncating the file in
/// place destroyed the passphrase on a machine when the permission step failed
/// in between. Here the worst case is a stray `.new` beside an untouched file.
pub(crate) fn write_passphrase_file(path: &Path, passphrase: &str) -> Result<()> {
    let os = std::env::consts::OS;
    fits_service_file(os, passphrase)?;
    let dir = path
        .parent()
        .ok_or_else(|| CliError::Usage(format!("{} has no directory", path.display())))?;
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let pending = path.with_extension("new");
    let _ = std::fs::remove_file(&pending);
    let content = if os == "windows" {
        // Windows PowerShell reads a file without a mark in the ANSI code
        // page, which would hand the daemon a different passphrase than the
        // one typed. The mark makes `Get-Content -Raw` read UTF-8.
        if passphrase.is_ascii() {
            Secret::new(passphrase.to_owned())
        } else {
            Secret::new(format!("\u{feff}{passphrase}"))
        }
    } else {
        Secret::new(format!("{ENV_PREFIX}{passphrase}\n"))
    };
    create_locked_down(&pending)?;
    let written = std::fs::write(&pending, content.as_bytes())
        .map_err(io(&pending))
        .and_then(|()| std::fs::rename(&pending, path).map_err(io(path)));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&pending);
        return Err(CliError::Usage(format!(
            "could not put the passphrase in {} ({error}); the file already there, if any, was \
             left as it was. If an administrator created it, remove it as administrator and run \
             this again",
            path.display()
        )));
    }
    // Read back what is there: the one file whose protection must never be
    // asserted without looking (provision.ps1).
    match read_passphrase_file(path) {
        Some(stored) if *stored == passphrase => Ok(()),
        _ => Err(CliError::Usage(format!(
            "{} does not hold the passphrase just written; the background service would not \
             start. Remove it and run this again",
            path.display()
        ))),
    }
}

/// An empty file only this account can read, made before the secret exists.
fn create_locked_down(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        if let Some(dir) = path.parent() {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(io(path))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, b"").map_err(io(path))?;
        if let Err(why) = run_powershell(&windows_acl_script(path)) {
            let _ = std::fs::remove_file(path);
            return Err(CliError::Usage(format!(
                "could not make {} readable by this account alone: {why}",
                path.display()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::super::ps_quote;
    use super::*;

    /// `launchctl print-disabled` as macOS 13 and later print it, and the older
    /// `=> true`; a label only sharing a prefix is another agent.
    #[test]
    fn a_disabled_launchd_agent_is_read_as_disabled_and_no_other() {
        let listing = "disabled services = {\n\t\"fr.ngas.itsanas\" => disabled\n\t\"fr.ngas.itsanas-work\" => enabled\n}\n";
        assert!(
            launchd_disabled(listing, "fr.ngas.itsanas"),
            "a disabled agent read as starting at login"
        );
        assert!(
            !launchd_disabled(listing, "fr.ngas.itsanas-work"),
            "an enabled agent read as disabled"
        );
        assert!(
            !launchd_disabled(listing, "fr.ngas.itsanas-home"),
            "an agent nobody disabled read as disabled"
        );
        assert!(
            launchd_disabled("\t\"x\" => true\n", "x"),
            "the pre-13 wording was missed"
        );
    }

    #[test]
    fn the_passphrase_file_reads_back_in_both_forms() {
        assert_eq!(
            parse_passphrase_file("ITSANAS_PASSPHRASE=open sesame\n")
                .as_deref()
                .map(String::as_str),
            Some("open sesame"),
            "the Linux form was misread, and the service would not unlock"
        );
        assert_eq!(
            parse_passphrase_file("\u{feff}\u{e9}t\u{e9}")
                .as_deref()
                .map(String::as_str),
            Some("\u{e9}t\u{e9}"),
            "the Windows form with its UTF-8 mark was misread"
        );
        assert!(
            parse_passphrase_file("# ITSANAS_PASSPHRASE=your-passphrase-here\n").is_none(),
            "linux.sh's commented placeholder was taken for a passphrase"
        );
        assert!(
            fits_service_file("linux", "ends with a space ").is_err()
                && fits_service_file("linux", "back\\slash").is_err()
                && fits_service_file("windows", "back\\slash").is_ok(),
            "a passphrase systemd would change was accepted, or Windows refused one it keeps"
        );
    }

    #[test]
    fn red_team_a_windows_passphrase_starting_with_a_hash_reads_back() {
        assert_eq!(
            parse_passphrase_file("#Horse-Battery-9")
                .as_deref()
                .map(String::as_str),
            Some("#Horse-Battery-9"),
            "a bare Windows passphrase starting with # was taken for linux.sh's placeholder: setup stops at the Secret step forever"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("passphrase-hash.txt");
        write_passphrase_file(&path, "#Horse-Battery-9")
            .expect("a passphrase starting with # is written and read back");
        assert_eq!(
            read_passphrase_file(&path).as_deref().map(String::as_str),
            Some("#Horse-Battery-9")
        );
    }

    #[test]
    fn the_passphrase_file_is_written_whole_and_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state").join("passphrase-test.txt");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, "the old one").expect("old");
        write_passphrase_file(&path, "the new one").expect("write");
        assert_eq!(
            read_passphrase_file(&path).as_deref().map(String::as_str),
            Some("the new one"),
            "the service would start with the old passphrase and not unlock"
        );
        assert!(
            !path.with_extension("new").exists(),
            "a copy of the passphrase was left beside the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "other users of this machine can read the passphrase"
            );
        }
        #[cfg(windows)]
        {
            let count = run_powershell(&format!(
                "([System.IO.FileInfo]{}).GetAccessControl().GetAccessRules($true, $true, \
                 [System.Security.Principal.NTAccount]).Count",
                ps_quote(&path.display().to_string())
            ))
            .expect("read the access list");
            assert_eq!(
                count.trim(),
                "1",
                "the passphrase file is open to more than this account: other users of this \
                 machine could unlock the keys"
            );
        }
    }
}
