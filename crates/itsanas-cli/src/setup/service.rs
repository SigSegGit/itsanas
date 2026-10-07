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
pub(crate) trait ServiceControl {
    /// The file the service reads the passphrase from.
    fn passphrase_file(&self) -> PathBuf;
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
    /// Where to look when it does not run.
    fn log_hint(&self) -> String;
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

// ---------------------------------------------------------------------------
// The texts, as pure functions
// ---------------------------------------------------------------------------

/// A PowerShell single-quoted string: nothing inside is interpreted.
pub(crate) fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Text safe inside an XML element.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One argument of a `.desktop` file's `Exec=`, quoted as its specification
/// asks.
fn desktop_quote(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// The wrapper the Windows task runs: `provision.ps1`'s, line for line in
/// what it does (read the passphrase file, log to a rotated UTF-8 file,
/// restart a daemon that dies), so the two installs are one install.
pub(crate) fn windows_wrapper(names: &Names) -> String {
    [
        "# Started by the ITSaNAS scheduled task at logon (written by `itsanas setup`, the",
        "# same wrapper as install/provision.ps1). A daemon cannot be prompted, so the",
        "# passphrase comes from a file only this account can read.",
        &format!("$instanceSuffix = {}", ps_quote(&names.suffix)),
        "if ($instanceSuffix) { $env:ITSANAS_HOME = \"$env:USERPROFILE\\.itsanas$instanceSuffix\" }",
        "$env:ITSANAS_PASSPHRASE = Get-Content \"$env:LOCALAPPDATA\\itsanas\\passphrase$instanceSuffix.txt\" -Raw",
        "$log = \"$env:LOCALAPPDATA\\itsanas\\daemon$instanceSuffix.log\"",
        "if ((Test-Path $log) -and ((Get-Item $log).Length -gt 5MB)) {",
        "    Move-Item -LiteralPath $log -Destination \"$log.1\" -Force",
        "}",
        "# Restarts live here, not in the task: conhost --headless reports exit 0",
        "# whatever happened, so the scheduler's restart-on-failure never fires.",
        "$pause = 10",
        "while ($true) {",
        "    & \"$env:LOCALAPPDATA\\Programs\\itsanas\\bin\\itsanas.exe\" daemon *>&1 |",
        "        Out-File -LiteralPath $log -Encoding utf8 -Append",
        "    if ($LASTEXITCODE -eq 0) { break }",
        "    \"daemon exited with $LASTEXITCODE; restarting in $pause s\" |",
        "        Out-File -LiteralPath $log -Encoding utf8 -Append",
        "    Start-Sleep -Seconds $pause",
        "    $pause = [Math]::Min($pause * 2, 300)",
        "}",
        "",
    ]
    .join("\r\n")
}

/// The script that registers the Windows task: `provision.ps1`'s action,
/// trigger and settings (conhost `--headless`, so no black window anybody
/// closes; at logon; on battery too).
pub(crate) fn windows_task_script(names: &Names, wrapper: &Path) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $taskName = {task}\n\
         $wrapper = {wrapper}\n\
         $action = New-ScheduledTaskAction -Execute 'conhost.exe' `\n    \
         -Argument \"--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File `\"$wrapper`\"\"\n\
         $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `\n    \
         -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)\n\
         Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `\n    \
         -Settings $settings -Force -Description 'ITSaNAS peer-to-peer storage daemon' | Out-Null\n",
        task = ps_quote(&names.task),
        wrapper = ps_quote(&wrapper.display().to_string()),
    )
}

/// The script that makes a file this account's alone: `provision.ps1`'s
/// `FileInfo.SetAccessControl`, which works where `icacls` and `Set-Acl`
/// were each measured to fail (see the comment there).
#[cfg_attr(not(windows), allow(dead_code))] // called by the Windows branch and its tests
pub(crate) fn windows_acl_script(path: &Path) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $acl = New-Object System.Security.AccessControl.FileSecurity\n\
         $acl.SetAccessRuleProtection($true, $false)\n\
         $acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule(\n    \
         $env:USERNAME, 'FullControl', 'Allow')))\n\
         ([System.IO.FileInfo]{path}).SetAccessControl($acl)\n",
        path = ps_quote(&path.display().to_string()),
    )
}

