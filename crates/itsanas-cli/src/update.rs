//! `itsanas update`, and the daemon's daily look for a newer release
//! (HANDOVER §8 0w (6), 0t part 4).
//!
//! A release is trusted only through `itsanas-release`: the manifest's
//! signature against the key compiled into this build, then its format, then
//! a version newer than this one (no downgrade), then the downloaded binary's
//! size, BLAKE3 and SHA-256. Nothing here re-implements a step of that; this
//! module fetches the files, decides whether this build may replace itself at
//! all, and puts the new program in place.
//!
//! **Only a binary installed from a release updates itself.** One run from a
//! cargo `target/` directory, built for a platform no release covers, or whose
//! bytes are not the binary its own version's signed manifest lists (built
//! from source, then copied by an installer), reports and changes nothing: a
//! developer's or a source-installed build replaced behind their back by a
//! downloaded one is a surprise, never a fix.
//!
//! **The program is replaced by rename, never written over** (the #240 rule:
//! a running file written in place is a crash, or on Windows a refusal). The
//! new binary is downloaded beside the old one, so the rename stays on one
//! filesystem; the running file is renamed aside to `.old` -- Windows allows
//! renaming a running program, not replacing it -- then the new one is renamed
//! in, and if that second rename fails the first is undone.
//!
//! **HTTPS by `curl`.** The workspace has no HTTP client (rustls is there,
//! under QUIC, with no HTTP above it), and adding one for two small files a
//! day is a dependency tree in a security-sensitive binary for nothing: the
//! transport is not what is trusted -- the signature is. `curl` ships with
//! Windows 10 and later, macOS and every Linux this project targets, and
//! `install/get.sh` already relies on it.

use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_release::{Manifest, ReleaseError, Trust, Version};

use crate::{
    config::{Config, Updates},
    error::{CliError, Result},
    node::Node,
};

/// Where published releases are. `latest/` is GitHub's newest published,
/// non-draft release: a draft nobody signed is never offered.
const RELEASES: &str = "https://github.com/SigSegGit/itsanas/releases/";
const MANIFEST: &str = "manifest.txt";
const SIGNATURE: &str = "manifest.txt.sig";
/// A manifest is a few hundred bytes; a server sending more is not sending one.
const MANIFEST_MAX: u64 = 64 * 1024;
/// In a node's home: the newer version the daemon found, for `status`.
pub(crate) const NOTICE_FILE: &str = "update-available";
/// Where downloads go, beside the program so the final rename is on one disk.
const SCRATCH: &str = ".itsanas-update";

/// How the files are fetched. A trait so the tests serve a temporary directory
/// and never touch the network.
pub(crate) trait Fetch {
    /// Fetch `path` (relative to the releases URL) into `to`, refusing more
    /// than `max_bytes`.
    fn get(&self, path: &str, to: &Path, max_bytes: u64) -> std::result::Result<(), String>;
}

/// The real fetch: `curl`, HTTPS only, redirects (GitHub's `latest` is one)
/// HTTPS only too.
pub(crate) struct Curl;

impl Fetch for Curl {
    fn get(&self, path: &str, to: &Path, max_bytes: u64) -> std::result::Result<(), String> {
        let output = curl_command(path, to, max_bytes)
            .output()
            .map_err(|error| format!("could not run curl ({error}); is it installed?"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "could not download {path}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// The `curl` call a download makes, apart so a test can read it: HTTPS
/// only, redirects included (GitHub sends `latest/download` to its CDN, and a
/// redirect to plain HTTP would hand the file to anyone on the path); a cap
/// on the size; the URL always under this project's releases, with the path
/// passed as one argument and never through a shell.
pub(crate) fn curl_command(path: &str, to: &Path, max_bytes: u64) -> Command {
    let mut command = Command::new("curl");
    command
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "900",
            "--max-filesize",
            &max_bytes.to_string(),
            "--output",
        ])
        .arg(to)
        .arg(format!("{RELEASES}{}", path.trim_start_matches('/')));
    command
}

/// Read a downloaded file, refusing one larger than `max`: the cap `curl`
/// enforces is not the only way a file reaches that path.
fn read_capped(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(std::io::Error::other(format!("larger than {max} bytes")));
    }
    Ok(bytes)
}

/// The release target this build is, or `None` on a platform no release
/// covers (then it never updates itself).
pub(crate) const fn this_target() -> Option<&'static str> {
    if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "gnu"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    )) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else {
        None
    }
}

/// What a check found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// No release key compiled in: nothing can be trusted, so nothing is fetched.
    NoKey,
    /// This build never replaces itself; why.
    NotFromRelease(String),
    /// The newest signed release is not newer than this one.
    UpToDate(Version),
    /// A newer signed release.
    Available(Manifest),
}

