//! The desktop around the page: whether there is one, the browser, the
//! folder picker, opening and pinning a folder, and the entry points that
//! open the page.

use std::{
    fmt::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use super::{
    super::answers,
    Context, IDLE, Mode, Server,
    backends::Backends,
    http::{Request, Response},
    json::{self, field, object, optional, quote},
    ok_json,
    state::existing_ancestor,
};
use crate::{
    config::Config,
    error::{CliError, Result},
    node::Node,
};

/// Whether a browser window opened here would be in front of somebody.
///
/// Over SSH it would open on the remote machine's screen, or nowhere; a
/// Linux session with no `DISPLAY` or `WAYLAND_DISPLAY` has no screen.
pub(crate) fn has_desktop(os: &str, over_ssh: bool, display: bool) -> bool {
    if over_ssh {
        return false;
    }
    matches!(os, "windows" | "macos") || display
}

pub(crate) fn desktop_here() -> bool {
    let over_ssh =
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some();
    let display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    has_desktop(std::env::consts::OS, over_ssh, display)
}

/// The program that opens `url` in the default browser: what each system
/// ships, so nothing is installed for it. `rundll32` rather than `cmd /c
/// start`, because `cmd` would read the `&` of a URL as a second command.
pub(crate) fn browser_command(os: &str, url: &str) -> Command {
    let mut command = match os {
        "windows" => {
            let mut command = Command::new("rundll32");
            command.arg("url.dll,FileProtocolHandler");
            command
        }
        "macos" => Command::new("open"),
        _ => Command::new("xdg-open"),
    };
    command.arg(url);
    command
}

/// The system's own folder chooser, started from `start`. Returns the
/// program to run; the chosen path is its standard output. The start folder
/// goes in an environment variable, never inside the script's text.
pub(crate) fn picker_command(os: &str, start: &Path) -> Command {
    let mut command = match os {
        "windows" => {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-STA",
                "-Command",
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
                 Add-Type -AssemblyName System.Windows.Forms; \
                 $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
                 $d.Description = 'ITSaNAS'; $d.ShowNewFolderButton = $true; \
                 $d.SelectedPath = $env:ITSANAS_PICK_START; \
                 $owner = New-Object System.Windows.Forms.Form; $owner.TopMost = $true; \
                 if ($d.ShowDialog($owner) -eq 'OK') { [Console]::Out.Write($d.SelectedPath) }",
            ]);
            command
        }
        "macos" => {
            let mut command = Command::new("osascript");
            command.args([
                "-e",
                "set start to POSIX file (system attribute \"ITSANAS_PICK_START\")",
                "-e",
                "POSIX path of (choose folder with prompt \"ITSaNAS\" default location start)",
            ]);
            command
        }
        _ => {
            let mut command = Command::new("zenity");
            command.args(["--file-selection", "--directory", "--title=ITSaNAS"]);
            command.arg(format!("--filename={}/", start.display()));
            command
        }
    };
    command.env("ITSANAS_PICK_START", start);
    command
}

/// Ask the system's folder chooser; `{"path": ...}`, or no path when the
/// person cancelled.
pub(super) fn pick(request: &Request) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let base = crate::config::user_home();
    let start = field(&form, "start").map_or_else(|| base.clone(), |p| answers::expand(p, &base));
    let start = existing_ancestor(&start).map_or(base, Path::to_path_buf);
    let output = picker_command(std::env::consts::OS, &start)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) => {
            let chosen = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            // The free room of the disk chosen, for a page that offers space on it.
            let free = (!chosen.is_empty())
                .then(|| crate::drives::free_bytes(Path::new(&chosen)))
                .flatten();
            ok_json(object(&[
                (
                    "path",
                    optional((!chosen.is_empty()).then_some(chosen.as_str())),
                ),
                (
                    "free_bytes",
                    optional(free.map(|f| f.to_string()).as_deref()),
                ),
            ]))
        }
        Err(error) => Response::refuse(
            500,
            &format!("this system's folder chooser could not open ({error}): type the path"),
        ),
    }
}

/// The program that shows `folder` in the file manager.
pub(crate) fn open_command(os: &str, folder: &Path) -> Command {
    let mut command = Command::new(match os {
        "windows" => "explorer.exe",
        "macos" => "open",
        _ => "xdg-open",
    });
    command.arg(folder);
    command
}