/// The script that puts the tray's shortcut in the Startup folder:
/// `provision.ps1`'s, with the same name, target and arguments, so
/// `clean.ps1` finds it.
pub(crate) fn windows_tray_script(names: &Names, tray: &Path, bin_dir: &Path) -> String {
    let instance = names
        .instance
        .as_deref()
        .map_or_else(String::new, |name| format!(" -Instance {name}"));
    let description = names.instance.as_deref().map_or_else(
        || "ITSaNAS tray icon".to_owned(),
        |name| format!("ITSaNAS tray icon for {name}"),
    );
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $startup = if ($env:ITSANAS_STARTUP_DIR) {{ $env:ITSANAS_STARTUP_DIR }} else {{ [Environment]::GetFolderPath('Startup') }}\n\
         if (-not $startup) {{ throw 'this account has no Startup folder' }}\n\
         $link = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $startup {shortcut}))\n\
         $link.TargetPath = Join-Path $env:windir 'System32\\conhost.exe'\n\
         $link.Arguments = {arguments}\n\
         $link.WorkingDirectory = {bin}\n\
         $link.Description = {description}\n\
         $link.Save()\n\
         # Started now too, not only at the next logon: an install that ends with\n\
         # nothing on screen reads as an install that did not work. Once: a second\n\
         # run of setup must not put a second icon beside the first.\n\
         $running = Get-CimInstance Win32_Process -Filter \"Name='powershell.exe'\" |\n\
         \x20   Where-Object {{ $_.CommandLine -like {running} }}\n\
         if (-not $running) {{\n\
         \x20   Start-Process -FilePath $link.TargetPath -ArgumentList $link.Arguments -WorkingDirectory $link.WorkingDirectory\n\
         }}\n",
        shortcut = ps_quote(&names.tray_shortcut),
        arguments = ps_quote(&format!(
            "--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\"{instance}",
            tray.display()
        )),
        bin = ps_quote(&bin_dir.display().to_string()),
        description = ps_quote(&description),
        running = ps_quote(&format!("*{}\"{instance}", tray.display())),
    )
}

/// The systemd user unit: `install/linux.sh`'s, directive for directive.
///
/// `template` is the `itsanas@.service` form, whose `%i` is the instance.
/// The passphrase is not here: `EnvironmentFile=` names the file holding it.
pub(crate) fn systemd_unit(template: bool, exe: &Path) -> String {
    let (description, home, env_file, state) = if template {
        (
            "ITSaNAS peer-to-peer storage, instance %i",
            "Environment=ITSANAS_HOME=%h/.itsanas-%i\n",
            "%h/.config/itsanas/%i.environment",
            "%h/.itsanas-%i",
        )
    } else {
        (
            "ITSaNAS peer-to-peer storage",
            "",
            "%h/.config/itsanas/environment",
            "%h/.itsanas",
        )
    };
    format!(
        "[Unit]\n\
         Description={description}\n\
         Documentation=https://github.com/SigSegGit/itsanas\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart=\"{exe}\" daemon\n\
         {home}\
         Restart=on-failure\n\
         RestartSec=30\n\
         # Written by `itsanas setup`; install/linux.sh explains each limit below.\n\
         MemoryHigh=384M\n\
         MemoryMax=512M\n\
         OOMScoreAdjust=500\n\
         CPUWeight=50\n\
         IOWeight=50\n\
         EnvironmentFile=-{env_file}\n\
         PrivateTmp=true\n\
         ProtectSystem=strict\n\
         ProtectHome=false\n\
         NoNewPrivileges=true\n\
         ReadWritePaths=-{state} -%h/.config/itsanas\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = exe.display(),
    )
}

/// The daemon's `LaunchAgent`: `macos.sh`'s, without the passphrase.
///
/// `macos.sh` left a commented `EnvironmentVariables` block for the person to
/// fill in, which puts the secret in a plist; here a two-line `sh` reads it
/// from the same environment file Linux uses, so the plist holds no secret
/// and `clean.sh` removes the file it already knows.
pub(crate) fn launchd_daemon_plist(
    names: &Names,
    exe: &Path,
    env_file: &Path,
    home: Option<&Path>,
    log: &Path,
) -> String {
    let script = "ITSANAS_PASSPHRASE=$(sed -n 's/^ITSANAS_PASSPHRASE=//p' \"$1\" | head -n 1); \
                  export ITSANAS_PASSPHRASE; exec \"$2\" daemon";
    let environment = home.map_or_else(String::new, |home| {
        format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>ITSANAS_HOME</key>\n    <string>{}</string>\n  </dict>\n",
            xml_escape(&home.display().to_string())
        )
    });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key>\n  <string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n\
         \x20   <string>/bin/sh</string>\n    <string>-c</string>\n    <string>{script}</string>\n\
         \x20   <string>itsanas-daemon</string>\n    <string>{env_file}</string>\n    <string>{exe}</string>\n\
         \x20 </array>\n\
         {environment}\
         \x20 <key>RunAtLoad</key>\n  <true/>\n\
         \x20 <key>KeepAlive</key>\n  <dict><key>SuccessfulExit</key><false/></dict>\n\
         \x20 <key>StandardOutPath</key>\n  <string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
         </dict>\n</plist>\n",
        label = xml_escape(&names.launchd_label),
        script = xml_escape(script),
        env_file = xml_escape(&env_file.display().to_string()),
        exe = xml_escape(&exe.display().to_string()),
        log = xml_escape(&log.display().to_string()),
    )
}

