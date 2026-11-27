//! The background service and the tray, installed the way the installers do.
//!
//! Absorbed rather than called (the question HANDOVER §8 0w (2) left open):
//! `provision.ps1` and `provision.sh` are scripts in a checkout, and a tester
//! with a downloaded binary has no checkout. So this writes what they write,
//! **under the same names and paths** -- the scheduled task `ITSaNAS` /
//! `ITSaNAS-NAME` and its wrapper, the systemd user units `itsanas.service` /
//! `itsanas@.service`, the `LaunchAgent` `net.itsanas.daemon` /
//! `net.itsanas.NAME`, the passphrase files beside them -- so that
//! `install/clean.ps1` and `install/clean.sh` remove a node set up here exactly
//! as they remove one provisioned, and a node provisioned by a script is one
//! `itsanas setup` recognises as done.
//!
//! Everything a platform is given -- the task, the unit, the plist, the
//! shortcut, the autostart entry -- is text built by a pure function below, so
//! the tests read it without installing anything; the platform's own tool
//! (`powershell`/`schtasks`, `systemctl --user`, `launchctl`) then takes it.
//! None of that text holds the passphrase. The passphrase goes in one file,
//! readable by this account alone, which the service reads at start: the trade
//! every background service makes, and the one the installers already made.
//!
//! The texts are in [`texts`], the passphrase file in [`passphrase_file`];
//! this module names things and runs the platform's tools.

mod passphrase_file;
mod texts;

pub(crate) use self::passphrase_file::{
    fits_service_file, launchd_disabled, read_passphrase_file, write_passphrase_file,
};
pub(crate) use self::texts::{
    autostart_desktop, launchd_daemon_plist, launchd_menubar_plist, ps_quote, systemd_unit,
    windows_task_script, windows_tray_script, windows_wrapper,
};

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::secrets::{Secret, powershell};
use std::fmt::Write as _;

use crate::error::{CliError, Result};

/// The Windows tray, copied beside `itsanas.exe` as `provision.ps1` does.
const TRAY_PS1: &str = include_str!("../../../../scripts/itsanas-tray.ps1");

/// The tray's base icon, copied beside it: the tray draws its state dot over
/// it, and a downloaded binary has no checkout to find it in.
const TRAY_ICO: &[u8] = include_bytes!("../../../../docs/assets/itsanas.ico");

/// The macOS menu-bar item, run by `osascript -l JavaScript`.
const MENUBAR_JS: &str = include_str!("../../../../scripts/itsanas-menubar.js");

/// The Linux desktop tray, run by `python3`.
const TRAY_PY: &str = include_str!("../../../../scripts/itsanas-tray.py");

/// What a background service can be asked to do. A trait so that `signout`
/// and `signin` are tested against a stand-in, never against this machine's
/// real scheduler.
pub trait ServiceControl {
    /// The file the service reads the passphrase from.
    fn passphrase_file(&self) -> PathBuf;
    /// The passphrase this machine remembers for the service, if there is a
    /// usable one. A desktop reads [`ServiceControl::passphrase_file`]; a phone
    /// answers from behind its keystore, so nothing above this trait names a file.
    fn stored_passphrase(&self) -> Option<Secret> {
        read_passphrase_file(&self.passphrase_file())
    }
    /// Remember `passphrase` for the service; says where, for a page or a log.
    fn store_passphrase(&self, passphrase: &Secret) -> Result<String> {
        let path = self.passphrase_file();
        write_passphrase_file(&path, passphrase)?;
        Ok(format!(
            "{} holds it, readable by this account alone (anything running as you can read it: \
             the trade every background service makes)",
            path.display()
        ))
    }
    /// Forget what [`ServiceControl::store_passphrase`] kept; says what was done.
    fn forget_passphrase(&self) -> Result<&'static str> {
        let file = self.passphrase_file();
        match std::fs::remove_file(&file) {
            Ok(()) => Ok("the passphrase file is deleted"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok("there was no passphrase file to delete")
            }
            Err(source) => Err(CliError::Io { path: file, source }),
        }
    }
    /// Whether the service is defined here (it may be stopped or disabled).
    fn installed(&self) -> bool;
    /// Define the service and, if asked, the tray's autostart; says what was
    /// written. Stops a running service first, so its program can be replaced.
    fn install(&self, tray: bool) -> Result<String>;
    /// Start it now.
    fn start(&self) -> Result<()>;
    /// Stop it now; it still starts at the next logon unless disabled.
    fn stop(&self) -> Result<()>;
    /// Whether it starts by itself at logon.
    fn set_autostart(&self, on: bool) -> Result<()>;
    /// Whether it starts by itself at logon, as the system says; `None` when
    /// that cannot be read.
    fn autostart(&self) -> Option<bool> {
        None
    }
    /// Take the service's definition away, and the tray's start at logon: an
    /// account removed from this machine leaves nothing that would start for it.
    /// An app that is its own service has nothing to take away.
    fn uninstall(&self) -> Result<()> {
        Ok(())
    }
    /// Where to look when it does not run.
    fn log_hint(&self) -> String;
    /// Whether the program asking is the one that holds the store, as an app that
    /// is its own service is. Nothing else will let go of it, so waiting for that
    /// only makes the person wait.
    fn holds_the_store_itself(&self) -> bool {
        false
    }
}

