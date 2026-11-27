//! The Linux desktop windows: `zenity` or `kdialog`, and the desktop's
//! clipboard for the words window's copy and save.

use std::{
    io::Write as _,
    path::PathBuf,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use super::{Ask, Prepared, Secret, WORDS_INSTRUCTION, grid, on_path};

/// zenity's extra buttons on the words window; a click exits non-zero and
/// prints the label, which [`Native`] reads to copy or save and show again.
pub(super) const ZENITY_COPY: &str = "Copy";
pub(super) const ZENITY_SAVE: &str = "Save to a file...";

/// A Linux desktop's clipboard, as commands: one that takes the text on its
/// standard input, never in its argv, and one that prints it back.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Clipboard {
    pub(super) copy: &'static [&'static str],
    paste: &'static [&'static str],
}

/// The clipboard tool of this desktop: `wl-copy` under Wayland, else `xclip`
/// or `xsel`; `None` when none is installed.
pub(super) fn linux_clipboard(wayland: bool, found: &dyn Fn(&str) -> bool) -> Option<Clipboard> {
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
pub(super) fn piped(command: &[&str], input: &str) -> Option<Secret> {
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
pub(super) fn linux_copy(phrase: &str) -> bool {
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
pub(super) fn linux_save(phrase: &str) {
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
    let path = PathBuf::from(path.trim());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let saved = options
        .open(&path)
        .and_then(|mut file| file.write_all(phrase.as_bytes()));
    zenity_says(if saved.is_ok() {
        "Saved. Move it to a USB stick or a password manager: on this disk, anyone who takes the machine has your files."
    } else {
        "The file could not be written; choose another place."
    });
}

/// Once the check ends: the clipboard emptied if it still holds the words;
/// something copied since is the person's own and stays.
pub(super) fn linux_clipboard_emptied(phrase: &str) {
    if let Some(tool) = this_clipboard()
        && piped(tool.paste, "").is_some_and(|held| held.trim() == phrase.trim())
    {
        let _ = piped(tool.copy, "");
    }
}

pub(super) fn zenity(ask: Ask<'_>) -> Vec<Prepared> {
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

pub(super) fn kdialog(ask: Ask<'_>) -> Vec<Prepared> {
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
