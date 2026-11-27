//! The rules of a copy, as pure functions: the key's passphrase, the room
//! on the drive, and the target folder.

use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::MIN_LENGTH;
use crate::{
    config::Config,
    error::{CliError, Result},
    node::{Node, SNAPSHOT},
    portable,
};

/// What the five programs take when the release cannot say (0.1.11: about
/// 40 MiB together).
const PROGRAMS_GUESS: u64 = 45 * 1024 * 1024;

/// Why `key` will not do as the key's passphrase; `None` when it will.
///
/// Too short; this machine's own in any case, or holding it, or held in it
/// (one guess opens both, or nearly); the same few characters repeated
/// (`aaaaaaaaaaaa`, `abcabcabcabc`); a run of keys or digits
/// (`123456789012`, `abcdefghijkl`); a common base dressed in digits or
/// symbols (`Password2026!`, [`COMMON_BASES`]); or the username with too
/// little around it -- the username counts for nothing, it is on every claim.
#[must_use]
pub(crate) fn weak_passphrase(key: &str, machine: &str, username: &str) -> Option<String> {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < MIN_LENGTH {
        return Some(format!(
            "is {} characters long; a key's needs at least {MIN_LENGTH}",
            chars.len()
        ));
    }
    let (lower, machine) = (
        Zeroizing::new(key.to_lowercase()),
        Zeroizing::new(machine.to_lowercase()),
    );
    if *lower == *machine {
        return Some(
            "is this machine's passphrase: one guess would open both. Choose another".to_owned(),
        );
    }
    // Shorter than this, a passphrase held in another is a coincidence of
    // words rather than the same secret.
    let holds = |outer: &str, inner: &str| inner.chars().count() >= 8 && outer.contains(inner);
    if holds(&lower, &machine) || holds(&machine, &lower) {
        return Some(
            "holds this machine's passphrase, or is part of it: whoever knows one nearly \
             knows the other. Choose another"
                .to_owned(),
        );
    }
    let letters: Zeroizing<String> =
        Zeroizing::new(lower.chars().filter(|c| c.is_alphabetic()).collect());
    let common = COMMON_BASES.iter().any(|base| {
        if base.chars().all(char::is_alphabetic) {
            *letters == **base
        } else {
            letters.is_empty() && lower.contains(base)
        }
    });
    if common {
        return Some(
            "is a common password with digits or symbols around it, among the first \
             anybody tries"
                .to_owned(),
        );
    }
    let repeats = (1..=chars.len() / 2).any(|period| {
        chars.len().is_multiple_of(period)
            && chars
                .iter()
                .enumerate()
                .all(|(at, c)| *c == chars[at % period])
    });
    if repeats {
        return Some("is the same few characters repeated".to_owned());
    }
    if is_run(&chars) {
        return Some("is a run of keys or digits, the first thing anybody tries".to_owned());
    }
    let user = username.trim().to_lowercase();
    if user.chars().count() >= 3 {
        let rest = key.to_lowercase().replace(&user, "");
        if rest.chars().count() < MIN_LENGTH {
            return Some(format!(
                "is mostly your username, which counts for nothing: it needs {MIN_LENGTH} \
                 characters besides it"
            ));
        }
    }
    None
}

/// Bases every guessing list starts from: a passphrase that is one of them
/// plus digits or symbols is refused.
const COMMON_BASES: [&str; 6] = [
    "password",
    "motdepasse",
    "azerty",
    "qwerty",
    "itsanas",
    "123456",
];

/// Every character one up (or one down) from the one before: digits wrap
/// round, so `123456789012` is one.
fn is_run(chars: &[char]) -> bool {
    let step = |a: char, b: char| -> i64 {
        match (a.to_digit(10), b.to_digit(10)) {
            (Some(a), Some(b)) => match (b + 10 - a) % 10 {
                1 => 1,
                9 => -1,
                _ => 0,
            },
            _ => i64::from(u32::from(b)) - i64::from(u32::from(a)),
        }
    };
    let steps: Vec<i64> = chars
        .windows(2)
        .map(|pair| step(pair[0], pair[1]))
        .collect();
    steps.iter().all(|s| *s == 1) || steps.iter().all(|s| *s == -1)
}

/// Whether the key has room: `Ok` with what was counted, `Err` with why
/// not. `programs` is the release's size when it could be read, `content`
/// the account's files when a status said it, `free` the drive's.
///
/// A drive whose free space cannot be read is refused, not let through with
/// a note: `fs4` reads it on every system this ships for (`statvfs`,
/// `GetDiskFreeSpaceExW`), so a drive that cannot say is one failing or gone
/// under the copy -- the moment a copy would stop halfway on it.
///
/// # Errors
///
/// When the drive has less free than the copy needs, or cannot say.
pub(crate) fn room(
    programs: Option<u64>,
    content: Option<u64>,
    free: Option<u64>,
) -> std::result::Result<String, String> {
    use crate::config::format_size;
    let programs = programs.unwrap_or(PROGRAMS_GUESS);
    let needed = programs.saturating_add(content.unwrap_or(0));
    let counted = match content {
        Some(content) => format!(
            "{} for the programs and {} for your files",
            format_size(programs),
            format_size(content)
        ),
        None => format!(
            "{} for the programs; the size of your files is not known yet (no status from \
             this machine's service), so they were not counted",
            format_size(programs)
        ),
    };
    match free {
        None => Err(format!(
            "could not read the free space on the drive; make sure the key has at least {} \
             ({counted}), and try again once it answers. Nothing was written",
            format_size(needed)
        )),
        Some(free) if free < needed => Err(format!(
            "the drive has {} free and the copy needs about {}: {counted}. Free some room on \
             it, or choose a bigger key. Nothing was written",
            format_size(free),
            format_size(needed)
        )),
        Some(free) => Ok(format!(
            "room: about {} needed ({counted}), {} free",
            format_size(needed),
            format_size(free)
        )),
    }
}