pub(super) fn open_folder(ctx: &Context, request: &Request, pin: bool) -> Response {
    let form = json::parse_form(&request.body).unwrap_or_default();
    let folder = field(&form, "path")
        .map(|p| answers::expand(p, &ctx.base))
        .or_else(|| {
            Config::load(&Node::config_path(&ctx.home))
                .ok()
                .and_then(|config| config.folder)
        });
    let Some(folder) = folder.filter(|folder| folder.is_dir()) else {
        return Response::refuse(400, "the folder does not exist yet");
    };
    let os = std::env::consts::OS;
    let said = if pin {
        match pin_folder(os, &folder) {
            Ok(said) => said,
            Err(why) => return Response::refuse(500, &why),
        }
    } else {
        match open_command(os, &folder)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
                format!("opened {}", folder.display())
            }
            Err(error) => return Response::refuse(500, &format!("could not open it: {error}")),
        }
    };
    ok_json(object(&[("said", quote(&said))]))
}

/// The line a GTK bookmarks file holds for `folder`.
pub(crate) fn gtk_bookmark(folder: &Path) -> String {
    let mut url = String::from("file://");
    for byte in folder.display().to_string().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

pub(super) fn pin_folder(os: &str, folder: &Path) -> std::result::Result<String, String> {
    match os {
        "windows" => {
            let status = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    "(New-Object -ComObject Shell.Application).Namespace($env:ITSANAS_PIN).Self.InvokeVerb('pintohome')",
                ])
                .env("ITSANAS_PIN", folder)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(format!("{} is in Quick access", folder.display()))
            } else {
                Err(
                    "Windows refused to pin it: right-click the folder, Pin to Quick access"
                        .to_owned(),
                )
            }
        }
        "macos" => {
            Ok("macOS has no command for this: drag the folder into Finder's sidebar".to_owned())
        }
        _ => {
            let file = crate::config::user_home().join(".config/gtk-3.0/bookmarks");
            let line = gtk_bookmark(folder);
            let existing = std::fs::read_to_string(&file).unwrap_or_default();
            if !existing
                .lines()
                .any(|l| l.split(' ').next() == Some(line.as_str()))
            {
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                }
                let mut text = existing;
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&line);
                text.push_str(" ITSaNAS\n");
                std::fs::write(&file, text).map_err(|error| error.to_string())?;
            }
            Ok(format!(
                "{} is in your file manager's bookmarks",
                folder.display()
            ))
        }
    }
}

/// `itsanas setup` on a desktop.
pub(crate) fn wizard(home: &Path, instance: Option<&str>) -> Result<()> {
    if open(home, instance, Mode::Setup, None)? {
        println!("This machine is set up. `itsanas settings` changes it later.");
        Ok(())
    } else {
        Err(CliError::Usage(
            "setup did not finish in the page. Run `itsanas setup` again: the steps already \
             done are kept and not asked again (`itsanas setup --text` asks in this terminal)."
                .to_owned(),
        ))
    }
}

/// `itsanas settings [--section NAME]`.
pub(crate) fn settings(home: &Path, instance: Option<&str>, section: Option<&str>) -> Result<()> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    open(home, instance, Mode::Settings, section).map(|_| ())
}

/// The page's address, opened at `section`: inside the one fragment an
/// address has, after the page key, which `app.js` reads wherever it is.
pub(crate) fn section_url(url: String, section: Option<&str>) -> String {
    match section {
        Some(section) => format!("{url}&section={section}"),
        None => url,
    }
}

pub(super) fn open(
    home: &Path,
    instance: Option<&str>,
    mode: Mode,
    section: Option<&str>,
) -> Result<bool> {
    let server = Server::bind(
        home,
        instance,
        mode,
        &crate::config::user_home(),
        Backends::of_this_machine(home, instance),
    )?;
    let url = section_url(server.url(), section);
    if desktop_here() {
        println!("Opening ITSaNAS in your browser.");
        println!("If no page opened, paste this into your browser:");
        println!("  {url}");
        launch_browser(&url);
    } else {
        // `settings` over SSH: the page is still the way in, through a
        // forwarded port that keeps the same number, so the Host matches.
        let port = server.ctx.port;
        println!("No desktop here. From your own computer, run");
        println!("  ssh -L {port}:127.0.0.1:{port} <this machine>");
        println!("and open this in your browser there:");
        println!("  {url}");
    }
    println!(
        "The address works only on this machine and only while this command runs (Ctrl+C \
         stops it; it also stops by itself after 30 minutes without a click)."
    );
    Ok(server.serve(IDLE))
}

pub(super) fn launch_browser(url: &str) {
    let spawned = browser_command(std::env::consts::OS, url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Reaped on the side, so no zombie is left while the page runs.
            std::thread::spawn(move || child.wait());
        }
        Err(error) => println!("(The browser could not be opened: {error}.)"),
    }
}