/// Every name that carries the instance, in one place, so no platform can
/// spell one differently from the installers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Names {
    pub(crate) instance: Option<String>,
    /// `-NAME`, or nothing for the default node.
    pub(crate) suffix: String,
    /// The Windows scheduled task.
    pub(crate) task: String,
    /// The systemd unit to start: `itsanas` or `itsanas@NAME`.
    pub(crate) systemd_service: String,
    /// The unit file that defines it.
    pub(crate) unit_file: String,
    /// The launchd label of the daemon.
    pub(crate) launchd_label: String,
    /// The launchd label of the menu-bar item.
    pub(crate) menubar_label: String,
    /// The Windows Startup shortcut of the tray.
    pub(crate) tray_shortcut: String,
    /// The Linux autostart entry of the tray.
    pub(crate) autostart_file: String,
}

impl Names {
    pub(crate) fn of(instance: Option<&str>) -> Self {
        let instance = instance.map(str::to_owned);
        let suffix = instance
            .as_deref()
            .map_or_else(String::new, |name| format!("-{name}"));
        match instance.as_deref() {
            None => Self {
                instance: None,
                task: "ITSaNAS".to_owned(),
                systemd_service: "itsanas".to_owned(),
                unit_file: "itsanas.service".to_owned(),
                launchd_label: "net.itsanas.daemon".to_owned(),
                menubar_label: "net.itsanas.menubar".to_owned(),
                tray_shortcut: "ITSaNAS tray.lnk".to_owned(),
                autostart_file: "itsanas-tray.desktop".to_owned(),
                suffix,
            },
            Some(name) => Self {
                task: format!("ITSaNAS-{name}"),
                systemd_service: format!("itsanas@{name}"),
                unit_file: "itsanas@.service".to_owned(),
                launchd_label: format!("net.itsanas.{name}"),
                menubar_label: format!("net.itsanas.menubar.{name}"),
                tray_shortcut: format!("ITSaNAS tray ({name}).lnk"),
                autostart_file: format!("itsanas-tray-{name}.desktop"),
                instance: instance.clone(),
                suffix,
            },
        }
    }
}

/// Where things go on this machine. Passed in, so tests point it at a
/// temporary directory and never at this machine's real profile.
#[derive(Clone, Debug)]
pub(crate) struct Paths {
    /// The user's home directory.
    pub(crate) user_home: PathBuf,
    /// `%LOCALAPPDATA%` on Windows.
    pub(crate) local_app_data: Option<PathBuf>,
    /// `ITSANAS_PREFIX`, which `linux.sh` and `macos.sh` honour.
    pub(crate) prefix: Option<PathBuf>,
}

impl Paths {
    pub(crate) fn of_this_machine() -> Self {
        Self {
            user_home: crate::config::user_home(),
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            prefix: std::env::var_os("ITSANAS_PREFIX").map(PathBuf::from),
        }
    }

