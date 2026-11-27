//! Where a person sees the 24 words and types the passphrase.
//!
//! Never a web page and never a log: HANDOVER §8 0w (4) settled that the words
//! do not go through a browser, where every extension allowed to read all sites
//! reads them. So each platform gets the window it already has -- Windows
//! Forms through PowerShell, `display dialog` through `osascript` on a Mac,
//! `zenity` or `kdialog` on a Linux desktop -- and a terminal when there is no
//! desktop, which is what `rpassword` already did for `init` and `login`.
//!
//! # The one rule every backend keeps
//!
//! A secret never reaches another process's argv or environment: both are
//! readable by every process of the same user (`ps -E`, `/proc/PID/environ`,
//! Process Explorer). It goes in on standard input and comes back on standard
//! output, through pipes nothing else can open. [`plan`] builds every command
//! a backend runs as data, so `red_team_no_secret_window_carries_a_secret_in_argv_or_env`
//! can read each argv and environment before anything starts.
//!
//! On Windows the script also carries no secret, and that is a second rule,
//! not a detail: PowerShell writes "suspicious" script blocks to the event log
//! whatever the policy says (event 4104), and a script that decodes Base64
//! qualifies. The script text is constant; the words arrive as data the script
//! reads, which script-block logging does not record.

use std::{
    io::{IsTerminal as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use crate::error::{CliError, Result};

/// A secret in memory, wiped when dropped.
pub type Secret = Zeroizing<String>;

/// How many words a person types back before the account is written.
///
/// Three, as Nicolas's wizard specification asks: enough that a person who
/// clicked through without writing anything down is caught, few enough that
/// one who did write them is not annoyed into skipping the step.
pub(crate) const CONFIRM_WORDS: usize = 3;

/// What a person is asked for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ask<'a> {
    /// Show the 24 words, then ask back the words at these positions (from 1).
    ShowAndConfirm {
        phrase: &'a str,
        positions: [usize; CONFIRM_WORDS],
    },
    /// A new passphrase, typed twice, hidden.
    NewPassphrase,
    /// An existing passphrase, once, hidden; `purpose` says which one.
    Passphrase { purpose: &'a str },
    /// The 24 words of an existing account, hidden.
    RecoveryPhrase,
}

/// Where the questions are put. The engine knows nothing else about it.
pub trait SecretPrompt {
    /// Show the words and return what was typed at `positions`, or `None`
    /// when nobody can be asked (an unattended run).
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        positions: [usize; CONFIRM_WORDS],
    ) -> Result<Option<Vec<Secret>>>;
    /// A new passphrase, already checked typed twice the same and not empty.
    fn new_passphrase(&mut self) -> Result<Secret>;
    /// An existing passphrase.
    fn passphrase(&mut self, purpose: &str) -> Result<Secret>;
    /// Show the 24 words of an account that already exists, for someone who
    /// wants to read them again. Nothing is asked back that matters: a window
    /// that wants three words typed is satisfied by whatever is typed, because
    /// the person is reading, not proving.
    ///
    /// The default goes through [`SecretPrompt::show_and_confirm`], which every
    /// prompt has; a prompt with a plainer way overrides it.
    fn show_words(&mut self, phrase: &str) -> Result<()> {
        self.show_and_confirm(phrase, random_positions()?)?
            .map(|_| ())
            .ok_or_else(|| CliError::Usage("nobody can be asked to read the words here".to_owned()))
    }
    /// The 24 words of an existing account.
    fn recovery_phrase(&mut self) -> Result<Secret>;
    /// What the person will see, for the progress line ("a window", "this
    /// terminal").
    fn where_asked(&self) -> &'static str;
}

/// Which window a platform has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// Windows Forms, through `powershell.exe`, which every Windows has.
    WinForms,
    /// `display dialog`, through `osascript`, which every Mac has.
    AppleScript,
    /// GNOME's dialogs, on a Linux desktop that has them.
    Zenity,
    /// KDE's dialogs, on a Linux desktop that has them.
    Kdialog,
    /// Hidden prompts in the terminal setup was started from.
    Terminal,
}

/// Choose where to ask, for this machine as it is now.
///
/// A window wherever there is a desktop to put it on, the terminal otherwise.
/// A session over SSH has a terminal and no desktop of its own -- a window
/// opened there appears on a screen nobody is looking at, or hangs -- so it
/// gets the terminal even on Windows and macOS.
pub(crate) fn choose() -> Result<Backend> {
    let over_ssh =
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some();
    let terminal = std::io::stdin().is_terminal();
    let display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    choose_for(
        std::env::consts::OS,
        over_ssh,
        terminal,
        display,
        &|program| on_path(program),
    )
}