/// One build's view of the releases: what it trusts, what it is, where it is.
pub(crate) struct Updater<'a> {
    pub(crate) fetch: &'a dyn Fetch,
    /// `None` when this build pins no key.
    pub(crate) trust: Option<Trust>,
    pub(crate) running: Version,
    pub(crate) target: Option<&'static str>,
    /// The running program, the file that gets replaced.
    pub(crate) exe: PathBuf,
}

fn refused(error: &ReleaseError) -> CliError {
    CliError::Usage(error.to_string())
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> CliError + '_ {
    move |source| CliError::Io {
        path: path.to_owned(),
        source,
    }
}

impl<'a> Updater<'a> {
    /// This program, fetching with `curl`, trusting the pinned key.
    pub(crate) fn of_this_build(fetch: &'a dyn Fetch) -> Result<Self> {
        Ok(Self {
            fetch,
            trust: Trust::pinned().ok(),
            running: Version::running(),
            target: this_target(),
            exe: std::env::current_exe().map_err(io_error(Path::new("<this program>")))?,
        })
    }

    fn scratch(&self) -> PathBuf {
        self.exe
            .parent()
            .map_or_else(|| PathBuf::from(SCRATCH), |dir| dir.join(SCRATCH))
    }

    /// Why this build must not replace itself, from what it can see without
    /// the network.
    fn local_refusal(&self) -> Option<String> {
        if self.target.is_none() {
            return Some(
                "no release is built for this platform, so this program never updates itself"
                    .to_owned(),
            );
        }
        // A cargo build directory: a developer's binary, never replaced by a
        // download.
        if self
            .exe
            .ancestors()
            .any(|dir| dir.file_name().is_some_and(|name| name == "target"))
        {
            return Some(format!(
                "{} is a build from source (it runs from a target/ directory), so it never \
                 updates itself; install a release, or rebuild",
                self.exe.display()
            ));
        }
        None
    }

    /// Fetch `dir`'s manifest and its signature, unchecked: the caller hands
    /// both to `itsanas-release`, which checks before it parses.
    fn fetch_manifest(&self, dir: &str) -> Result<(Vec<u8>, String)> {
        let scratch = self.scratch();
        std::fs::create_dir_all(&scratch).map_err(io_error(&scratch))?;
        let manifest_path = scratch.join(MANIFEST);
        let signature_path = scratch.join(SIGNATURE);
        let fetched = self
            .fetch
            .get(&format!("{dir}/{MANIFEST}"), &manifest_path, MANIFEST_MAX)
            .and_then(|()| {
                self.fetch
                    .get(&format!("{dir}/{SIGNATURE}"), &signature_path, MANIFEST_MAX)
            });
        let read = fetched.map_err(CliError::Usage).and_then(|()| {
            let bytes =
                read_capped(&manifest_path, MANIFEST_MAX).map_err(io_error(&manifest_path))?;
            let signature = String::from_utf8(
                read_capped(&signature_path, MANIFEST_MAX).map_err(io_error(&signature_path))?,
            )
            .map_err(|_| CliError::Usage("the release signature is not text".to_owned()))?;
            Ok((bytes, signature))
        });
        let _ = std::fs::remove_file(&manifest_path);
        let _ = std::fs::remove_file(&signature_path);
        let _ = std::fs::remove_dir(&scratch);
        read
    }

    /// Is there a newer signed release? Changes nothing on this machine.
    pub(crate) fn check(&self) -> Result<Found> {
        let Some(trust) = &self.trust else {
            return Ok(Found::NoKey);
        };
        if let Some(why) = self.local_refusal() {
            return Ok(Found::NotFromRelease(why));
        }
        let (bytes, signature) = self.fetch_manifest("latest/download")?;
        match itsanas_release::verify_release(&bytes, &signature, trust, self.running) {
            Ok(manifest) => Ok(Found::Available(manifest)),
            Err(ReleaseError::NotNewer { .. }) => Ok(Found::UpToDate(self.running)),
            Err(other) => Err(refused(&other)),
        }
    }

    /// The running program is the binary its own version's signed manifest
    /// lists for this target: proof it was installed from a release.
    fn installed_from_release(&self, trust: &Trust, target: &str) -> Result<()> {
        let not_ours = |why: String| {
            CliError::Usage(format!(
                "this program was not installed from a release ({why}), so it never updates \
                 itself; install a release (install/get.ps1 or get.sh) to get updates"
            ))
        };
        let (bytes, signature) = self
            .fetch_manifest(&format!("download/v{}", self.running))
            .map_err(|error| not_ours(format!("no release {}: {error}", self.running)))?;
        let own = itsanas_release::verify_signed(&bytes, &signature, trust)
            .map_err(|error| not_ours(format!("release {}: {error}", self.running)))?;
        let entry = own
            .file_for(target)
            .ok_or_else(|| not_ours(format!("release {} has no {target}", self.running)))?;
        entry
            .check_file(&self.exe)
            .map_err(|_| not_ours(format!("its bytes are not release {}'s", self.running)))
    }

