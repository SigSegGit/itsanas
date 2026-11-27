//! Progress, for a page that follows a run in the background.

use std::{io::Read as _, path::Path};

use super::STATUS;
use crate::error::Result;

/// A JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `<home>/copy-to-key.status`: `{"step", "done", "said", "error"}`, replaced
/// whole through a temporary name at each step. Best effort: a status that
/// cannot be written never stops the copy.
pub(super) struct Status<'a>(pub(super) &'a Path);

impl Status<'_> {
    pub(super) fn write(&self, step: &str, done: bool, said: &str, error: Option<&str>) {
        let text = format!(
            "{{\"step\":{},\"done\":{done},\"said\":{},\"error\":{}}}\n",
            json_string(step),
            json_string(said),
            error.map_or_else(|| "null".to_owned(), json_string)
        );
        let staging = self.0.join(format!("{STATUS}.tmp"));
        if std::fs::write(&staging, text).is_ok() {
            let _ = std::fs::rename(&staging, self.0.join(STATUS));
        }
    }

    pub(super) fn step(&self, step: &str, said: &str) {
        self.write(step, false, said, None);
    }

    pub(super) fn ended(&self, result: &Result<String>) {
        match result {
            Ok(said) => self.write("done", true, said, None),
            Err(error) => self.write("failed", true, "", Some(&error.to_string())),
        }
    }
}

/// A run's progress, as [`Status`] wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Progress {
    pub(crate) step: String,
    pub(crate) done: bool,
    pub(crate) said: String,
    pub(crate) error: Option<String>,
}

/// The progress of the last run from this home, and when it was written;
/// `None` when no run wrote one, or it is not what [`Status`] writes.
pub(crate) fn progress(home: &Path) -> Option<(Progress, std::time::SystemTime)> {
    let path = home.join(STATUS);
    let file = std::fs::File::open(&path).ok()?;
    let written = file.metadata().and_then(|meta| meta.modified()).ok()?;
    let mut text = String::new();
    file.take(64 * 1024).read_to_string(&mut text).ok()?;
    Some((parse_progress(&text)?, written))
}

/// Forget the last run's progress, before a new one starts.
pub(crate) fn forget_progress(home: &Path) {
    let _ = std::fs::remove_file(home.join(STATUS));
}

/// The one line [`Status::write`] writes, read back.
fn parse_progress(text: &str) -> Option<Progress> {
    let rest = text.trim().strip_prefix("{\"step\":")?;
    let (step, rest) = read_json_string(rest)?;
    let rest = rest.strip_prefix(",\"done\":")?;
    let (done, rest) = if let Some(rest) = rest.strip_prefix("true") {
        (true, rest)
    } else {
        (false, rest.strip_prefix("false")?)
    };
    let (said, rest) = read_json_string(rest.strip_prefix(",\"said\":")?)?;
    let rest = rest.strip_prefix(",\"error\":")?;
    let (error, rest) = if let Some(rest) = rest.strip_prefix("null") {
        (None, rest)
    } else {
        let (error, rest) = read_json_string(rest)?;
        (Some(error), rest)
    };
    (rest == "}").then_some(Progress {
        step,
        done,
        said,
        error,
    })
}

/// A JSON string literal at the start of `text`, as [`json_string`] writes
/// it, and what follows.
fn read_json_string(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((out, &text[1 + at + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = (0..4)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}