    fn windows_root(&self) -> PathBuf {
        self.local_app_data
            .clone()
            .unwrap_or_else(|| self.user_home.join("AppData").join("Local"))
    }

    /// Where the programs are installed: `provision.ps1`'s `$binDir`, or
    /// `linux.sh`/`macos.sh`'s `$PREFIX/bin`.
    pub(crate) fn bin_dir(&self, os: &str) -> PathBuf {
        if os == "windows" {
            return self
                .windows_root()
                .join("Programs")
                .join("itsanas")
                .join("bin");
        }
        self.prefix
            .clone()
            .unwrap_or_else(|| self.user_home.join(".local"))
            .join("bin")
    }

    /// The directory of the passphrase files: `%LOCALAPPDATA%\itsanas`, or
    /// `~/.config/itsanas`.
    pub(crate) fn state_dir(&self, os: &str) -> PathBuf {
        if os == "windows" {
            self.windows_root().join("itsanas")
        } else {
            self.user_home.join(".config").join("itsanas")
        }
    }

    /// The passphrase file the service of `names` reads.
    pub(crate) fn passphrase_file(&self, os: &str, names: &Names) -> PathBuf {
        let dir = self.state_dir(os);
        if os == "windows" {
            return dir.join(format!("passphrase{}.txt", names.suffix));
        }
        match &names.instance {
            None => dir.join("environment"),
            Some(name) => dir.join(format!("{name}.environment")),
        }
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CliError + '_ {
    move |source| CliError::Io {
        path: path.to_owned(),
        source,
    }
}

// ---------------------------------------------------------------------------
// Running the platform's tools
// ---------------------------------------------------------------------------

/// Run a PowerShell script (no secret in it) and say why it failed.
pub(crate) fn run_powershell(script: &str) -> std::result::Result<String, String> {
    let mut prepared = powershell(script, &[]);
    run(&mut prepared.command, Some(prepared.stdin.as_bytes()))
}

/// Run a tool, feeding `input`; its output, or the reason it failed.
fn run(command: &mut Command, input: Option<&[u8]>) -> std::result::Result<String, String> {
    use std::io::Write as _;
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", command.get_program().display()))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(input);
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("{}: {error}", command.get_program().display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.lines().rev().find(|line| !line.trim().is_empty());
        Err(format!(
            "{} failed ({}){}",
            command.get_program().display(),
            output.status,
            said.map_or_else(String::new, |line| format!(": {}", line.trim()))
        ))
    }
}

fn tool(program: &str, args: &[&str]) -> std::result::Result<String, String> {
    run(Command::new(program).args(args), None)
}

fn failed(what: &str) -> impl FnOnce(String) -> CliError + '_ {
    move |why| CliError::Usage(format!("{what}: {why}"))
}

/// Copy this program to where the service runs it, unless it already is it.
///
/// Through a temporary name and a rename, as `macos.sh` does: a daemon that is
/// running keeps its old file, and nothing ever runs half a copy.
fn install_program(bin_dir: &Path, name: &str) -> Result<PathBuf> {
    let target = bin_dir.join(name);
    let current = std::env::current_exe().map_err(io(Path::new("<this program>")))?;
    let same = match (current.canonicalize(), target.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same {
        return Ok(target);
    }
    std::fs::create_dir_all(bin_dir).map_err(io(bin_dir))?;
    let pending = bin_dir.join(format!(".{name}.new"));
    std::fs::copy(&current, &pending).map_err(io(&pending))?;
    std::fs::rename(&pending, &target).map_err(io(&target))?;
    Ok(target)
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
    }
    std::fs::write(path, text).map_err(io(path))
}