    /// Download `manifest`'s binary for this target, check it, and put it in
    /// place of the running program. The running program is untouched unless
    /// every check passed.
    pub(crate) fn install(&self, manifest: &Manifest) -> Result<Version> {
        let Some(trust) = &self.trust else {
            return Err(refused(&ReleaseError::NoKeyPinned));
        };
        if let Some(why) = self.local_refusal() {
            return Err(CliError::Usage(why));
        }
        let target = self.target.unwrap_or_default();
        if manifest.version <= self.running {
            return Err(refused(&ReleaseError::NotNewer {
                offered: manifest.version,
                running: self.running,
            }));
        }
        self.installed_from_release(trust, target)?;
        let entry = manifest.file_for(target).ok_or_else(|| {
            CliError::Usage(format!(
                "release {} has no binary for {target}: nothing to install",
                manifest.version
            ))
        })?;
        // The name goes into a URL path: a signed manifest is Nicolas's, but a
        // name that could climb out of the release is refused anyway.
        if entry.name.is_empty()
            || !entry
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            || entry.name.starts_with('.')
        {
            return Err(refused(&ReleaseError::Damaged(format!(
                "binary name {:?}",
                entry.name
            ))));
        }
        let scratch = self.scratch();
        std::fs::create_dir_all(&scratch).map_err(io_error(&scratch))?;
        let download = scratch.join("itsanas.new");
        let _ = std::fs::remove_file(&download);
        let checked = self
            .fetch
            .get(
                &format!("download/v{}/{}", manifest.version, entry.name),
                &download,
                entry.size.saturating_add(1),
            )
            .map_err(CliError::Usage)
            .and_then(|()| entry.check_file(&download).map_err(|e| refused(&e)))
            .and_then(|()| make_executable(&download))
            .and_then(|()| {
                replace(&self.exe, &download, &mut |from, to| {
                    std::fs::rename(from, to)
                })
            });
        // Whatever happened, no half download stays behind to be mistaken
        // for a program.
        let _ = std::fs::remove_file(&download);
        let _ = std::fs::remove_dir(&scratch);
        checked.map(|()| manifest.version)
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io_error(path))
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as the Unix one"
)]
const fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Where the replaced program goes: `itsanas.exe.old`.
pub(crate) fn aside(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Remove the program a previous update set aside. On Windows it could not be
/// deleted while it ran; by the next start it no longer does.
pub(crate) fn forget_the_old_program(exe: &Path) {
    let _ = std::fs::remove_file(aside(exe));
}

/// Put `new` in place of `exe` by two renames, undoing the first if the
/// second fails. `rename` is a parameter so a test can make either one fail.
pub(crate) fn replace(
    exe: &Path,
    new: &Path,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<()> {
    let old = aside(exe);
    let _ = std::fs::remove_file(&old);
    rename(exe, &old).map_err(|error| {
        CliError::Usage(format!(
            "could not move {} aside ({error}); nothing was changed",
            exe.display()
        ))
    })?;
    if let Err(error) = rename(new, exe) {
        return Err(match rename(&old, exe) {
            Ok(()) => CliError::Usage(format!(
                "could not put the new program in place ({error}); the old one is back"
            )),
            Err(again) => CliError::Usage(format!(
                "could not put the new program in place ({error}) nor the old one back \
                 ({again}): rename {} to {} by hand",
                old.display(),
                exe.display()
            )),
        });
    }
    // Elsewhere the running process keeps its file open by inode; on Windows
    // it is still running and cannot be deleted until it exits.
    if !cfg!(windows) {
        let _ = std::fs::remove_file(&old);
    }
    Ok(())
}

/// The line `status` shows when the daemon found a newer release.
pub(crate) fn notice(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(NOTICE_FILE)).ok()?;
    let version = Version::parse(text.trim()).ok()?;
    (version > Version::running())
        .then(|| format!("update available: {version} (`itsanas update` installs it)"))
}

/// What one daily look did, for the daemon's log and for the tests.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Daily {
    Off,
    Nothing(String),
    Notified(Version),
    Installed(Version),
}

