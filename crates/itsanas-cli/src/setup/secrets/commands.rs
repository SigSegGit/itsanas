//! The commands a window backend runs, built as data: one program and what to
//! feed it, the plan of each backend, and the Windows Forms window.

use std::process::Command;

use zeroize::Zeroizing;

use super::{
    Ask, Backend, Secret,
    linux::{kdialog, zenity},
    mac::applescript,
};

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
    pub(super) fn new(program: &str, args: &[&str], stdin: String) -> Self {
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
pub(crate) const WINFORMS_SCRIPT: &str = include_str!("../secret-window.ps1");

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
pub(super) fn unhex(line: &str) -> Option<Secret> {
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
