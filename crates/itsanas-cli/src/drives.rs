//! What kind of drive a vault, a backup or a portable copy is about to go
//! on, and what that will cost the person later.
//!
//! The common case is a USB key or an old external disk, formatted by
//! whichever machine saw it first:
//!
//! - **NTFS** (Windows): a Mac reads it and cannot write to it, so a portable
//!   copy or a vault on it stops working the day it is plugged into a Mac.
//! - **FAT32** (most keys up to 32 GB, as sold): no file may exceed 4 GiB, and
//!   the vault's index or a backup's segment file can outgrow that.
//! - **exFAT**: read and written by Windows, macOS and Linux, no such limit.
//!
//! Asked of the system rather than guessed, through the tool each one ships:
//! `Get-Volume` on Windows, `diskutil` on macOS, `stat -f` elsewhere. A drive
//! the tool cannot name gives no warning: a wrong warning teaches people to
//! skip them.

use std::{path::Path, process::Command};

/// The formats that change what a person can do with a drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Ntfs,
    Fat32,
    ExFat,
    /// Named, and none of the above (APFS, ext4, ...): nothing to say.
    Other(String),
}

/// The format of the drive holding `path` (or its nearest existing parent),
/// `None` when the system would not say.
pub fn format_of(path: &Path) -> Option<Format> {
    let dir = existing_ancestor(path)?;
    let output = if cfg!(windows) {
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-Volume -FilePath $env:ITSANAS_PATH).FileSystem",
            ])
            .env("ITSANAS_PATH", dir)
            .output()
    } else if cfg!(target_os = "macos") {
        Command::new("diskutil").arg("info").arg(dir).output()
    } else {
        Command::new("stat")
            .args(["-f", "-c", "%T"])
            .arg(dir)
            .output()
    }
    .ok()
    .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    if cfg!(target_os = "macos") {
        parse_diskutil(&text)
    } else {
        parse_name(&text)
    }
}

/// `path` itself or the nearest parent that exists: a target folder is
/// usually not made yet.
pub fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|dir| dir.exists())
}

/// Bytes free on the drive that will hold `path`.
pub fn free_bytes(path: &Path) -> Option<u64> {
    fs4::available_space(existing_ancestor(path)?).ok()
}

/// A file-system name as `Get-Volume` or `stat -f -c %T` print it.
fn parse_name(text: &str) -> Option<Format> {
    let name = text.trim().to_ascii_lowercase();
    Some(match name.as_str() {
        "ntfs" | "ntfs3" => Format::Ntfs,
        "fat32" | "msdos" | "vfat" => Format::Fat32,
        "exfat" => Format::ExFat,
        // ntfs-3g and exfat-fuse both mount as `fuseblk`: not enough to warn.
        "" | "fuseblk" => return None,
        _ => Format::Other(name),
    })
}

/// The "File System Personality" line of `diskutil info`.
fn parse_diskutil(text: &str) -> Option<Format> {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("File System Personality:"))?;
    let name = line.split_once(':')?.1.trim().to_ascii_lowercase();
    Some(if name.contains("exfat") {
        Format::ExFat
    } else if name.contains("ntfs") {
        Format::Ntfs
    } else if name.contains("fat32") || name.contains("ms-dos") {
        Format::Fat32
    } else {
        Format::Other(name)
    })
}

/// What to tell the person about putting `what` on this format; empty when
/// there is nothing to say.
#[must_use]
pub fn warning(format: Option<&Format>, what: &str) -> Option<String> {
    match format? {
        Format::Ntfs => Some(format!(
            "this drive is NTFS: a Mac can read {what} on it but not write to it. For a \
             drive used on Windows and Mac, reformat it as exFAT (after copying off what it \
             holds)"
        )),
        Format::Fat32 => Some(format!(
            "this drive is FAT32: no file on it may exceed 4 GiB, and {what} can grow past \
             that. exFAT has no such limit and works on Windows, Mac and Linux"
        )),
        Format::ExFat | Format::Other(_) => None,
    }
}

/// The warning for `path`, asked of the system.
pub fn warning_for(path: &Path, what: &str) -> Option<String> {
    warning(format_of(path).as_ref(), what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_formats_are_read_from_what_each_system_prints() {
        assert_eq!(parse_name("NTFS\r\n"), Some(Format::Ntfs));
        assert_eq!(parse_name("FAT32"), Some(Format::Fat32));
        assert_eq!(parse_name("exFAT"), Some(Format::ExFat));
        assert_eq!(parse_name("msdos\n"), Some(Format::Fat32));
        assert_eq!(
            parse_name("ext2/ext3\n"),
            Some(Format::Other("ext2/ext3".into()))
        );
        assert_eq!(
            parse_name("fuseblk"),
            None,
            "ntfs-3g or exfat-fuse: unknown"
        );
        assert_eq!(parse_name(""), None);

        let diskutil = "   Device Node:  /dev/disk4s1\n   File System Personality:  ExFAT\n";
        assert_eq!(parse_diskutil(diskutil), Some(Format::ExFat));
        let fat = "   File System Personality:  MS-DOS FAT32\n";
        assert_eq!(parse_diskutil(fat), Some(Format::Fat32));
        let ntfs = "   File System Personality:  NTFS\n";
        assert_eq!(parse_diskutil(ntfs), Some(Format::Ntfs));
        assert_eq!(parse_diskutil("nothing useful"), None);
    }

    #[test]
    fn only_formats_that_limit_the_person_are_warned_about() {
        assert!(warning(Some(&Format::Ntfs), "a vault").is_some_and(|w| w.contains("Mac")));
        assert!(warning(Some(&Format::Fat32), "a vault").is_some_and(|w| w.contains("4 GiB")));
        assert_eq!(warning(Some(&Format::ExFat), "a vault"), None);
        assert_eq!(
            warning(Some(&Format::Other("apfs".into())), "a vault"),
            None
        );
        assert_eq!(
            warning(None, "a vault"),
            None,
            "an unnamed drive gets no guess"
        );
    }
}
