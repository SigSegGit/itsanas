//! Running the window backends, and the terminal when there is no desktop;
//! and the unattended answers in their place.

use std::{
    io::{IsTerminal as _, Write as _},
    path::PathBuf,
    process::Stdio,
};

use zeroize::Zeroizing;

use super::{
    Ask, Backend, CONFIRM_WORDS, Prepared, Secret, SecretPrompt, WORDS_INSTRUCTION,
    commands::unhex,
    grid,
    linux::{ZENITY_COPY, ZENITY_SAVE, linux_clipboard_emptied, linux_copy, linux_save},
    plan, random_positions,
};
use crate::error::{CliError, Result};

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