/// [`choose`], with the machine's facts passed in, so the rule is testable.
fn choose_for(
    os: &str,
    over_ssh: bool,
    terminal: bool,
    display: bool,
    found: &dyn Fn(&str) -> bool,
) -> Result<Backend> {
    if over_ssh && terminal {
        return Ok(Backend::Terminal);
    }
    match os {
        "windows" => return Ok(Backend::WinForms),
        "macos" => return Ok(Backend::AppleScript),
        _ => {}
    }
    if display && found("zenity") {
        return Ok(Backend::Zenity);
    }
    if display && found("kdialog") {
        return Ok(Backend::Kdialog);
    }
    if terminal {
        return Ok(Backend::Terminal);
    }
    Err(CliError::Usage(
        "nowhere to ask for the passphrase: no terminal, and no desktop with zenity or \
         kdialog. Install zenity (apt install zenity), run this from a terminal, or give \
         the answers in a file with `itsanas setup --answers FILE` and the passphrase in \
         ITSANAS_PASSPHRASE."
            .to_owned(),
    ))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Three distinct positions among the 24, in order, drawn at random.
pub(crate) fn random_positions() -> Result<[usize; CONFIRM_WORDS]> {
    let mut chosen: Vec<usize> = Vec::with_capacity(CONFIRM_WORDS);
    while chosen.len() < CONFIRM_WORDS {
        let mut byte = [0u8; 1];
        getrandom::fill(&mut byte).map_err(|error| {
            CliError::Usage(format!("this machine gave no randomness: {error}"))
        })?;
        // Rejection sampling: 240 is the largest multiple of 24 below 256, so
        // every position is equally likely.
        if byte[0] >= 240 {
            continue;
        }
        let position = usize::from(byte[0] % 24) + 1;
        if !chosen.contains(&position) {
            chosen.push(position);
        }
    }
    chosen.sort_unstable();
    Ok([chosen[0], chosen[1], chosen[2]])
}

/// Whether what was typed is the words at those positions.
///
/// Case and surrounding spaces do not count -- a capital from a phone's
/// keyboard is not a wrong word -- and anything else does: a word typed from
/// a bad copy is exactly what this step exists to catch, before the account
/// that depends on the copy is written.
pub(crate) fn words_match(phrase: &str, positions: &[usize], typed: &[Secret]) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    positions.len() == typed.len()
        && positions.iter().zip(typed).all(|(&position, typed)| {
            position >= 1
                && words
                    .get(position - 1)
                    .is_some_and(|word| word.eq_ignore_ascii_case(typed.trim()))
        })
}

/// The 24 words, numbered, four to a line: the grid `init` prints.
fn grid(phrase: &str) -> String {
    crate::phrase_grid(phrase)
}

/// What every window says above the words.
pub(crate) const WORDS_INSTRUCTION: &str = "Keep these 24 words somewhere safe: a password manager \
     is better than paper, and copying them is allowed. Anyone with them can read your files. \
     ITSaNAS will never ask for them in a web page.";

// ---------------------------------------------------------------------------
// Commands, built as data
// ---------------------------------------------------------------------------

/// One program to run, and what to feed it.
#[derive(Debug)]
pub(crate) struct Prepared {
    /// The program, its arguments and environment: never a secret.
    pub(crate) command: Command,
    /// What goes on its standard input, which may be one.
    pub(crate) stdin: Secret,
}

impl Prepared {
    fn new(program: &str, args: &[&str], stdin: String) -> Self {
        let mut command = Command::new(program);
        command.args(args);
        Self {
            command,
            stdin: Zeroizing::new(stdin),
        }
    }
}

/// Every program a backend runs to put `ask`, in order.
///
/// Empty for the terminal, which runs nothing.
pub(crate) fn plan(backend: Backend, ask: Ask<'_>) -> Vec<Prepared> {
    match backend {
        Backend::WinForms => vec![winforms(ask)],
        Backend::AppleScript => vec![applescript(ask)],
        Backend::Zenity => zenity(ask),
        Backend::Kdialog => kdialog(ask),
        Backend::Terminal => Vec::new(),
    }
}

/// The PowerShell that reads the real script from standard input and runs it.
///
/// On the command line, because `-Command -` reads *all* of standard input as
/// commands before anything runs (measured on Windows 11, PowerShell 5.1):
/// a secret sent after the script there is executed as a command, and echoed
/// back in the error that it is not one. This line is constant and carries
/// nothing; the script and the data follow on standard input.
pub(crate) const POWERSHELL_BOOTSTRAP: &str = "$i=[Console]::In; \
     $s=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($i.ReadLine())); \
     & ([scriptblock]::Create($s)) $i";

/// The window script, constant: see the module's note on script-block logging.
pub(crate) const WINFORMS_SCRIPT: &str = include_str!("secret-window.ps1");

/// `powershell.exe` running [`WINFORMS_SCRIPT`] with `lines` as its data.
pub(crate) fn powershell(script: &str, lines: &[&str]) -> Prepared {
    let mut stdin = base64(script.as_bytes());
    for line in lines {
        stdin.push('\n');
        stdin.push_str(line);
    }
    stdin.push('\n');
    Prepared::new(
        "powershell.exe",
        &[
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            POWERSHELL_BOOTSTRAP,
        ],
        stdin,
    )
}

fn winforms(ask: Ask<'_>) -> Prepared {
    let (mode, extra, secret) = match ask {
        Ask::ShowAndConfirm { phrase, positions } => (
            "show",
            positions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            base64(phrase.as_bytes()),
        ),
        Ask::NewPassphrase => ("new-passphrase", String::new(), String::new()),
        Ask::Passphrase { purpose } => ("passphrase", base64(purpose.as_bytes()), String::new()),
        Ask::RecoveryPhrase => ("phrase", String::new(), String::new()),
    };
    // Empty lines are read as empty: `ReadLine` returns "" for them.
    powershell(WINFORMS_SCRIPT, &[mode, &extra, &secret])
}

/// An `AppleScript` string literal.
fn applescript_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\" & linefeed & \""),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// One hidden-answer dialog, as an `AppleScript` expression for its text.
fn hidden_question(title: &str, prompt: &str) -> String {
    format!(
        "text returned of (display dialog {} default answer \"\" with hidden answer \
         with title {} with icon caution buttons {{\"Cancel\", \"OK\"}} default button \"OK\")",
        applescript_string(prompt),
        applescript_string(title)
    )
}

/// What the Mac's words window says after Copy, and after Save.
const MAC_COPIED: &str = "Copied. Paste it into your password manager now: the clipboard is emptied when this check ends, and any program running as you can read it until then.";
const MAC_SAVED: &str = "Saved. Move it to a USB stick or a password manager: on this disk, anyone who takes the machine has your files.";

