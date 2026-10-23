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
//!
//! [`candidates`] lists the drives a person would plug in to copy an account
//! to (the Settings page's "Copy this account to a USB key"): what each
//! system says is removable or external, never a guess from a name. The
//! system's answer is parsed by pure functions ([`parse_windows`],
//! [`parse_proc_mounts`], [`parse_mac_mount`]), so the rules are tested on
//! any machine with samples of each system's output.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

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

// ---------------------------------------------------------------------------
// The drives a person may copy an account to
// ---------------------------------------------------------------------------

/// A drive that looks like a USB key or an external disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Where it is mounted: `E:\`, `/Volumes/KEY`, `/media/camille/KEY`.
    pub path: PathBuf,
    /// What the person calls it: its label, else where it is.
    pub label: String,
    /// Its size in bytes, when the system says.
    pub total: Option<u64>,
    /// Its free room in bytes, when the system says.
    pub free: Option<u64>,
    pub format: Option<Format>,
    /// A fixed disk on a USB bus (an external hard disk on Windows), not a
    /// key the system calls removable: shown as "external disk?".
    pub external: bool,
}

/// The drives plugged into this machine that look like a USB key or an
/// external disk, in the order the system lists them. Empty when the system
/// would not say: the page then offers "Another folder…" alone.
#[must_use]
pub fn candidates() -> Vec<Candidate> {
    let mut found = if cfg!(windows) {
        run_text(Command::new("powershell.exe").args(["-NoProfile", "-Command", WINDOWS_VOLUMES]))
            .map(|text| parse_windows(&text))
            .unwrap_or_default()
    } else if cfg!(target_os = "macos") {
        run_text(&mut Command::new("/sbin/mount"))
            .map(|text| parse_mac_mount(&text))
            .unwrap_or_default()
    } else {
        let user = std::env::var("USER")
            .ok()
            .filter(|user| !user.is_empty())
            .or_else(|| {
                crate::config::user_home()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
        std::fs::read_to_string("/proc/mounts")
            .map(|text| parse_proc_mounts(&text, user.as_deref().unwrap_or("")))
            .unwrap_or_default()
    };
    for candidate in &mut found {
        // The system's own count wins over what the listing said, when it
        // answers: the listing may be a few seconds old.
        if let Some(free) = free_bytes(&candidate.path) {
            candidate.free = Some(free);
        }
        if candidate.total.is_none() {
            candidate.total = fs4::total_space(&candidate.path).ok();
        }
        if candidate.format.is_none() {
            candidate.format = format_of(&candidate.path);
        }
    }
    found
}

fn run_text(command: &mut Command) -> Option<String> {
    let output = command
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One line per lettered volume, `letter|type|bus|filesystem|size|free|label`,
/// after a first `system|C` line; the label last, so a `|` in it stays in it.
/// The bus is asked of the partition's disk, so an external hard disk (which
/// Windows calls Fixed) is told from the internal one by `USB`.
const WINDOWS_VOLUMES: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
     'system|' + $env:SystemDrive.TrimEnd(':'); \
     Get-Volume | Where-Object { $_.DriveLetter } | ForEach-Object { \
       $bus = ''; \
       try { $bus = (Get-Partition -DriveLetter $_.DriveLetter -ErrorAction Stop | \
         Get-Disk -ErrorAction Stop).BusType } catch {}; \
       [string]::Join('|', @($_.DriveLetter, $_.DriveType, $bus, $_.FileSystem, $_.Size, \
         $_.SizeRemaining, $_.FileSystemLabel)) }";

/// [`WINDOWS_VOLUMES`]' output: the volumes Windows calls Removable, and the
/// Fixed ones on a USB bus (marked external), never the system's own drive.
#[must_use]
pub fn parse_windows(text: &str) -> Vec<Candidate> {
    let mut system = String::from("C");
    let mut found = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(letter) = line.strip_prefix("system|") {
            letter.trim().clone_into(&mut system);
            continue;
        }
        let fields: Vec<&str> = line.splitn(7, '|').collect();
        let [letter, kind, bus, filesystem, size, free, label] = fields[..] else {
            continue;
        };
        let letter = letter.trim().to_ascii_uppercase();
        if letter.len() != 1
            || !letter.chars().all(|c| c.is_ascii_alphabetic())
            || letter.eq_ignore_ascii_case(&system)
        {
            continue;
        }
        let external = match kind.trim() {
            "Removable" => false,
            "Fixed" if bus.trim().eq_ignore_ascii_case("USB") => true,
            _ => continue,
        };
        let label = label.trim();
        found.push(Candidate {
            path: PathBuf::from(format!("{letter}:\\")),
            label: if label.is_empty() {
                format!("{letter}:")
            } else {
                format!("{label} ({letter}:)")
            },
            total: size.trim().parse().ok().filter(|size| *size > 0),
            free: free.trim().parse().ok(),
            format: parse_name(filesystem),
            external,
        });
    }
    found
}

/// The file systems of a disk plugged in here: an allow-list, because what
/// else is mounted under `/mnt` is as often somebody else's machine (NFS,
/// SMB, SSHFS, 9p, `WebDAV`, any `fuse.*`), where the account's master must
/// never land, as it is the system's own (`tmpfs`, `overlay`...).
/// `fuseblk` is ntfs-3g or exfat-fuse on a local device.
const LOCAL_DISK: [&str; 13] = [
    "ext2", "ext3", "ext4", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "btrfs", "xfs", "f2fs",
    "hfsplus", "apfs",
];

/// `/proc/mounts`: what is mounted under `/media/<user>/` or
/// `/run/media/<user>/` (where a desktop mounts a key it is given) or under
/// `/mnt/`, writable, and on a local disk ([`LOCAL_DISK`]).
#[must_use]
pub fn parse_proc_mounts(text: &str, user: &str) -> Vec<Candidate> {
    let mut places = vec![PathBuf::from("/mnt")];
    if !user.is_empty() && !user.contains('/') {
        places.push(Path::new("/media").join(user));
        places.push(Path::new("/run/media").join(user));
    }
    let mut found: Vec<Candidate> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [_, point, filesystem, options, ..] = fields[..] else {
            continue;
        };
        let point = PathBuf::from(unescape_mount(point));
        let read_only = options.split(',').any(|option| option == "ro");
        if read_only
            || !LOCAL_DISK.contains(&filesystem)
            || !places
                .iter()
                .any(|place| point.starts_with(place) && point != *place)
            || found.iter().any(|seen| seen.path == point)
        {
            continue;
        }
        found.push(unix_candidate(point, filesystem));
    }
    found
}

/// The macOS `mount` output: what is mounted under `/Volumes/`, writable and
/// shown in the Finder. The system volume is mounted at `/`, and
/// `/Volumes/Macintosh HD` is only a link to it, so it is never listed; a
/// disk image is read-only, and the system's own snapshots say `nobrowse`.
#[must_use]
pub fn parse_mac_mount(text: &str) -> Vec<Candidate> {
    let mut found = Vec::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(" on ") else {
            continue;
        };
        let Some((point, options)) = rest.rsplit_once(" (") else {
            continue;
        };
        let options: Vec<&str> = options
            .trim_end_matches(')')
            .split(',')
            .map(str::trim)
            .collect();
        let point = PathBuf::from(point);
        if !point.starts_with("/Volumes")
            || point == Path::new("/Volumes")
            || options
                .iter()
                .any(|option| matches!(*option, "read-only" | "nobrowse"))
        {
            continue;
        }
        let filesystem = options.first().copied().unwrap_or("");
        found.push(unix_candidate(point, filesystem));
    }
    found
}

