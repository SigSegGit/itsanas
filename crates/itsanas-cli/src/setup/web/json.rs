//! JSON out and a form in, by hand: the page's API is a handful of flat
//! objects, and `serde_json` would be a new dependency for one escaper.

/// `text` as a JSON string literal. `<`, `>` and `&` are escaped too, so a
/// value can never close a tag if a body is ever shown as HTML by mistake.
pub(super) fn quote(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if matches!(c, '<' | '>' | '&') || u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `null`, or the quoted text.
pub(super) fn optional(text: Option<&str>) -> String {
    text.map_or_else(|| "null".to_owned(), quote)
}

/// An object from already-encoded values.
pub(super) fn object(fields: &[(&str, String)]) -> String {
    let inner: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{value}", quote(key)))
        .collect();
    format!("{{{}}}", inner.join(","))
}

/// An array from already-encoded values.
pub(super) fn array(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}

/// An `application/x-www-form-urlencoded` body, as the page's
/// `URLSearchParams` writes it. Refused rather than guessed at when it is not
/// UTF-8 or a `%` is not followed by two hex digits.
pub(super) fn parse_form(body: &[u8]) -> Result<Vec<(String, String)>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "the form is not UTF-8".to_owned())?;
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((decode(key)?, decode(value)?))
        })
        .collect()
}

fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes
                    .get(index + 1..index + 3)
                    .and_then(|pair| std::str::from_utf8(pair).ok())
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                    .ok_or_else(|| "a form value is badly encoded".to_owned())?;
                out.push(hex);
                index += 2;
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8(out).map_err(|_| "a form value is not UTF-8".to_owned())
}

/// The value of `key` in a parsed form, trimmed; `None` when absent or empty.
pub(super) fn field<'a>(form: &'a [(String, String)], key: &str) -> Option<&'a str> {
    form.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}