/// The clipboard emptied once the check ends, if it still holds the words:
/// something copied since is the person's own and stays.
const MAC_CLIPBOARD_EMPTIED: &str = "if copied then\ntry\nif (the clipboard as text) is phrase24 then set the clipboard to \"\"\nend try\nend if";

/// The Mac's words window, with Copy and Save as on Windows (Nicolas,
/// 2026-10-07: forced paper loses more data than it protects). It loops until
/// "I have written them down"; the words stay in the script, which is read
/// from standard input, and are written by `AppleScript` itself, never through
/// a shell, whose command line every program of this user can read.
fn mac_words_window(phrase: &str) -> String {
    let title = "\"ITSaNAS - your recovery words\"";
    format!(
        "set phrase24 to {}\nset copied to false\nrepeat\n\
         set choice to button returned of (display dialog {} with title {title} with icon caution \
         buttons {{\"Copy\", \"Save to a file...\", \"I have written them down\"}} default button 3)\n\
         if choice is \"Copy\" then\nset the clipboard to phrase24\nset copied to true\n\
         display dialog {} with title {title} buttons {{\"OK\"}} default button 1\n\
         else if choice is \"Save to a file...\" then\ntry\n\
         set target to choose file name with prompt \"Where to save your recovery words\" default name \"ITSaNAS recovery words.txt\"\n\
         set f to open for access target with write permission\nset eof f to 0\n\
         write phrase24 to f as \u{ab}class utf8\u{bb}\nclose access f\n\
         display dialog {} with title {title} buttons {{\"OK\"}} default button 1\n\
         end try\nelse\nexit repeat\nend if\nend repeat",
        applescript_string(phrase),
        applescript_string(&format!("{WORDS_INSTRUCTION}\n\n{}", grid(phrase))),
        applescript_string(MAC_COPIED),
        applescript_string(MAC_SAVED),
    )
}

fn applescript(ask: Ask<'_>) -> Prepared {
    // The script goes on standard input (`osascript -`), so the words in it
    // never reach an argv. AppleScript has no channel for data apart from its
    // script; this is the Mac's equivalent of the Windows rule, and it holds
    // because nothing on macOS logs an `osascript` script.
    let body = match ask {
        Ask::ShowAndConfirm { phrase, positions } => {
            let title = "ITSaNAS - check your recovery words";
            let asks: Vec<String> = positions
                .iter()
                .map(|position| {
                    hidden_question(
                        title,
                        &format!("Word number {position}, from where you kept them:"),
                    )
                })
                .collect();
            format!(
                "{}\nset a to {}\nset b to {}\nset c to {}\n{}\nreturn a & linefeed & b & linefeed & c",
                mac_words_window(phrase),
                asks[0],
                asks[1],
                asks[2],
                MAC_CLIPBOARD_EMPTIED
            )
        }
        Ask::NewPassphrase => {
            let title = "ITSaNAS - choose a passphrase";
            format!(
                "repeat\nset p to {}\nset q to {}\nconsidering case\n\
                 if p is q and p is not \"\" then exit repeat\nend considering\n\
                 display dialog \"The two did not match, or were empty. Try again.\" \
                 with title {} buttons {{\"Try again\"}} default button 1\nend repeat\nreturn p",
                hidden_question(
                    title,
                    "A passphrase for this machine's keys. Choose a long one, and write it down."
                ),
                hidden_question(title, "The same passphrase again:"),
                applescript_string(title)
            )
        }
        Ask::Passphrase { purpose } => {
            format!(
                "return {}",
                hidden_question("ITSaNAS - your passphrase", purpose)
            )
        }
        Ask::RecoveryPhrase => format!(
            "return {}",
            hidden_question(
                "ITSaNAS - enter your recovery words",
                "Your 24 recovery words, in order, separated by spaces:"
            )
        ),
    };
    Prepared::new("osascript", &["-"], format!("activate\n{body}\n"))
}

/// zenity's extra buttons on the words window; a click exits non-zero and
/// prints the label, which [`Native`] reads to copy or save and show again.
const ZENITY_COPY: &str = "Copy";
const ZENITY_SAVE: &str = "Save to a file...";

/// A Linux desktop's clipboard, as commands: one that takes the text on its
/// standard input, never in its argv, and one that prints it back.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Clipboard {
    copy: &'static [&'static str],
    paste: &'static [&'static str],
}

/// The clipboard tool of this desktop: `wl-copy` under Wayland, else `xclip`
/// or `xsel`; `None` when none is installed.
fn linux_clipboard(wayland: bool, found: &dyn Fn(&str) -> bool) -> Option<Clipboard> {
    if wayland && found("wl-copy") {
        return Some(Clipboard {
            copy: &["wl-copy"],
            paste: &["wl-paste", "-n"],
        });
    }
    if found("xclip") {
        return Some(Clipboard {
            copy: &["xclip", "-selection", "clipboard"],
            paste: &["xclip", "-selection", "clipboard", "-o"],
        });
    }
    found("xsel").then_some(Clipboard {
        copy: &["xsel", "--clipboard", "--input"],
        paste: &["xsel", "--clipboard", "--output"],
    })
}

/// Run `argv` with `input` on its standard input; its output, if it ran.
fn piped(command: &[&str], input: &str) -> Option<Secret> {
    let (program, args) = command.split_first()?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    let text = Zeroizing::new(String::from_utf8_lossy(&output.stdout).into_owned());
    output.status.success().then_some(text)
}

