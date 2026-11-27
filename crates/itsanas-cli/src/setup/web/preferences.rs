//! The page's language and theme: chosen in the page, kept in the node's
//! home, and written into the first byte of `index.html`.

use std::path::Path;

use super::{
    Context, INDEX_HTML,
    http::{Request, Response},
    json::{self, field, object, quote},
    lock, ok_json,
};

/// The languages the page speaks: English, written in the page, and those of
/// `i18n.js`.
pub(super) const LANGUAGES: [&str; 3] = ["en", "fr", "zh"];
/// In a node's home, the language chosen in the page's list. Each page is a
/// new port, so a new origin with empty browser storage: a choice kept there
/// was gone at the next `itsanas settings` (hand test, 2026-10-22: French
/// chosen in setup, Settings in English). Kept beside the node, the next page
/// and the tray read the same word.
pub(crate) const LANGUAGE_FILE: &str = "language";

/// The language chosen in a page of this node, when one was.
pub(crate) fn saved_language(home: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(home.join(LANGUAGE_FILE)).ok()?;
    let word = text.trim();
    LANGUAGES.into_iter().find(|language| *language == word)
}

/// Keep `language` for the next page, once the node's home exists: before the
/// first step of a setup it does not, and the choice waits in memory until the
/// run ends.
pub(super) fn save_language(home: &Path, language: &str) -> bool {
    home.is_dir() && std::fs::write(home.join(LANGUAGE_FILE), format!("{language}\n")).is_ok()
}

/// The themes the page has: the system's own, or one of its two.
pub(super) const THEMES: [&str; 3] = ["system", "light", "dark"];
/// In a node's home, the theme chosen in Settings, beside [`LANGUAGE_FILE`] for the
/// same reason: each page is a new origin, and a phone's native screens read the
/// same word.
pub(crate) const THEME_FILE: &str = "theme";

/// The theme chosen in a page of this node, when one was.
pub(crate) fn saved_theme(home: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(home.join(THEME_FILE)).ok()?;
    let word = text.trim();
    THEMES.into_iter().find(|theme| *theme == word)
}

/// Keep `theme`, once the home exists; before it does the choice waits in memory.
pub(super) fn save_theme(home: &Path, theme: &str) -> bool {
    home.is_dir() && std::fs::write(home.join(THEME_FILE), format!("{theme}\n")).is_ok()
}

/// The language of the page a browser asks for in `Accept-Language`: the one
/// it prefers most (by `q`, then by order) among those the page speaks, by
/// primary tag, so `fr-CA` is French. `None` when it asks for none of them.
pub(crate) fn language_of(accept: &str) -> Option<&'static str> {
    let mut best: Option<(u16, &'static str)> = None;
    for item in accept.split(',') {
        let mut parts = item.split(';');
        let tag = parts.next().unwrap_or("").trim();
        let primary = tag.split('-').next().unwrap_or("").to_ascii_lowercase();
        let quality = parts
            .find_map(|param| param.trim().strip_prefix("q="))
            .map_or(Some(1000), quality_of);
        let (Some(quality), Some(language)) = (
            quality,
            LANGUAGES.into_iter().find(|language| *language == primary),
        ) else {
            continue;
        };
        if quality > 0 && best.is_none_or(|(had, _)| quality > had) {
            best = Some((quality, language));
        }
    }
    best.map(|(_, language)| language)
}

/// A `q` value in thousandths, without a float: `1`, `0.8`, `0.125`.
pub(super) fn quality_of(text: &str) -> Option<u16> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let thousandths = format!("{fraction:0<3}").parse::<u16>().ok()?;
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(1000),
        _ => None,
    }
}

/// The page, in the language chosen before, else the browser's: said in
/// `<html lang>` from the first byte, so nothing shows in English first and
/// the page needs no request to know it.
pub(super) fn index_html(ctx: &Context, request: &Request) -> String {
    let chosen = *lock(&ctx.language);
    let mut page = match chosen.or_else(|| request.header("accept-language").and_then(language_of))
    {
        Some(language) => INDEX_HTML.replacen(
            "<html lang=\"en\">",
            &format!("<html lang=\"{language}\" data-lang=\"{language}\">"),
            1,
        ),
        None => INDEX_HTML.to_owned(),
    };
    // In the first byte, so nothing flashes in the other theme first.
    if let Some(theme) = (*lock(&ctx.theme)).filter(|theme| *theme != "system") {
        page = page.replacen("<html ", &format!("<html data-theme=\"{theme}\" "), 1);
    }
    page
}

/// The theme chosen in Settings, kept for the next page, and told to the shell
/// (a phone draws its own screens by the same word).
pub(super) fn choose_theme(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let Some(theme) =
        field(&form, "theme").and_then(|asked| THEMES.into_iter().find(|theme| *theme == asked))
    else {
        return Response::refuse(400, "the theme is system, light or dark");
    };
    *lock(&ctx.theme) = Some(theme);
    let kept = save_theme(&ctx.home, theme);
    ctx.backends.host.event("theme", theme);
    ok_json(object(&[
        ("theme", quote(theme)),
        ("kept", kept.to_string()),
    ]))
}

/// The language chosen in the page's list, kept for the next page.
pub(super) fn choose_language(ctx: &Context, request: &Request) -> Response {
    let form = match json::parse_form(&request.body) {
        Ok(form) => form,
        Err(why) => return Response::refuse(400, &why),
    };
    let Some(language) = field(&form, "lang")
        .and_then(|asked| LANGUAGES.into_iter().find(|language| *language == asked))
    else {
        return Response::refuse(
            400,
            "this page speaks English (en), French (fr) and Simplified Chinese (zh)",
        );
    };
    *lock(&ctx.language) = Some(language);
    let kept = save_language(&ctx.home, language);
    ok_json(object(&[
        ("language", quote(language)),
        ("kept", kept.to_string()),
    ]))
}
