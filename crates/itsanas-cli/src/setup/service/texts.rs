//! The texts each platform is given -- the task, the wrapper, the unit, the
//! plist, the shortcut, the autostart entry -- as pure functions, so the tests
//! read them without installing anything.

use std::path::Path;

use super::Names;

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
///
/// Two faults of the 2026-10-22 hand test are answered here. The log showed
/// `nÔÇÖa pu ├¬tre` and a `NativeCommandError` block around every line the
/// daemon wrote to stderr: PowerShell decodes a native program's output with
/// the console's OEM code page, and wraps each stderr line in an
/// `ErrorRecord` when it is redirected. So the console encoding is UTF-8 (the
/// daemon writes UTF-8) and each line is turned back into its text. And
/// `update` saw a daemon stop and not come back in a minute: exit
/// [`RESTART_EXIT`](crate::lifecycle::RESTART_EXIT) is a request, not a
/// failure, so it restarts at once, and the pause after a failure grows
/// from 10 s only while the daemon keeps failing quickly. At once at most
/// three times in 60 s (red team, 2026-10-23): a daemon that exits 75 at
/// every start would otherwise restart every second for ever, so past that
/// 75 takes the failure path, its pause and its doubling, and says so.
pub(crate) fn windows_wrapper(names: &Names) -> String {
    [
        "# Started by the ITSaNAS scheduled task at logon (written by `itsanas setup`, the",
        "# same wrapper as install/provision.ps1). A daemon cannot be prompted, so the",
        "# passphrase comes from a file only this account can read.",
        &format!("$instanceSuffix = {}", ps_quote(&names.suffix)),
        "if ($instanceSuffix) { $env:ITSANAS_HOME = \"$env:USERPROFILE\\.itsanas$instanceSuffix\" }",
        "$passphraseFile = \"$env:LOCALAPPDATA\\itsanas\\passphrase$instanceSuffix.txt\"",
        "if (Test-Path -LiteralPath $passphraseFile) { $env:ITSANAS_PASSPHRASE = Get-Content -LiteralPath $passphraseFile -Raw }",
        "$log = \"$env:LOCALAPPDATA\\itsanas\\daemon$instanceSuffix.log\"",
        "if ((Test-Path $log) -and ((Get-Item $log).Length -gt 5MB)) {",
        "    Move-Item -LiteralPath $log -Destination \"$log.1\" -Force",
        "}",
        "# The daemon writes UTF-8. PowerShell decodes it with the console's code page,",
        "# the OEM one unless told otherwise, and the log read as mojibake.",
        "try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }",
        "$OutputEncoding = [System.Text.Encoding]::UTF8",
        "# Restarts live here, not in the task: conhost --headless reports exit 0",
        "# whatever happened, so the scheduler's restart-on-failure never fires.",
        "# Exit 0 is a stop; 75 a restart asked for (itsanas update), done at once;",
        "# anything else a failure, restarted after a pause that grows to five",
        "# minutes while the daemon keeps failing within ten minutes of starting.",
        "# At most 3 immediate restarts in 60 s: past that, 75 is a failure too.",
        "$pause = 10",
        "$asked = @()",
        "while ($true) {",
        "    $started = Get-Date",
        "    # 2>&1 with each line turned back into its text: redirected, a native",
        "    # program's stderr line is an ErrorRecord, which Out-File writes as a",
        "    # NativeCommandError block.",
        "    & \"$env:LOCALAPPDATA\\Programs\\itsanas\\bin\\itsanas.exe\" daemon 2>&1 |",
        "        ForEach-Object { \"$_\" } |",
        "        Out-File -LiteralPath $log -Encoding utf8 -Append",
        "    $code = $LASTEXITCODE",
        "    if ($code -eq 0) { break }",
        "    if ($code -eq 75) {",
        "        $now = Get-Date",
        "        $asked = @($asked | Where-Object { ($now - $_).TotalSeconds -lt 60 })",
        "        if ($asked.Count -lt 3) {",
        "            $asked += $now",
        "            'daemon asked to restart (exit 75); restarting now' |",
        "                Out-File -LiteralPath $log -Encoding utf8 -Append",
        "            $pause = 10",
        "            Start-Sleep -Seconds 1",
        "            continue",
        "        }",
        "        'daemon asked to restart (exit 75) more than 3 times in 60 s; pausing as after a failure' |",
        "            Out-File -LiteralPath $log -Encoding utf8 -Append",
        "    }",
        "    if (((Get-Date) - $started).TotalMinutes -ge 10) { $pause = 10 }",
        "    \"daemon exited with $code; restarting in $pause s\" |",
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

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::super::{TRAY_PS1, run_powershell};
    use super::*;

    /// The exit-75 branch of the wrapper, `lines[restart..failure]`, restarts
    /// at once at most three times in 60 s (red team, 2026-10-23): a daemon
    /// exiting 75 at every start restarted every second for ever. Past the
    /// limit, 75 falls through to the failure path's pause, and says so.
    /// Sabotage: drop the count, widen it, or `continue` past the limit.
    fn restarts_at_once_at_most_three_times_a_minute(
        lines: &[&str],
        restart: usize,
        failure: usize,
    ) {
        let at = |needle: &str| {
            lines
                .iter()
                .position(|line| line.starts_with(needle))
                .unwrap_or_else(|| panic!("no line starts with {needle:?}: {lines:?}"))
        };
        let branch = &lines[restart..failure];
        let window = at("$asked = @($asked | Where-Object { ($now - $_).TotalSeconds -lt 60 })");
        let limit = at("if ($asked.Count -lt 3) {");
        let over = at(
            "'daemon asked to restart (exit 75) more than 3 times in 60 s; pausing as after a failure' |",
        );
        assert!(
            restart < window && window < limit && limit < over && over < failure,
            "the limit on immediate restarts is not inside the exit-75 branch: {branch:?}"
        );
        assert_eq!(
            lines[limit + 1],
            "$asked += $now",
            "an immediate restart is not counted"
        );
        let continues = branch.iter().position(|line| *line == "continue");
        assert!(
            continues.is_some_and(|at| restart + at < over)
                && !lines[over..failure].contains(&"continue"),
            "past the limit, 75 still restarts at once: {branch:?}"
        );
        assert!(
            at("$asked = @()") < at("while ($true) {"),
            "the restarts counted are not kept across the loop"
        );
    }

    /// The daemon log on Windows read `nÔÇÖa pu ├¬tre`, with a
    /// `NativeCommandError` block around each stderr line, and `update` saw a
    /// daemon asked to restart stay down past its minute (hand test,
    /// 2026-10-22). Sabotage: put back `*>&1` without the `ForEach-Object`,
    /// drop the encoding line, or the exit-75 branch -- each goes red.
    #[test]
    fn red_team_the_windows_wrapper_logs_plain_utf8_and_restarts_a_requested_restart_at_once() {
        let script = windows_wrapper(&Names::of(Some("test")));
        let lines: Vec<&str> = script.lines().map(str::trim).collect();
        let at = |needle: &str| {
            lines
                .iter()
                .position(|line| line.starts_with(needle))
                .unwrap_or_else(|| panic!("no line starts with {needle:?}:\n{script}"))
        };
        assert!(
            script.is_ascii(),
            "written without a BOM, so Windows PowerShell reads it as ANSI"
        );
        let daemon = at("& \"$env:LOCALAPPDATA\\Programs\\itsanas\\bin\\itsanas.exe\" daemon");
        assert!(
            at("try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }") < daemon,
            "the console decodes the daemon's UTF-8 as UTF-8 before it runs"
        );
        assert!(at("$OutputEncoding = [System.Text.Encoding]::UTF8") < daemon);
        assert!(
            lines[daemon].ends_with("daemon 2>&1 |") && !script.contains("*>&1"),
            "stderr is merged as lines of text, not every stream as records: {}",
            lines[daemon]
        );
        assert_eq!(
            lines[daemon + 1],
            "ForEach-Object { \"$_\" } |",
            "each ErrorRecord is written as the line the daemon wrote"
        );
        assert_eq!(
            lines[daemon + 2],
            "Out-File -LiteralPath $log -Encoding utf8 -Append"
        );
        let code = at("$code = $LASTEXITCODE");
        assert!(code > daemon);
        assert_eq!(lines[code + 1], "if ($code -eq 0) { break }", "0 stops");
        let restart = at("if ($code -eq 75) {");
        let failure = at("\"daemon exited with $code; restarting in $pause s\" |");
        assert!(restart < failure, "75 is decided before the failure path");
        let branch = &lines[restart..failure];
        assert!(
            branch.contains(&"continue") && branch.contains(&"$pause = 10"),
            "75 restarts at once, with the failure pause reset: {branch:?}"
        );
        assert!(
            !branch.iter().any(|line| line.contains("-Seconds $pause")),
            "75 never waits the failure pause: {branch:?}"
        );
        assert!(
            lines[failure..].contains(&"Start-Sleep -Seconds $pause"),
            "any other code restarts after the pause"
        );
        // At most three immediate restarts in 60 s (red team, 2026-10-23).
        restarts_at_once_at_most_three_times_a_minute(&lines, restart, failure);
        assert_eq!(
            lines
                .iter()
                .filter(|line| **line == "while ($true) {")
                .count(),
            1,
            "the restart loop is kept"
        );

        // provision.ps1 writes the same wrapper from single-quoted lines: the
        // code from the encoding on must be the same, or one install keeps
        // the mojibake the other lost.
        let code_from_encoding = |lines: Vec<String>| -> Vec<String> {
            let lines: Vec<String> = lines
                .into_iter()
                .map(|line| line.trim().to_owned())
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect();
            let start = lines
                .iter()
                .position(|line| line.starts_with("try { [Console]::OutputEncoding"))
                .expect("the encoding line");
            lines[start..].to_vec()
        };
        let provision = include_str!("../../../../../install/provision.ps1");
        let begin = provision
            .find("$wrapper = Join-Path $secretDir")
            .expect("provision.ps1 writes the wrapper");
        let written: Vec<String> = provision[begin..]
            .lines()
            .take_while(|line| !line.contains("Set-Content -LiteralPath $wrapper"))
            .filter_map(|line| {
                let line = line.trim().trim_end_matches(',');
                line.strip_prefix('\'')
                    .and_then(|line| line.strip_suffix('\''))
                    .map(|line| line.replace("''", "'"))
            })
            .collect();
        assert_eq!(
            code_from_encoding(written),
            code_from_encoding(script.lines().map(str::to_owned).collect()),
            "install/provision.ps1 and `itsanas setup` write different wrappers"
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
                (
                    "window",
                    super::super::super::secrets::WINFORMS_SCRIPT.to_owned(),
                ),
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