/// A message in a zenity window, waited for.
fn zenity_says(text: &str) {
    let _ = Command::new("zenity")
        .args([
            "--info",
            "--title",
            "ITSaNAS - your recovery words",
            "--text",
            text,
        ])
        .stderr(Stdio::null())
        .status();
}

fn this_clipboard() -> Option<Clipboard> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
    linux_clipboard(wayland, &|program| on_path(program))
}

/// Copy the words for a password manager, as the Windows window does.
fn linux_copy(phrase: &str) -> bool {
    let copied = this_clipboard()
        .and_then(|tool| piped(tool.copy, phrase))
        .is_some();
    zenity_says(if copied {
        "Copied. Paste it into your password manager now: the clipboard is emptied when this check ends, and any program running as you can read it until then."
    } else {
        "Nothing was copied: install wl-clipboard, xclip or xsel, or save the words to a file."
    });
    copied
}

/// Save the words where the person says, readable by them alone.
fn linux_save(phrase: &str) {
    let Some(path) = piped(
        &[
            "zenity",
            "--file-selection",
            "--save",
            "--confirm-overwrite",
            "--filename",
            "ITSaNAS recovery words.txt",
        ],
        "",
    ) else {
        return;
    };
    let saved = save_words(&PathBuf::from(path.trim()), phrase);
    zenity_says(if saved.is_ok() {
        "Saved. Move it to a USB stick or a password manager: on this disk, anyone who takes the machine has your files."
    } else {
        "The file could not be written; choose another place."
    });
}

/// The words written to `path`, readable by their owner alone, even when
/// `path` is an existing file (zenity confirms an overwrite): opening keeps an
/// existing file's mode, so it is narrowed before a word is written.
fn save_words(path: &Path, phrase: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    )?;
    file.write_all(phrase.as_bytes())
}

/// Once the check ends: the clipboard emptied if it still holds the words;
/// something copied since is the person's own and stays.
fn linux_clipboard_emptied(phrase: &str) {
    if let Some(tool) = this_clipboard()
        && piped(tool.paste, "").is_some_and(|held| held.trim() == phrase.trim())
    {
        let _ = piped(tool.copy, "");
    }
}

fn zenity(ask: Ask<'_>) -> Vec<Prepared> {
    let hidden = |title: &str, text: &str| {
        Prepared::new(
            "zenity",
            &["--entry", "--hide-text", "--title", title, "--text", text],
            String::new(),
        )
    };
    match ask {
        Ask::ShowAndConfirm { phrase, positions } => {
            let mut steps = vec![Prepared::new(
                "zenity",
                &[
                    "--text-info",
                    "--title",
                    "ITSaNAS - your recovery words",
                    "--width",
                    "640",
                    "--height",
                    "420",
                    "--ok-label",
                    "I have written them down",
                    "--extra-button",
                    ZENITY_COPY,
                    "--extra-button",
                    ZENITY_SAVE,
                ],
                format!("{WORDS_INSTRUCTION}\n\n{}", grid(phrase)),
            )];
            for position in positions {
                steps.push(hidden(
                    "ITSaNAS - check your recovery words",
                    &format!("Word number {position}, from where you kept them:"),
                ));
            }
            steps
        }
        Ask::NewPassphrase => vec![
            Prepared::new(
                "zenity",
                &["--password", "--title", "ITSaNAS - choose a passphrase"],
                String::new(),
            ),
            hidden(
                "ITSaNAS - choose a passphrase",
                "The same passphrase again:",
            ),
        ],
        Ask::Passphrase { purpose } => vec![hidden("ITSaNAS - your passphrase", purpose)],
        Ask::RecoveryPhrase => vec![hidden(
            "ITSaNAS - enter your recovery words",
            "Your 24 recovery words, in order, separated by spaces:",
        )],
    }
}

fn kdialog(ask: Ask<'_>) -> Vec<Prepared> {
    let hidden = |title: &str, text: &str| {
        Prepared::new(
            "kdialog",
            &["--title", title, "--password", text],
            String::new(),
        )
    };
    match ask {
        Ask::ShowAndConfirm { phrase, positions } => {
            // `--textbox` takes a file; /dev/stdin is the one that is not on
            // a disk.
            let mut steps = vec![Prepared::new(
                "kdialog",
                &[
                    "--title",
                    "ITSaNAS - your recovery words",
                    "--textbox",
                    "/dev/stdin",
                    "640",
                    "420",
                ],
                format!("{WORDS_INSTRUCTION}\n\n{}", grid(phrase)),
            )];
            for position in positions {
                steps.push(hidden(
                    "ITSaNAS - check your recovery words",
                    &format!("Word number {position}, from where you kept them:"),
                ));
            }
            steps
        }
        Ask::NewPassphrase => vec![
            hidden(
                "ITSaNAS - choose a passphrase",
                "A passphrase for this machine's keys. Choose a long one, and write it down.",
            ),
            hidden(
                "ITSaNAS - choose a passphrase",
                "The same passphrase again:",
            ),
        ],
        Ask::Passphrase { purpose } => vec![hidden("ITSaNAS - your passphrase", purpose)],
        Ask::RecoveryPhrase => vec![hidden(
            "ITSaNAS - enter your recovery words",
            "Your 24 recovery words, in order, separated by spaces:",
        )],
    }
}

/// Standard Base64, for what PowerShell's `FromBase64String` reads.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (index, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// What the Windows script writes back: hexadecimal UTF-8, one value a line.
///
/// Hexadecimal because a redirected PowerShell writes in the console's code
/// page, which turns an accented passphrase into a different passphrase;
/// ASCII survives every code page.
fn unhex(line: &str) -> Option<Secret> {
    let line = line.trim();
    if !line.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..line.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(line.get(at..at + 2)?, 16).ok())
        .collect();
    let bytes = Zeroizing::new(bytes?);
    String::from_utf8(bytes.to_vec()).ok().map(Zeroizing::new)
}