/// The menu-bar item's `LaunchAgent`: `osascript -l JavaScript` on the script,
/// in the person's graphical session only.
pub(crate) fn launchd_menubar_plist(names: &Names, script: &Path) -> String {
    let instance = names.instance.as_deref().map_or_else(String::new, |name| {
        format!("    <string>{}</string>\n", xml_escape(name))
    });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key>\n  <string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n\
         \x20   <string>/usr/bin/osascript</string>\n    <string>-l</string>\n    <string>JavaScript</string>\n\
         \x20   <string>{script}</string>\n{instance}\
         \x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n  <true/>\n\
         \x20 <key>LimitLoadToSessionType</key>\n  <string>Aqua</string>\n\
         </dict>\n</plist>\n",
        label = xml_escape(&names.menubar_label),
        script = xml_escape(&script.display().to_string()),
    )
}

/// The Linux tray's autostart entry (XDG autostart, read by every desktop).
pub(crate) fn autostart_desktop(names: &Names, script: &Path) -> String {
    let (name, argument) = names.instance.as_deref().map_or_else(
        || ("ITSaNAS tray".to_owned(), String::new()),
        |instance| {
            (
                format!("ITSaNAS tray ({instance})"),
                format!(" {}", desktop_quote(instance)),
            )
        },
    );
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment=Shows whether this ITSaNAS node is syncing\n\
         Exec=python3 {script}{argument}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        script = desktop_quote(&script.display().to_string()),
    )
}

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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CliError + '_ {
    move |source| CliError::Io {
        path: path.to_owned(),
        source,
    }
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
    fn names_and_paths_are_quoted_where_they_land() {
        let names = Names::of(Some("o-brien"));
        let script = windows_task_script(&names, Path::new(r"C:\Users\O'Brien\run.ps1"));
        assert!(
            script.contains(r"'C:\Users\O''Brien\run.ps1'"),
            "a quote in the profile path breaks the task script, and setup fails for that person: {script}"
        );
        let plist = launchd_daemon_plist(
            &names,
            Path::new("/Users/a&b/bin/itsanas"),
            Path::new("/e"),
            Some(Path::new("/Users/a&b/.itsanas-o-brien")),
            Path::new("/l"),
        );
        assert!(
            plist.contains("/Users/a&amp;b/bin/itsanas") && !plist.contains("a&b"),
            "an ampersand in the home makes the plist invalid XML and launchd refuses it"
        );
        let entry = autostart_desktop(&names, Path::new("/home/x y/$t/itsanas-tray.py"));
        assert!(
            entry.contains("Exec=python3 \"/home/x y/\\$t/itsanas-tray.py\" \"o-brien\""),
            "a space or dollar in the path starts a different program at login: {entry}"
        );
    }

    #[test]
    fn the_windows_tray_starts_now_and_only_once() {
        for (instance, tail) in [(None, "\"'"), (Some("tester"), "\" -Instance tester'")] {
            let script = windows_tray_script(
                &Names::of(instance),
                Path::new(r"C:\x\itsanas-tray.ps1"),
                Path::new(r"C:\x"),
            );
            assert!(
                script.contains("Start-Process -FilePath $link.TargetPath"),
                "setup saves the tray shortcut without starting it: nothing is on screen until \
                 the next logon, and the install looks like it failed: {script}"
            );
            assert!(
                script.contains(&format!(r"-like '*C:\x\itsanas-tray.ps1{tail}")),
                "the tray is started without checking for one already running ({instance:?}): \
                 every run of setup adds an icon: {script}"
            );
        }
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

    /// Parse-check every PowerShell text this module generates.
    #[cfg(windows)]
    #[test]
    fn the_windows_scripts_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        for instance in [None, Some("tester")] {
            let names = Names::of(instance);
            let scripts = [
                ("wrapper", windows_wrapper(&names)),
                (
                    "task",
                    windows_task_script(&names, Path::new(r"C:\O'x\run.ps1")),
                ),
                (
                    "tray",
                    windows_tray_script(&names, Path::new(r"C:\x\t.ps1"), Path::new(r"C:\x")),
                ),
                ("acl", windows_acl_script(Path::new(r"C:\x\p.txt"))),
                ("window", super::super::secrets::WINFORMS_SCRIPT.to_owned()),
                ("tray icon", TRAY_PS1.to_owned()),
            ];
            for (what, text) in scripts {
                let path = dir.path().join(format!("{what}.ps1"));
                std::fs::write(&path, text).expect("write");
                let errors = run_powershell(&format!(
                    "$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile({}, [ref]$null, [ref]$e); \
                     $e | ForEach-Object {{ $_.ToString() }}",
                    ps_quote(&path.display().to_string())
                ))
                .expect("run the parser");
                assert!(
                    errors.trim().is_empty(),
                    "the {what} script for {instance:?} does not parse, so setup would fail on \
                     Windows at that step: {errors}"
                );
            }
        }
    }
}