/// Rewrite `path` with `text` when it exists and says something else;
/// `true` when it was rewritten. A missing file is a node `setup` never
/// installed as a service here, and is not created.
fn refresh_file(path: &Path, text: &str) -> std::io::Result<bool> {
    match std::fs::read(path) {
        Ok(old) if old == text.as_bytes() => Ok(false),
        Ok(_) => std::fs::write(path, text).map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// The real service of one node on this machine.
#[derive(Clone, Debug)]
pub(crate) struct Platform {
    pub(crate) os: &'static str,
    pub(crate) names: Names,
    pub(crate) paths: Paths,
    pub(crate) home: PathBuf,
}

impl Platform {
    pub(crate) fn of_this_machine(home: &Path, instance: Option<&str>) -> Self {
        Self {
            os: std::env::consts::OS,
            names: Names::of(instance),
            paths: Paths::of_this_machine(),
            home: home.to_owned(),
        }
    }

    fn unit_path(&self) -> PathBuf {
        self.paths
            .user_home
            .join(".config/systemd/user")
            .join(&self.names.unit_file)
    }

    fn plist_path(&self, label: &str) -> PathBuf {
        self.paths
            .user_home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"))
    }

    fn mac_log(&self) -> PathBuf {
        match self.names.instance {
            None => self.paths.user_home.join("Library/Logs/itsanas.log"),
            Some(_) => self.home.join("daemon.log"),
        }
    }

    /// `gui/UID`, the domain of this user's `LaunchAgents`.
    fn gui_domain() -> Result<String> {
        let uid = tool("id", &["-u"]).map_err(failed("could not read this user's id"))?;
        Ok(format!("gui/{}", uid.trim()))
    }

    /// `systemctl --user`, with `XDG_RUNTIME_DIR` found the way
    /// `provision.sh` finds it when a detached session (ssh host 'cmd') left
    /// it unset.
    fn systemctl(args: &[&str]) -> std::result::Result<String, String> {
        let mut command = Command::new("systemctl");
        command.arg("--user").args(args);
        if std::env::var_os("XDG_RUNTIME_DIR").is_none()
            && let Ok(uid) = tool("id", &["-u"])
        {
            let runtime = PathBuf::from(format!("/run/user/{}", uid.trim()));
            if runtime.is_dir() {
                command.env("XDG_RUNTIME_DIR", runtime);
            }
        }
        run(&mut command, None)
    }

    /// Bring this node's Windows wrapper up to this version, if `setup`
    /// wrote one, and say so. `update` replaces the program and nothing else,
    /// so a wrapper from an older `setup` stayed as it was: the laptop's log
    /// of 2026-10-22 still wrapped each line in a `NativeCommandError` block
    /// after #305 fixed the wrapper, and treated exit 75 as a failure. A
    /// wrapper's loop already running keeps the text it started with; the
    /// new one runs from the next logon.
    pub(crate) fn refresh_windows_wrapper(&self) -> Option<String> {
        if self.os != "windows" {
            return None;
        }
        let wrapper = self
            .paths
            .state_dir(self.os)
            .join(format!("run-daemon{}.ps1", self.names.suffix));
        match refresh_file(&wrapper, &windows_wrapper(&self.names)) {
            Ok(true) => Some(format!(
                "wrapper {} brought up to date (used from the next logon)",
                wrapper.display()
            )),
            Ok(false) => None,
            Err(error) => Some(format!("could not update {}: {error}", wrapper.display())),
        }
    }

    fn install_windows(&self, tray: bool) -> Result<String> {
        let bin_dir = self.paths.bin_dir(self.os);
        install_program(&bin_dir, "itsanas.exe")?;
        let wrapper = self
            .paths
            .state_dir(self.os)
            .join(format!("run-daemon{}.ps1", self.names.suffix));
        write_file(&wrapper, &windows_wrapper(&self.names))?;
        run_powershell(&windows_task_script(&self.names, &wrapper)).map_err(failed(
            "could not register the scheduled task (if it was created from an elevated \
             PowerShell, remove it there: Unregister-ScheduledTask)",
        ))?;
        let mut said = format!("scheduled task {} (at logon)", self.names.task);
        if tray {
            let script = bin_dir.join("itsanas-tray.ps1");
            write_file(&script, TRAY_PS1)?;
            let ico = bin_dir.join("itsanas.ico");
            std::fs::write(&ico, TRAY_ICO).map_err(io(&ico))?;
            run_powershell(&windows_tray_script(&self.names, &script, &bin_dir))
                .map_err(failed("could not put the tray icon in the Startup folder"))?;
            let _ = write!(said, ", tray icon at logon ({})", self.names.tray_shortcut);
        }
        Ok(said)
    }

    fn install_linux(&self, tray: bool) -> Result<String> {
        let bin_dir = self.paths.bin_dir(self.os);
        let exe = install_program(&bin_dir, "itsanas")?;
        let unit = self.unit_path();
        // A unit install/linux.sh wrote is left alone: it is this text, and
        // somebody may have tuned a limit in it.
        if !unit.exists() {
            write_file(&unit, &systemd_unit(self.names.instance.is_some(), &exe))?;
        }
        Self::systemctl(&["daemon-reload"]).map_err(failed(
            "systemd did not take the unit (is there a user session?)",
        ))?;
        let mut said = format!("systemd user service {}", self.names.systemd_service);
        if tool("loginctl", &["show-user", &whoami(), "-p", "Linger"])
            .is_ok_and(|out| out.trim() == "Linger=no")
        {
            let _ = write!(
                said,
                "; it stops when you log out unless: sudo loginctl enable-linger {}",
                whoami()
            );
        }
        if tray {
            let script = bin_dir.join("itsanas-tray.py");
            write_file(&script, TRAY_PY)?;
            let entry = self
                .paths
                .user_home
                .join(".config/autostart")
                .join(&self.names.autostart_file);
            write_file(&entry, &autostart_desktop(&self.names, &script))?;
            let _ = write!(said, ", tray at login ({})", entry.display());
            // And now, when there is a desktop to draw in, as on Windows.
            if std::env::var_os("DISPLAY").is_some()
                || std::env::var_os("WAYLAND_DISPLAY").is_some()
            {
                start_linux_tray(&script, self.names.instance.as_deref());
            }
        }
        Ok(said)
    }

    fn install_macos(&self, tray: bool) -> Result<String> {
        let bin_dir = self.paths.bin_dir(self.os);
        let exe = install_program(&bin_dir, "itsanas")?;
        let home = self.names.instance.as_ref().map(|_| self.home.as_path());
        let plist = self.plist_path(&self.names.launchd_label);
        write_file(
            &plist,
            &launchd_daemon_plist(
                &self.names,
                &exe,
                &self.passphrase_file(),
                home,
                &self.mac_log(),
            ),
        )?;
        let mut said = format!("LaunchAgent {}", self.names.launchd_label);
        if tray {
            let script = bin_dir.join("itsanas-menubar.js");
            write_file(&script, MENUBAR_JS)?;
            let menubar = self.plist_path(&self.names.menubar_label);
            write_file(&menubar, &launchd_menubar_plist(&self.names, &script))?;
            let domain = Self::gui_domain()?;
            let _ = tool(
                "launchctl",
                &["bootout", &format!("{domain}/{}", self.names.menubar_label)],
            );
            tool(
                "launchctl",
                &["bootstrap", &domain, &menubar.display().to_string()],
            )
            .map_err(failed("launchd refused the menu-bar item"))?;
            let _ = write!(said, ", menu-bar item ({})", self.names.menubar_label);
        }
        Ok(said)
    }
}

/// Start the Linux tray unless one for this node already runs. Best effort:
/// the autostart entry is the guarantee, this only spares a logout.
fn start_linux_tray(script: &Path, instance: Option<&str>) {
    let pattern = format!(
        "{}{}",
        script.display(),
        instance.map_or(String::new(), |name| format!(" {name}"))
    );
    if tool("pgrep", &["-f", &pattern]).is_ok() {
        return;
    }
    let mut command = std::process::Command::new("python3");
    command.arg(script);
    if let Some(name) = instance {
        command.arg(name);
    }
    let _ = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

impl ServiceControl for Platform {
    fn passphrase_file(&self) -> PathBuf {
        self.paths.passphrase_file(self.os, &self.names)
    }

    fn installed(&self) -> bool {
        match self.os {
            "windows" => tool("schtasks", &["/Query", "/TN", &self.names.task]).is_ok(),
            "macos" => self.plist_path(&self.names.launchd_label).is_file(),
            _ => self.unit_path().is_file(),
        }
    }

    fn install(&self, tray: bool) -> Result<String> {
        if self.installed() {
            // Its program may be the file about to be replaced, and Windows
            // refuses to replace a program that runs.
            let _ = self.stop();
        }
        match self.os {
            "windows" => self.install_windows(tray),
            "macos" => self.install_macos(tray),
            _ => self.install_linux(tray),
        }
    }

    fn start(&self) -> Result<()> {
        match self.os {
            "windows" => tool("schtasks", &["/Run", "/TN", &self.names.task]).map(|_| ()),
            "macos" => {
                let domain = Self::gui_domain()?;
                let _ = tool(
                    "launchctl",
                    &["bootout", &format!("{domain}/{}", self.names.launchd_label)],
                );
                tool(
                    "launchctl",
                    &[
                        "bootstrap",
                        &domain,
                        &self
                            .plist_path(&self.names.launchd_label)
                            .display()
                            .to_string(),
                    ],
                )
                .map(|_| ())
            }
            _ => Self::systemctl(&["start", &self.names.systemd_service]).map(|_| ()),
        }
        .map_err(failed("the background service did not start"))
    }

    fn stop(&self) -> Result<()> {
        match self.os {
            "windows" => tool("schtasks", &["/End", "/TN", &self.names.task]).map(|_| ()),
            "macos" => {
                let domain = Self::gui_domain()?;
                tool(
                    "launchctl",
                    &["bootout", &format!("{domain}/{}", self.names.launchd_label)],
                )
                .map(|_| ())
            }
            _ => Self::systemctl(&["stop", &self.names.systemd_service]).map(|_| ()),
        }
        .map_err(failed("the background service did not stop"))
    }

    fn set_autostart(&self, on: bool) -> Result<()> {
        match self.os {
            "windows" => tool(
                "schtasks",
                &[
                    "/Change",
                    "/TN",
                    &self.names.task,
                    if on { "/ENABLE" } else { "/DISABLE" },
                ],
            )
            .map(|_| ()),
            "macos" => {
                let domain = Self::gui_domain()?;
                tool(
                    "launchctl",
                    &[
                        if on { "enable" } else { "disable" },
                        &format!("{domain}/{}", self.names.launchd_label),
                    ],
                )
                .map(|_| ())
            }
            _ => Self::systemctl(&[
                if on { "enable" } else { "disable" },
                &self.names.systemd_service,
            ])
            .map(|_| ()),
        }
        .map_err(failed(
            "could not change whether the service starts at logon",
        ))
    }

    fn uninstall(&self) -> Result<()> {
        let gone = |path: PathBuf| match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(CliError::Io {
                path,
                source: error,
            }),
            _ => Ok(()),
        };
        match self.os {
            "windows" => {
                tool("schtasks", &["/Delete", "/TN", &self.names.task, "/F"])
                    .map_err(failed("the background service was not removed"))?;
                run_powershell(&format!(
                    "Remove-Item -LiteralPath (Join-Path ([Environment]::GetFolderPath('Startup')) {}) -ErrorAction SilentlyContinue",
                    ps_quote(&self.names.tray_shortcut)
                ))
                .map(|_| ())
                .map_err(failed("the tray's start at logon was not removed"))
            }
            "macos" => {
                if let Ok(domain) = Self::gui_domain() {
                    for label in [&self.names.launchd_label, &self.names.menubar_label] {
                        let _ = tool("launchctl", &["bootout", &format!("{domain}/{label}")]);
                    }
                }
                gone(self.plist_path(&self.names.launchd_label))?;
                gone(self.plist_path(&self.names.menubar_label))
            }
            _ => {
                let _ = Self::systemctl(&["disable", "--now", &self.names.systemd_service]);
                // An instance runs from the template every other instance shares.
                if self.names.instance.is_none() {
                    gone(self.unit_path())?;
                }
                gone(
                    self.paths
                        .user_home
                        .join(".config/autostart")
                        .join(&self.names.autostart_file),
                )
            }
        }
    }

    fn autostart(&self) -> Option<bool> {
        match self.os {
            // The state's name, not `schtasks`' words, which follow the language.
            "windows" => run_powershell(&format!(
                "(Get-ScheduledTask -TaskName {}).State",
                ps_quote(&self.names.task)
            ))
            .ok()
            .map(|state| state.trim() != "Disabled"),
            "macos" => {
                let domain = Self::gui_domain().ok()?;
                let listing = tool("launchctl", &["print-disabled", &domain]).ok()?;
                Some(!launchd_disabled(&listing, &self.names.launchd_label))
            }
            _ => Some(Self::systemctl(&["is-enabled", &self.names.systemd_service]).is_ok()),
        }
    }

    fn log_hint(&self) -> String {
        match self.os {
            "windows" => self
                .paths
                .state_dir(self.os)
                .join(format!("daemon{}.log", self.names.suffix))
                .display()
                .to_string(),
            "macos" => self.mac_log().display().to_string(),
            _ => format!("journalctl --user -u {}", self.names.systemd_service),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_matches_what_the_installers_and_clean_scripts_use() {
        // clean.ps1 and clean.sh remove by these exact names: one letter off
        // and an uninstall leaves a service starting at every logon.
        let tester = Names::of(Some("tester"));
        assert_eq!(tester.task, "ITSaNAS-tester");
        assert_eq!(tester.systemd_service, "itsanas@tester");
        assert_eq!(tester.launchd_label, "net.itsanas.tester");
        assert_eq!(tester.tray_shortcut, "ITSaNAS tray (tester).lnk");
        assert_eq!(tester.menubar_label, "net.itsanas.menubar.tester");
        assert_eq!(tester.autostart_file, "itsanas-tray-tester.desktop");
        let default = Names::of(None);
        assert_eq!(default.task, "ITSaNAS");
        assert_eq!(default.launchd_label, "net.itsanas.daemon");
        assert_eq!(default.tray_shortcut, "ITSaNAS tray.lnk");
        let paths = Paths {
            user_home: PathBuf::from("/home/u"),
            local_app_data: Some(PathBuf::from(r"C:\Users\u\AppData\Local")),
            prefix: None,
        };
        assert_eq!(
            paths.passphrase_file("linux", &tester),
            PathBuf::from("/home/u/.config/itsanas/tester.environment"),
            "the unit's EnvironmentFile and clean.sh name this file"
        );
        assert_eq!(
            paths.passphrase_file("windows", &tester),
            PathBuf::from(r"C:\Users\u\AppData\Local")
                .join("itsanas")
                .join("passphrase-tester.txt"),
            "the wrapper reads, and clean.ps1 removes, passphrase-NAME.txt"
        );
        assert!(
            windows_wrapper(&tester).contains("$instanceSuffix = '-tester'"),
            "the task of instance tester would start the default node"
        );
    }

    #[test]
    fn an_update_brings_an_old_wrapper_up_to_date_and_creates_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wrapper = dir.path().join("run-daemon-test.ps1");
        let now = windows_wrapper(&Names::of(Some("test")));
        assert!(
            !refresh_file(&wrapper, &now).expect("refresh"),
            "a node with no wrapper reported one rewritten"
        );
        assert!(
            !wrapper.exists(),
            "a wrapper was created for a node setup never installed as a service"
        );
        std::fs::write(&wrapper, "& itsanas.exe daemon *>&1 | Out-File daemon.log").expect("old");
        assert!(refresh_file(&wrapper, &now).expect("refresh"));
        assert_eq!(
            std::fs::read_to_string(&wrapper).expect("read"),
            now,
            "an old wrapper survived the update: its daemon logs NativeCommandError blocks \
             and waits a failure pause on every requested restart"
        );
        assert!(
            !refresh_file(&wrapper, &now).expect("refresh"),
            "an up-to-date wrapper was rewritten at every update"
        );
    }
}