// ---------------------------------------------------------------------------
// Running them
// ---------------------------------------------------------------------------

/// The window backends, and the terminal.
#[derive(Debug)]
pub(crate) struct Native {
    backend: Backend,
}

impl Native {
    pub(crate) const fn new(backend: Backend) -> Self {
        Self { backend }
    }

    /// Run every program `ask` needs, and return what each wrote, in order.
    fn run(&self, ask: Ask<'_>) -> Result<Vec<Secret>> {
        let mut replies = Vec::new();
        for mut step in plan(self.backend, ask) {
            let (answered, text) = self.run_one(&mut step)?;
            if !answered {
                return Err(closed());
            }
            replies.push(text);
        }
        Ok(replies)
    }

    /// zenity's words window, shown again after each Copy or Save, then the
    /// three words asked back; whether the words were copied.
    fn zenity_show_and_confirm(
        &self,
        phrase: &str,
        positions: [usize; CONFIRM_WORDS],
    ) -> Result<(Vec<Secret>, bool)> {
        let mut steps = plan(self.backend, Ask::ShowAndConfirm { phrase, positions }).into_iter();
        let mut words = steps.next().ok_or_else(closed)?;
        let mut copied = false;
        loop {
            let (answered, said) = self.run_one(&mut words)?;
            if answered {
                break;
            }
            match said.trim() {
                ZENITY_COPY => copied |= linux_copy(phrase),
                ZENITY_SAVE => linux_save(phrase),
                _ => return Err(closed()),
            }
        }
        let mut replies = Vec::new();
        for mut step in steps {
            let (answered, text) = self.run_one(&mut step)?;
            if !answered {
                return Err(closed());
            }
            replies.push(text);
        }
        Ok((replies, copied))
    }

    /// Run one program; whether it answered, and what it wrote.
    fn run_one(&self, step: &mut Prepared) -> Result<(bool, Secret)> {
        {
            let mut child = step
                .command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                // Not kept: a script error can quote the line it failed on,
                // and nothing here is worth the risk of that line being one
                // that held a value.
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| {
                    CliError::Usage(format!(
                        "could not open the {:?} window ({error}); run `itsanas setup --text` \
                         from a terminal instead",
                        self.backend
                    ))
                })?;
            if let Some(mut input) = child.stdin.take() {
                // A window that closed early leaves a broken pipe, which the
                // exit status below reports better than this would.
                let _ = input.write_all(step.stdin.as_bytes());
            }
            let output = child.wait_with_output().map_err(|error| CliError::Io {
                path: PathBuf::from("<secret window>"),
                source: error,
            })?;
            let stdout = Zeroizing::new(output.stdout);
            let text = Zeroizing::new(String::from_utf8_lossy(&stdout).into_owned());
            Ok((output.status.success(), text))
        }
    }

    /// The lines one reply holds, decoded where the backend encodes them.
    fn lines(&self, reply: &str) -> Result<Vec<Secret>> {
        let lines: Vec<&str> = reply
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .collect();
        if self.backend == Backend::WinForms {
            lines
                .iter()
                .filter(|line| !line.is_empty())
                .map(|line| {
                    unhex(line).ok_or_else(|| {
                        CliError::Usage("the window returned something unreadable".to_owned())
                    })
                })
                .collect()
        } else {
            Ok(lines
                .iter()
                .map(|line| Zeroizing::new((*line).to_owned()))
                .collect())
        }
    }

    fn one(&self, ask: Ask<'_>) -> Result<Secret> {
        let replies = self.run(ask)?;
        let mut all = Vec::new();
        for reply in &replies {
            all.extend(self.lines(reply)?);
        }
        all.into_iter()
            .next()
            .ok_or_else(|| CliError::Usage("the window returned nothing".to_owned()))
    }
}

fn closed() -> CliError {
    CliError::Usage(
        "the window was closed or cancelled; nothing was saved. Run setup again when you are ready"
            .to_owned(),
    )
}

fn not_empty(secret: Secret, what: &str) -> Result<Secret> {
    if secret.trim().is_empty() {
        return Err(CliError::Usage(format!("an empty {what} protects nothing")));
    }
    Ok(secret)
}

fn terminal_line(prompt: &str) -> Result<Secret> {
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|error| CliError::Io {
            path: PathBuf::from("<terminal>"),
            source: error,
        })
}