/// The account's content on this machine, from the status the daemon last
/// wrote (its "your content" line), rounded up: the store is the daemon's
/// while it runs.
#[must_use]
pub(crate) fn content_in(snapshot: &str) -> Option<u64> {
    let line = snapshot
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("your content"))?;
    let (number, unit) = line.trim().split_once(' ')?;
    let unit: u64 = match unit.trim() {
        "B" => return number.parse().ok(),
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return None,
    };
    // `format_size` floors to a tenth: one tenth more never under-counts.
    let (whole, tenths) = number.split_once('.').unwrap_or((number, "0"));
    let whole: u64 = whole.parse().ok()?;
    let tenths: u64 = tenths.parse().ok()?;
    Some(
        whole
            .saturating_mul(unit)
            .saturating_add(tenths.saturating_add(1).saturating_mul(unit) / 10),
    )
}

/// Where the copy goes, `<dir>/ITSaNAS`, or why not there.
///
/// # Errors
///
/// A path `jobs::check_path` refuses, a drive that is not there, a place
/// inside this account's home, vault or synced folder (or holding one), or a
/// key that already holds an account.
pub(crate) fn check_target(home: &Path, config: &Config, dir: &Path) -> Result<PathBuf> {
    refusal(home, config, dir).map_err(|refused| CliError::Usage(refused.said))
}

/// Why a target will not do, as a word the Settings page translates and the
/// sentence the command prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// `path`, `missing`, `inside`, `account` or `room`.
    pub(crate) why: &'static str,
    pub(crate) said: String,
}

/// [`check_target`], with the kind of refusal kept.
pub(crate) fn refusal(
    home: &Path,
    config: &Config,
    dir: &Path,
) -> std::result::Result<PathBuf, Refusal> {
    let refused = |why, said| Err(Refusal { why, said });
    let dir = match crate::jobs::check_path(dir) {
        Ok(dir) => dir,
        Err(said) => return refused("path", said),
    };
    if dir
        .components()
        .any(|part| part == std::path::Component::ParentDir)
    {
        return refused(
            "path",
            format!(
                "{} goes up a folder (`..`) on the way: give the key's folder itself",
                dir.display()
            ),
        );
    }
    if crate::drives::existing_ancestor(&dir).is_none() {
        return refused(
            "missing",
            format!(
                "{} does not exist: no drive or folder there (is the key plugged in?)",
                dir.display()
            ),
        );
    }
    let root = dir.join(portable::DIR);
    let vault = Node::vault_path(home, config);
    let mut theirs = vec![
        (home.to_path_buf(), "this account's home"),
        (vault, "its vault"),
    ];
    if let Some(folder) = &config.folder {
        theirs.push((folder.clone(), "its synced folder"));
    }
    // Compared where they really are: a link on the way to the target (or
    // to the home) would put the key inside the account under another name.
    let real_root = resolved(&root);
    for (outer, what) in theirs {
        let real_outer = resolved(&outer);
        if real_root.starts_with(&real_outer)
            || real_outer.starts_with(&real_root)
            || root.starts_with(&outer)
            || outer.starts_with(&root)
        {
            return refused(
                "inside",
                format!(
                    "{} is inside {what} ({}), or holds it: the key would copy the account into \
                     itself. Choose the key's own drive",
                    dir.display(),
                    outer.display()
                ),
            );
        }
    }
    if Node::exists(&root.join("home")) {
        return refused(
            "account",
            format!(
                "this key already holds an account ({}); choose another folder. Nothing was \
                 changed",
                root.join("home").display()
            ),
        );
    }
    Ok(root)
}

/// `path` with its links resolved: its nearest existing ancestor made
/// canonical, the rest (not made yet, so holding no link) joined back.
fn resolved(path: &Path) -> PathBuf {
    let Some(existing) = crate::drives::existing_ancestor(path) else {
        return path.to_path_buf();
    };
    let Ok(real) = std::fs::canonicalize(existing) else {
        return path.to_path_buf();
    };
    match path.strip_prefix(existing) {
        Ok(rest) if !rest.as_os_str().is_empty() => real.join(rest),
        _ => real,
    }
}

/// What the five programs take: the release's size when it was read, else
/// the guess [`room`] counts.
#[must_use]
pub(crate) fn programs_bytes(programs: Option<u64>) -> u64 {
    programs.unwrap_or(PROGRAMS_GUESS)
}

/// The account's files on this machine, from the status its daemon last
/// wrote; `None` before it wrote one.
#[must_use]
pub(crate) fn content_of(home: &Path) -> Option<u64> {
    std::fs::read_to_string(home.join(SNAPSHOT))
        .ok()
        .and_then(|text| content_in(&text))
}

/// Whether `dir` will take the copy, as the Settings page lists it: every
/// refusal of [`check_target`] and [`room`], and the format's warning, which
/// does not stop it.
pub(crate) fn assess(
    home: &Path,
    config: &Config,
    dir: &Path,
    free: Option<u64>,
    format: Option<&crate::drives::Format>,
    programs: Option<u64>,
) -> (std::result::Result<(), Refusal>, Option<String>) {
    let fits = refusal(home, config, dir).and_then(|_| {
        room(programs, content_of(home), free)
            .map(|_| ())
            .map_err(|said| Refusal { why: "room", said })
    });
    (
        fits,
        crate::drives::warning(format, "the copy of your account"),
    )
}