fn unix_candidate(point: PathBuf, filesystem: &str) -> Candidate {
    Candidate {
        label: point.file_name().map_or_else(
            || point.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        format: parse_name(filesystem),
        path: point,
        total: None,
        free: None,
        external: false,
    }
}

/// `/proc/mounts` writes a space as `\040`, a tab `\011`, a line break
/// `\012` and a backslash `\134`.
fn unescape_mount(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let code = bytes
            .get(at + 1..at + 4)
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match (bytes[at], code) {
            (b'\\', Some(code)) => {
                out.push(code);
                at += 4;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The name of a format, as the page shows it.
#[must_use]
pub fn format_name(format: &Format) -> String {
    match format {
        Format::Ntfs => "NTFS".to_owned(),
        Format::Fat32 => "FAT32".to_owned(),
        Format::ExFat => "exFAT".to_owned(),
        Format::Other(name) => name.clone(),
    }
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

    #[test]
    fn windows_lists_its_removable_keys_and_usb_disks_never_the_system_drive() {
        // What WINDOWS_VOLUMES prints on a laptop with a key, an external
        // disk, a second internal disk, a DVD drive and a recovery partition.
        let sample = "system|C\r\n\
            C|Fixed|NVMe|NTFS|510770802688|201326592000|Windows\r\n\
            D|Fixed|SATA|NTFS|1000202039296|800000000000|Data\r\n\
            E|Removable|USB|FAT32|31004295168|30000000000|KINGSTON\r\n\
            F|Fixed|USB|exFAT|2000398934016|1500000000000|My Passport|2\r\n\
            G|CD-ROM|SATA||0|0|\r\n\
            H|Removable|USB|exFAT|62000000000|61000000000|\r\n\
            garbage line\r\n";
        let found = parse_windows(sample);
        let paths: Vec<String> = found.iter().map(|c| c.path.display().to_string()).collect();
        assert_eq!(paths, ["E:\\", "F:\\", "H:\\"], "{found:#?}");
        assert_eq!(found[0].label, "KINGSTON (E:)");
        assert_eq!(found[0].format, Some(Format::Fat32));
        assert_eq!(found[0].total, Some(31_004_295_168));
        assert_eq!(found[0].free, Some(30_000_000_000));
        assert!(!found[0].external);
        assert_eq!(
            found[1].label, "My Passport|2 (F:)",
            "a | in a label is the label's"
        );
        assert!(found[1].external, "a Fixed disk on USB is an external disk");
        assert_eq!(
            found[2].label, "H:",
            "an unnamed key is named by its letter"
        );

        let elsewhere = "system|D\nC|Removable|USB|NTFS|8|4|OLD\nD|Removable|USB|NTFS|8|4|SYS\n";
        let found = parse_windows(elsewhere);
        assert_eq!(found.len(), 1, "Windows on D: is never offered: {found:#?}");
        assert_eq!(found[0].format, Some(Format::Ntfs));
        assert_eq!(parse_windows(""), []);
    }

    #[test]
    fn linux_lists_what_the_desktop_mounted_for_the_person_and_under_mnt() {
        // A /proc/mounts with a key the desktop mounted (a space in its
        // label), one under /run/media (Fedora), a disk under /mnt, another
        // person's key, a read-only DVD, snaps and the system's own.
        let sample = "\
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
tmpfs /run tmpfs rw,nosuid,nodev,size=1621284k,mode=755 0 0
/dev/loop3 /snap/firefox/4848 squashfs ro,nodev,relatime 0 0
/dev/sdb1 /media/camille/MY\\040KEY vfat rw,nosuid,nodev,relatime,uid=1000,gid=1000,fmask=0022,dmask=0022,codepage=437,iocharset=iso8859-1,shortname=mixed,showexec,utf8,flush,errors=remount-ro 0 0
/dev/sdc1 /run/media/camille/SAMSUNG exfat rw,nosuid,nodev,relatime,uid=1000,gid=1000 0 0
/dev/sdd1 /mnt/backup ntfs3 rw,relatime,uid=0,gid=0,iocharset=utf8 0 0
/dev/sde1 /media/alex/THEIRS vfat rw,nosuid,nodev,relatime 0 0
/dev/sr0 /media/camille/DVD iso9660 ro,nosuid,nodev,relatime 0 0
tmpfs /mnt/ram tmpfs rw,relatime 0 0
/dev/sdf1 /mnt ext4 rw,relatime 0 0
/dev/sdb1 /media/camille/MY\\040KEY vfat rw 0 0
";
        let found = parse_proc_mounts(sample, "camille");
        let paths: Vec<String> = found.iter().map(|c| c.path.display().to_string()).collect();
        assert_eq!(
            paths,
            [
                "/media/camille/MY KEY",
                "/run/media/camille/SAMSUNG",
                "/mnt/backup"
            ],
            "{found:#?}"
        );
        assert_eq!(found[0].label, "MY KEY");
        assert_eq!(found[0].format, Some(Format::Fat32));
        assert_eq!(found[1].format, Some(Format::ExFat));
        assert_eq!(found[2].format, Some(Format::Ntfs));
        assert!(
            parse_proc_mounts(sample, "")
                .iter()
                .all(|c| c.path.starts_with("/mnt")),
            "with no user known, only /mnt"
        );
        assert!(
            parse_proc_mounts(sample, "../alex")
                .iter()
                .all(|c| !c.path.starts_with("/media")),
            "a user name with a slash is no user name"
        );
        assert_eq!(unescape_mount("a\\040b\\134c\\011d"), "a b\\c\td");
        assert_eq!(unescape_mount("trailing\\04"), "trailing\\04");
    }

    /// A NETWORK MOUNT IS NO KEY: an NFS, SMB, SSHFS or 9p share under
    /// /mnt is somebody else's machine, and the account's master would land
    /// on it. Only the file systems of a disk plugged in here are listed.
    #[test]
    fn red_team_a_network_share_under_mnt_is_never_listed_as_a_key() {
        assert_eq!(parse_proc_mounts("nas:/x /mnt/nas nfs4 rw 0 0", ""), []);
        for filesystem in [
            "nfs",
            "cifs",
            "smb3",
            "fuse.sshfs",
            "fuse.rclone",
            "9p",
            "davfs",
            "ceph",
        ] {
            let line = format!("server:/share /mnt/share {filesystem} rw 0 0");
            assert!(
                parse_proc_mounts(&line, "camille").is_empty(),
                "{filesystem} was offered as a key"
            );
        }
        for filesystem in ["ext4", "vfat", "exfat", "ntfs3", "fuseblk", "btrfs", "xfs"] {
            let line = format!("/dev/sdb1 /mnt/key {filesystem} rw 0 0");
            assert_eq!(
                parse_proc_mounts(&line, "camille").len(),
                1,
                "{filesystem}, a disk plugged in, was not offered"
            );
        }
    }

    #[test]
    fn macos_lists_its_volumes_never_the_system_a_disk_image_or_a_snapshot() {
        let sample = "\
/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)
devfs on /dev (devfs, local, nobrowse)
/dev/disk3s6 on /System/Volumes/VM (apfs, local, noexec, journaled, noatime, nobrowse)
/dev/disk3s5 on /System/Volumes/Data (apfs, local, journaled, nobrowse, protect, root data)
map auto_home on /System/Volumes/Data/home (autofs, automounted, nobrowse)
/dev/disk4s1 on /Volumes/NO NAME (msdos, local, nodev, nosuid, noowners, noatime, fskit)
/dev/disk5s2 on /Volumes/Travel (exfat, local, nodev, nosuid, noowners, noatime, fskit)
/dev/disk6s1 on /Volumes/Firefox (hfs, local, nodev, nosuid, read-only, noowners, quarantine, mounted by camille)
/dev/disk7s1 on /Volumes/Archive (apfs, local, nodev, nosuid, journaled, noowners)
/dev/disk3s4 on /Volumes/com.apple.TimeMachine.localsnapshots (apfs, local, journaled, nobrowse)
";
        let found = parse_mac_mount(sample);
        let paths: Vec<String> = found.iter().map(|c| c.path.display().to_string()).collect();
        assert_eq!(
            paths,
            ["/Volumes/NO NAME", "/Volumes/Travel", "/Volumes/Archive"],
            "{found:#?}"
        );
        assert_eq!(found[0].label, "NO NAME");
        assert_eq!(found[0].format, Some(Format::Fat32));
        assert_eq!(found[1].format, Some(Format::ExFat));
        assert_eq!(found[2].format, Some(Format::Other("apfs".into())));
        assert_eq!(parse_mac_mount("nonsense\n"), []);
    }
}