/// One daily look, under `setting`: `notify` writes the notice `status`
/// shows; `auto` installs. The caller restarts.
pub(crate) fn daily(home: &Path, updater: &Updater<'_>, setting: Updates) -> Result<Daily> {
    let notice_file = home.join(NOTICE_FILE);
    if setting == Updates::Off {
        let _ = std::fs::remove_file(&notice_file);
        return Ok(Daily::Off);
    }
    match updater.check()? {
        Found::NoKey => Ok(Daily::Nothing(ReleaseError::NoKeyPinned.to_string())),
        Found::NotFromRelease(why) => Ok(Daily::Nothing(why)),
        Found::UpToDate(version) => {
            let _ = std::fs::remove_file(&notice_file);
            Ok(Daily::Nothing(format!("{version} is the newest release")))
        }
        Found::Available(manifest) if setting == Updates::Auto => {
            let version = updater.install(&manifest)?;
            let _ = std::fs::remove_file(&notice_file);
            Ok(Daily::Installed(version))
        }
        Found::Available(manifest) => {
            std::fs::write(&notice_file, format!("{}\n", manifest.version))
                .map_err(io_error(&notice_file))?;
            Ok(Daily::Notified(manifest.version))
        }
    }
}

/// Sleep `wait`, waking to see `shutdown`. False when it was asked to stop.
fn sleep_unless(shutdown: &AtomicBool, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1).min(until - Instant::now()));
    }
    !shutdown.load(Ordering::SeqCst)
}

/// A random duration in `[base, base + spread)`. Jittered so a network of
/// nodes started by the same release never asks GitHub in the same second.
fn jittered(base: Duration, spread: Duration) -> Duration {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    let spread = spread.as_secs().max(1);
    base + Duration::from_secs(u64::from_le_bytes(bytes) % spread)
}

/// The daemon's thread: a first look 5 to 65 minutes after start (not at once,
/// so a daemon that restarts in a loop does not hammer GitHub), then once a
/// day, give or take an hour. Sets `updated` and `shutdown` when it installed
/// a new program: the daemon then exits with a failure code, which every
/// service this project installs answers by starting the program again --
/// now the new one. Stopping the service from inside it would kill this very
/// process half way.
pub(crate) fn watch(home: &Path, shutdown: &AtomicBool, updated: &AtomicBool) {
    if let Ok(exe) = std::env::current_exe() {
        forget_the_old_program(&exe);
    }
    let mut wait = jittered(Duration::from_secs(5 * 60), Duration::from_secs(60 * 60));
    while sleep_unless(shutdown, wait) {
        wait = jittered(
            Duration::from_secs(23 * 3600),
            Duration::from_secs(2 * 3600),
        );
        let setting = Config::load(&Node::config_path(home))
            .map(|config| config.updates)
            .unwrap_or_default();
        let curl = Curl;
        let outcome =
            Updater::of_this_build(&curl).and_then(|updater| daily(home, &updater, setting));
        match outcome {
            Ok(Daily::Off | Daily::Nothing(_)) => {}
            Ok(Daily::Notified(version)) => println!(
                "itsanas: update available: {version} (`itsanas update` installs it, or set \
                 updates = auto)"
            ),
            Ok(Daily::Installed(version)) => {
                println!("itsanas: updated to {version}; restarting into it");
                updated.store(true, Ordering::SeqCst);
                shutdown.store(true, Ordering::SeqCst);
                return;
            }
            Err(error) => eprintln!("itsanas: the daily update check failed: {error}"),
        }
    }
}

/// `itsanas update [--check]`.
pub(crate) fn command(home: &Path, instance: Option<&str>, check_only: bool) -> Result<()> {
    let curl = Curl;
    let updater = Updater::of_this_build(&curl)?;
    forget_the_old_program(&updater.exe);
    let manifest = match updater.check()? {
        Found::NoKey => {
            println!("{}", ReleaseError::NoKeyPinned);
            return Ok(());
        }
        Found::NotFromRelease(why) => {
            println!("{why}");
            return Ok(());
        }
        Found::UpToDate(version) => {
            println!("{version} is up to date");
            return Ok(());
        }
        Found::Available(manifest) => manifest,
    };
    if check_only {
        println!(
            "update available: {} (this is {}); `itsanas update` installs it",
            manifest.version, updater.running
        );
        return Ok(());
    }
    let version = updater.install(&manifest)?;
    let _ = std::fs::remove_file(home.join(NOTICE_FILE));
    println!("installed {version} in place of {}", updater.running);
    let service = crate::setup::service::Platform::of_this_machine(home, instance);
    if crate::setup::ServiceControl::installed(&service) {
        let _ = crate::setup::ServiceControl::stop(&service);
        crate::setup::ServiceControl::start(&service)?;
        println!("the background service was restarted on {version}");
    } else {
        println!("no background service here: start ITSaNAS again to run {version}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