impl SecretPrompt for Native {
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        positions: [usize; CONFIRM_WORDS],
    ) -> Result<Option<Vec<Secret>>> {
        if self.backend == Backend::Terminal {
            return terminal_show_and_confirm(phrase, positions).map(Some);
        }
        let replies = if self.backend == Backend::Zenity {
            let (replies, copied) = self.zenity_show_and_confirm(phrase, positions)?;
            if copied {
                linux_clipboard_emptied(phrase);
            }
            replies
        } else {
            self.run(Ask::ShowAndConfirm { phrase, positions })?
        };
        let mut typed = Vec::new();
        for reply in &replies {
            typed.extend(self.lines(reply)?);
        }
        // zenity and kdialog answer the grid's window with nothing, then one
        // word a window; the Mac and Windows answer all three in one.
        typed.retain(|word| !word.trim().is_empty());
        Ok(Some(typed))
    }

    fn new_passphrase(&mut self) -> Result<Secret> {
        if self.backend == Backend::Terminal {
            let first = terminal_line("Choose a passphrase for this machine's keys: ")?;
            let again = terminal_line("The same passphrase again: ")?;
            if *first != *again {
                return Err(CliError::Usage("the passphrases did not match".to_owned()));
            }
            return not_empty(first, "passphrase");
        }
        let replies = self.run(Ask::NewPassphrase)?;
        let mut values = Vec::new();
        for reply in &replies {
            values.extend(self.lines(reply)?);
        }
        // Windows and the Mac compare in the window, and return one value;
        // zenity and kdialog ask twice and the comparison is here.
        if values.len() == 2 && *values[0] != *values[1] {
            return Err(CliError::Usage("the passphrases did not match".to_owned()));
        }
        let first = values
            .into_iter()
            .next()
            .ok_or_else(|| CliError::Usage("the window returned nothing".to_owned()))?;
        not_empty(first, "passphrase")
    }

    fn passphrase(&mut self, purpose: &str) -> Result<Secret> {
        if self.backend == Backend::Terminal {
            println!("{purpose}");
            return not_empty(terminal_line("Passphrase: ")?, "passphrase");
        }
        not_empty(self.one(Ask::Passphrase { purpose })?, "passphrase")
    }

    fn recovery_phrase(&mut self) -> Result<Secret> {
        if self.backend == Backend::Terminal {
            return terminal_line("Recovery phrase (24 words): ");
        }
        self.one(Ask::RecoveryPhrase)
    }

    fn show_words(&mut self, phrase: &str) -> Result<()> {
        if self.backend == Backend::Terminal {
            return terminal_show_words(phrase);
        }
        // The window that shows the words is the one that asks three back; what
        // is typed is not looked at.
        self.show_and_confirm(phrase, random_positions()?)?;
        Ok(())
    }

    fn where_asked(&self) -> &'static str {
        if self.backend == Backend::Terminal {
            "this terminal"
        } else {
            "a window"
        }
    }
}

/// The words in the terminal, then three of them back.
///
/// The screen is cleared once they are written down, so the scrollback of a
/// terminal left open does not keep them -- the reason `init`'s printout was
/// never the end state a wizard should leave.
fn terminal_show_and_confirm(
    phrase: &str,
    positions: [usize; CONFIRM_WORDS],
) -> Result<Vec<Secret>> {
    println!();
    println!("{WORDS_INSTRUCTION}");
    println!();
    print!("{}", grid(phrase));
    println!();
    let _ = terminal_line("Press Enter once they are written down. ")?;
    if std::io::stdout().is_terminal() {
        // Clear the screen and the scrollback, then home the cursor.
        print!("\x1b[2J\x1b[3J\x1b[H");
        let _ = std::io::stdout().flush();
    }
    println!("Now three of them, from where you kept them.");
    positions
        .iter()
        .map(|position| terminal_line(&format!("Word number {position}: ")))
        .collect()
}

