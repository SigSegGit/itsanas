//! The Mac window: `display dialog` through `osascript`, and the words
//! window's copy and save.

use super::{Ask, Prepared, WORDS_INSTRUCTION, grid};

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

pub(super) fn applescript(ask: Ask<'_>) -> Prepared {
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
