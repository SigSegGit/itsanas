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
//!
//! The commands each window backend runs are built in [`commands`] (Windows),
//! [`mac`] and [`linux`]; [`native`] runs them, or asks on the terminal.

mod commands;
mod linux;
mod mac;
mod native;

#[cfg(test)]
pub(crate) use self::commands::WINFORMS_SCRIPT;
pub(crate) use self::commands::{Prepared, plan, powershell};
pub(crate) use self::native::{Native, Unattended};

use std::io::IsTerminal as _;

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

#[cfg(test)]
mod tests {
    use super::*;
    use super::{commands::*, linux::*};

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