/// The words in the terminal, until Enter; then the screen is cleared so the
/// scrollback of a terminal left open does not keep them.
fn terminal_show_words(phrase: &str) -> Result<()> {
    println!();
    println!("{WORDS_INSTRUCTION}");
    println!();
    print!("{}", grid(phrase));
    println!();
    let _ = terminal_line("Press Enter once you have kept them. ")?;
    if std::io::stdout().is_terminal() {
        print!("[2J[3J[H");
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

/// For `--answers`: nothing can be asked, so everything comes from where the
/// CLI already takes it.
#[derive(Debug)]
pub(crate) struct Unattended {
    /// `--phrase-file`, for joining an account from its 24 words.
    pub(crate) phrase_file: Option<PathBuf>,
}

impl SecretPrompt for Unattended {
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        _positions: [usize; CONFIRM_WORDS],
    ) -> Result<Option<Vec<Secret>>> {
        // Exactly what `itsanas init` does with no terminal: the words go to
        // standard output once, and the caller -- a provisioning script, or a
        // person who chose a file of answers -- owns that output.
        println!("{WORDS_INSTRUCTION}");
        println!();
        print!("{}", grid(phrase));
        println!();
        println!("This phrase is shown once and is not stored anywhere on this machine.");
        Ok(None)
    }

    fn new_passphrase(&mut self) -> Result<Secret> {
        self.passphrase("")
    }

    fn passphrase(&mut self, _purpose: &str) -> Result<Secret> {
        match std::env::var(crate::PASSPHRASE_ENV) {
            Ok(value) => not_empty(Zeroizing::new(value), "passphrase"),
            Err(_) => Err(CliError::Usage(format!(
                "an unattended setup reads the passphrase from {}; set it, understanding that \
                 anything able to read this process's environment can then read it",
                crate::PASSPHRASE_ENV
            ))),
        }
    }

    fn recovery_phrase(&mut self) -> Result<Secret> {
        let Some(path) = &self.phrase_file else {
            return Err(CliError::Usage(
                "joining an account unattended needs its 24 words in a file: --phrase-file FILE"
                    .to_owned(),
            ));
        };
        std::fs::read_to_string(path)
            .map(Zeroizing::new)
            .map_err(|error| CliError::Io {
                path: path.clone(),
                source: error,
            })
    }

    fn where_asked(&self) -> &'static str {
        "the environment"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str = "abandon ability able about above absent absorb abstract absurd abuse \
                          access accident account accuse achieve acid acoustic acquire across act \
                          action actor actress actual";
    const PASSPHRASE: &str = "correct horse battery staple";

    /// The person saves the words over an existing 0644 file from an earlier
    /// attempt: the 24 words, which restore the whole account, end up in a
    /// file only they can read.
    #[cfg(unix)]
    #[test]
    fn red_team_words_saved_over_an_open_file_are_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ITSaNAS recovery words.txt");
        std::fs::write(&path, b"an earlier attempt").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        save_words(&path, PHRASE).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "other accounts can read the words");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PHRASE);
    }

    fn every_ask() -> Vec<Ask<'static>> {
        vec![
            Ask::ShowAndConfirm {
                phrase: PHRASE,
                positions: [3, 11, 20],
            },
            Ask::NewPassphrase,
            Ask::Passphrase {
                purpose: "this machine's passphrase",
            },
            Ask::RecoveryPhrase,
        ]
    }

    #[test]
    fn red_team_no_secret_window_carries_a_secret_in_argv_or_env() {
        // Every process of the same user reads another's argv (`ps`, Process
        // Explorer) and environment (`/proc/PID/environ`): a word there is a
        // word anybody on the machine can collect.
        for backend in [
            Backend::WinForms,
            Backend::AppleScript,
            Backend::Zenity,
            Backend::Kdialog,
        ] {
            for ask in every_ask() {
                let steps = plan(backend, ask);
                assert!(!steps.is_empty(), "{backend:?} has no window for {ask:?}");
                for step in &steps {
                    let mut visible: Vec<String> = step
                        .command
                        .get_args()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect();
                    visible.push(step.command.get_program().to_string_lossy().into_owned());
                    for (key, value) in step.command.get_envs() {
                        visible.push(key.to_string_lossy().into_owned());
                        visible.extend(value.map(|v| v.to_string_lossy().into_owned()));
                    }
                    let seen = visible.join(" ").to_lowercase();
                    for word in PHRASE.split_whitespace().chain([PASSPHRASE]) {
                        let leaked = seen
                            .split(|c: char| !c.is_ascii_alphanumeric())
                            .any(|token| token == word);
                        assert!(
                            !leaked,
                            "{backend:?} puts the secret {word:?} on the command line or in the \
                             environment of the window it opens for {ask:?}: every program this \
                             user runs could read the recovery words"
                        );
                    }
                }
            }
        }
    }

    /// zenity's words window offers Copy and Save, and the clipboard is the
    /// desktop's own tool, the words on its standard input.
    #[test]
    fn the_linux_words_window_offers_copy_and_save_on_the_desktops_clipboard() {
        let step = plan(
            Backend::Zenity,
            Ask::ShowAndConfirm {
                phrase: PHRASE,
                positions: [1, 2, 3],
            },
        )
        .remove(0);
        let args: Vec<String> = step
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for label in [ZENITY_COPY, ZENITY_SAVE] {
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--extra-button" && pair[1] == label),
                "zenity's words window has no {label} button: the words go on paper or nowhere"
            );
        }
        let all = |_: &str| true;
        assert_eq!(
            linux_clipboard(true, &all).map(|tool| tool.copy[0]),
            Some("wl-copy")
        );
        assert_eq!(
            linux_clipboard(false, &all).map(|tool| tool.copy[0]),
            Some("xclip"),
            "an X desktop was handed a Wayland tool, which copies nothing there"
        );
        assert_eq!(
            linux_clipboard(true, &|program| program == "xsel").map(|tool| tool.copy[0]),
            Some("xsel")
        );
        assert_eq!(linux_clipboard(true, &|_| false), None);
    }

    /// The Mac's words window offers Copy and Save as the Windows one does, and
    /// empties the clipboard it filled once the check ends, only if it still
    /// holds the words. Sabotage: drop the emptying.
    #[test]
    fn the_mac_words_window_offers_copy_and_save_and_empties_the_clipboard_it_filled() {
        let step = plan(
            Backend::AppleScript,
            Ask::ShowAndConfirm {
                phrase: PHRASE,
                positions: [1, 2, 3],
            },
        )
        .remove(0);
        let script = step.stdin.as_str();
        for button in [
            "\"Copy\"",
            "\"Save to a file...\"",
            "\"I have written them down\"",
        ] {
            assert!(
                script.contains(button),
                "the Mac's words window has no {button} button: the words go on paper or nowhere"
            );
        }
        assert!(
            script.contains("set the clipboard to phrase24")
                && script.contains("write phrase24 to f"),
            "Copy or Save does not hand over the words themselves"
        );
        let emptied = script
            .find("if (the clipboard as text) is phrase24 then set the clipboard to \"\"")
            .expect("the words copied stay on the clipboard for any program to read, for good");
        let checked = script.find("return a & linefeed").expect("the check");
        assert!(
            emptied < checked,
            "the clipboard is emptied after the script has returned, which is never"
        );
        assert!(
            !script.contains("do shell script"),
            "the words go through a shell, whose command line every program of this user can read"
        );
    }

    /// The Windows window refuses a new passphrase with itself still open, so its
    /// numbers have to be the core's: said to differ, it would either pass what the
    /// engine refuses, losing the words just shown, or refuse what the rule allows.
    /// Sabotage: change either number in the script; count `.Length` instead of
    /// characters; drop the `$Twice` guard (an existing short passphrase is locked
    /// out of a window asking for it).
    #[test]
    fn red_team_the_passphrase_window_states_the_rule_the_core_enforces() {
        use crate::node::{MAX_PASSPHRASE_CHARS, MIN_PASSPHRASE_CHARS};
        let min = format!("$MinPassphraseChars = {MIN_PASSPHRASE_CHARS}\n");
        let max = format!("$MaxPassphraseChars = {MAX_PASSPHRASE_CHARS}\n");
        for line in [&min, &max] {
            assert!(
                WINFORMS_SCRIPT.contains(line.as_str()),
                "secret-window.ps1 does not say {line:?}: the window and the core disagree on \
                 what a passphrase may be"
            );
        }
        let refusal = WINFORMS_SCRIPT
            .lines()
            .find(|line| line.contains("-lt $min"))
            .expect("the window does not check the rule");
        assert!(
            refusal.contains("$Twice"),
            "the rule reaches a window asking for an existing passphrase: {refusal}"
        );
        assert!(
            refusal.contains("$characters") && WINFORMS_SCRIPT.contains("IsLowSurrogate"),
            "the window counts UTF-16 units where the core counts characters: {refusal}"
        );
        for box_name in ["$first", "$second"] {
            assert!(
                WINFORMS_SCRIPT.contains(&format!(
                    "{box_name}.UseSystemPasswordChar = -not $show.Checked"
                )),
                "Show does not reach {box_name}: half of what is being typed stays hidden"
            );
        }
    }

    /// The installers say the rule before their build rather than after it, in
    /// the core's numbers, and only for an account about to be made: a machine
    /// re-provisioned keeps the passphrase it has. Sabotage: change a number in a
    /// script; drop the keystore condition (a short passphrase from before the
    /// rule stops a re-run on the machine it opens).
    #[test]
    fn the_installers_state_the_rule_the_core_enforces() {
        use crate::node::{MAX_PASSPHRASE_CHARS, MIN_PASSPHRASE_CHARS};
        let rule = format!("{MIN_PASSPHRASE_CHARS} to {MAX_PASSPHRASE_CHARS} characters");
        for (name, script, check) in [
            (
                "provision.sh",
                include_str!("../../../../install/provision.sh"),
                format!(
                    "keystore.bin\" ] && [ \"${{#ITSANAS_PASSPHRASE}}\" -lt {MIN_PASSPHRASE_CHARS} ]"
                ),
            ),
            (
                "provision.ps1",
                include_str!("../../../../install/provision.ps1"),
                format!("'keystore.bin'))) -and $passphrase.Length -lt {MIN_PASSPHRASE_CHARS}"),
            ),
        ] {
            assert!(script.contains(&rule), "{name} does not say {rule:?}");
            assert!(
                script.contains(&check),
                "{name} refuses by another number, or refuses a node that already exists"
            );
        }
    }

    #[test]
    fn red_team_typed_back_words_are_checked_not_waved_through() {
        // The account is written only after this says yes: a "yes" to a wrong
        // word is a person leaving with a paper that restores nothing.
        // Words 3, 11 and 19 of PHRASE: able, access, across.
        let positions = [3, 11, 19];
        let right: Vec<Secret> = ["able", "access", "across"]
            .iter()
            .map(|w| Zeroizing::new((*w).to_owned()))
            .collect();
        assert!(
            words_match(PHRASE, &positions, &right),
            "the right words were refused: nobody could finish setting up an account"
        );
        let capitals: Vec<Secret> = [" Able", "ACCESS ", "across"]
            .iter()
            .map(|w| Zeroizing::new((*w).to_owned()))
            .collect();
        assert!(
            words_match(PHRASE, &positions, &capitals),
            "a capital from a phone keyboard was refused as a wrong word"
        );
        for wrong in [
            ["able", "access", "act"],
            ["ability", "access", "across"],
            ["able", "", "across"],
        ] {
            let typed: Vec<Secret> = wrong
                .iter()
                .map(|w| Zeroizing::new((*w).to_owned()))
                .collect();
            assert!(
                !words_match(PHRASE, &positions, &typed),
                "{wrong:?} was accepted for positions {positions:?}: a person with a bad copy of \
                 their words would be told it is good, and lose the account with the machine"
            );
        }
        assert!(
            !words_match(PHRASE, &positions, &right[..2]),
            "two words were accepted where three were asked"
        );
    }

    #[test]
    fn positions_are_three_distinct_words_of_the_twenty_four() {
        for _ in 0..200 {
            let positions = random_positions().expect("randomness");
            assert!(
                positions.windows(2).all(|pair| pair[0] < pair[1])
                    && positions.iter().all(|p| (1..=24).contains(p)),
                "{positions:?}: a position repeated or outside 1-24 asks a word the paper does not have"
            );
        }
    }

    #[test]
    fn the_windows_reply_survives_any_code_page() {
        // "é" through a redirected console in code page 850 would come back
        // as another byte: a different passphrase than the one typed.
        assert_eq!(
            unhex("c3a9746521").map(|s| s.to_string()),
            Some("\u{e9}te!".to_owned()),
            "an accented passphrase came back changed, and would lock the person out"
        );
        assert!(unhex("zz").is_none(), "garbage was read as a passphrase");
        assert_eq!(
            base64(b"any carnal pleas"),
            "YW55IGNhcm5hbCBwbGVhcw==",
            "the script PowerShell decodes would be corrupt and no window would open"
        );
        assert_eq!(base64(b"ab"), "YWI=", "two-byte tail encoded wrong");
    }

    #[test]
    fn the_secret_window_is_the_platforms_own_or_the_terminal() {
        let none = |_: &str| false;
        let zenity = |p: &str| p == "zenity";
        assert_eq!(
            choose_for("windows", false, true, false, &none).ok(),
            Some(Backend::WinForms)
        );
        assert_eq!(
            choose_for("windows", true, true, false, &none).ok(),
            Some(Backend::Terminal),
            "over SSH a window opens on a screen nobody is looking at"
        );
        assert_eq!(
            choose_for("linux", false, true, true, &zenity).ok(),
            Some(Backend::Zenity)
        );
        assert_eq!(
            choose_for("linux", false, true, false, &zenity).ok(),
            Some(Backend::Terminal),
            "zenity with no display would fail where the terminal works"
        );
        let refused = choose_for("linux", false, false, false, &none)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            refused.contains("zenity"),
            "nowhere to ask, and the refusal does not say what to install: {refused}"
        );
    }
}
