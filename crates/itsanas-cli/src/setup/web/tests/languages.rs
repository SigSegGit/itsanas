//! The page's languages: every text of the page and of the server has its
//! translation, and the language chosen is kept and wins over the browser's.

use super::*;

/// What `/api/name` says of a name is shown in the page's language.
#[test]
fn every_reason_a_username_is_refused_has_a_french_translation() {
    let french = french_keys();
    for bad in ["", "marie_1", "-nope"] {
        let why = crate::coordinator::username_refusal(bad).expect("a refused name");
        assert!(
            french.contains(&why),
            "the page shows this in English whatever the language chosen: {why}"
        );
    }
}

/// Every text a person reads in the page, as the English the translations
/// are keyed by: the text between tags (outside scripts and code), the
/// placeholders and labels, and what app.js passes to `t(...)`.
fn texts_of_the_page() -> Vec<String> {
    let html = super::INDEX_HTML;
    let body = &html[html.find("<body>").expect("body")..];
    let mut texts = Vec::new();
    let mut skip = 0_i32;
    for piece in body.split('<').skip(1) {
        let (tag, text) = piece.split_once('>').unwrap_or((piece, ""));
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if matches!(name.as_str(), "script" | "style" | "code") {
            skip += if tag.starts_with('/') { -1 } else { 1 };
        }
        for attribute in ["placeholder=\"", "aria-label=\""] {
            if let Some(start) = tag.find(attribute) {
                let rest = &tag[start + attribute.len()..];
                texts.push(rest[..rest.find('"').expect("quote")].to_owned());
            }
        }
        let text = text
            .replace("&mdash;", "—")
            .replace("&hellip;", "…")
            .replace("&amp;", "&");
        let text = text.trim();
        if skip == 0 && !text.is_empty() {
            texts.push(text.to_owned());
        }
    }
    // `t('` as a call, not the end of `createElement('`; `N('` marks a text
    // of a table translated where it is shown; `say('` translates its text.
    let js = super::APP_JS;
    for call in ["t('", "N('", "say('"] {
        let mut from = 0;
        while let Some(at) = js[from..].find(call) {
            let start = from + at;
            from = start + call.len();
            if js[..start].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            let rest = &js[from..];
            let text = rest[..rest.find("')").expect("end of the call")].replace("\\'", "'");
            // `say('')` clears the line: nothing to translate.
            if !text.is_empty() {
                texts.push(text);
            }
        }
    }
    for label in super::APP_JS.split("label: '").skip(1) {
        texts.push(label[..label.find('\'').expect("label")].to_owned());
    }
    texts
}

/// One JavaScript string literal at the start of `text`: its value and what
/// follows it.
fn js_string(text: &str) -> (String, &str) {
    let quote = text.chars().next().expect("a string");
    assert!(
        quote == '\'' || quote == '"',
        "not a string in i18n.js: {}",
        &text[..text.len().min(60)]
    );
    let mut value = String::new();
    let mut chars = text[1..].char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => match chars.next().expect("an escape").1 {
                'n' => value.push('\n'),
                other => value.push(other),
            },
            c if c == quote => return (value, &text[1 + at + 1..]),
            c => value.push(c),
        }
    }
    panic!("a string in i18n.js never ends");
}

/// The French table of i18n.js, entry by entry, as the browser reads it.
fn french_table() -> Vec<(String, String)> {
    table_of("fr")
}

/// The table of `lang` in i18n.js, entry by entry, as the browser reads it. A
/// line the reader does not understand is a failure, not a skipped entry.
fn table_of(lang: &str) -> Vec<(String, String)> {
    let js = super::I18N_JS;
    let opening = format!("\n  {lang}: {{");
    let mut rest = &js[js
        .find(&opening)
        .unwrap_or_else(|| panic!("no {lang} table in i18n.js"))
        + opening.len()..];
    let mut entries = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.starts_with('}') {
            return entries;
        }
        if let Some(comment) = rest.strip_prefix("//") {
            rest = comment.split_once('\n').map_or("", |(_, next)| next);
            continue;
        }
        let (key, after) = js_string(rest);
        let after = after
            .trim_start()
            .strip_prefix(':')
            .unwrap_or_else(|| panic!("no ':' after {key:?} in i18n.js"));
        let (value, after) = js_string(after.trim_start());
        entries.push((key, value));
        rest = after;
    }
}

fn french_keys() -> BTreeSet<String> {
    french_table().into_iter().map(|(key, _)| key).collect()
}

#[test]
fn the_french_table_has_one_entry_per_english_text_and_none_empty() {
    let table = french_table();
    assert!(
        table.len() > 100,
        "i18n.js was misread: {} entries",
        table.len()
    );
    let mut keys = BTreeSet::new();
    for (english, french) in &table {
        assert!(
            keys.insert(english.as_str()),
            "{english:?} is twice in the French table: the browser keeps the last one, and the \
             first, maybe the one corrected, is silently dropped"
        );
        assert!(
            !french.trim().is_empty(),
            "{english:?} has an empty French entry: the page would show nothing there"
        );
    }
}

/// The `{name}` slots of a text, which the script fills in.
fn slots(text: &str) -> BTreeSet<&str> {
    text.match_indices('{')
        .filter_map(|(at, _)| text[at..].find('}').map(|end| &text[at..=at + end]))
        .collect()
}

#[test]
fn every_language_of_the_page_says_all_the_french_one_says() {
    // French is the reference every other test checks against the page; a
    // language is complete when it has the same keys, none empty, each with
    // the slots the script fills (a missing `{size}` would show a sentence
    // with no number in it).
    let french = french_keys();
    let others: Vec<&str> = super::LANGUAGES
        .into_iter()
        .filter(|lang| !matches!(*lang, "en" | "fr"))
        .collect();
    assert!(
        others.contains(&"zh"),
        "Chinese left the page's languages: {others:?}"
    );
    // A phone set to Chinese: its web view asks `zh-CN` first.
    assert_eq!(
        super::language_of("zh-CN,zh;q=0.9,en;q=0.8"),
        Some("zh"),
        "a phone set to Chinese would open the page in English"
    );
    for lang in others {
        let table = table_of(lang);
        let keys: BTreeSet<String> = table.iter().map(|(key, _)| key.clone()).collect();
        let missing: Vec<&String> = french.difference(&keys).collect();
        let extra: Vec<&String> = keys.difference(&french).collect();
        assert!(
            missing.is_empty() && extra.is_empty() && keys.len() == table.len(),
            "the {lang} table is not the French one's set of texts: a reader would see English among {lang}; missing {missing:#?}, extra {extra:#?}"
        );
        for (english, said) in &table {
            assert!(
                !said.trim().is_empty() && slots(english) == slots(said),
                "{lang}: {english:?} became {said:?}: empty, or a slot lost"
            );
        }
        assert!(
            super::INDEX_HTML.contains(&format!("<option value=\"{lang}\">")),
            "{lang} is spoken but cannot be chosen in the page's list"
        );
    }
}

#[test]
fn every_text_of_the_page_has_a_french_translation() {
    let french = french_keys();
    let untranslated: Vec<String> = texts_of_the_page()
        .into_iter()
        .filter(|text| !matches!(text.as_str(), "ITSaNAS" | "English" | "Français" | "中文"))
        .filter(|text| !french.contains(text))
        .collect();
    assert!(
        untranslated.is_empty(),
        "a French speaker would read these in English, in the middle of French ones: \
         add them to i18n.js: {untranslated:#?}"
    );
}

#[test]
fn no_text_set_by_the_script_skips_the_translation() {
    // Emptying a line (`textContent = ''`) shows no text.
    let js = super::APP_JS
        .replace("textContent = ''", "")
        .replace("textContent = \"\"", "");
    for bypass in [
        "textContent = '",
        "textContent = \"",
        "confirm('",
        "confirm(\"",
        "textContent = reply.said",
        "settings.job.said;",
    ] {
        assert!(
            !js.contains(bypass),
            "app.js sets a visible text with {bypass:?}: it shows in English whatever the \
             language chosen (hand test, 2026-10-22); pass it through t(), N() or said()"
        );
    }
}

#[test]
fn the_page_speaks_the_browser_s_language_until_one_is_chosen() {
    for (accept, language) in [
        ("fr-FR,fr;q=0.9,en-US;q=0.8,en;q=0.7", Some("fr")),
        ("fr", Some("fr")),
        ("FR-ca", Some("fr")),
        ("en-GB,en;q=0.9,fr;q=0.8", Some("en")),
        ("de-DE,de;q=0.9,fr;q=0.8,en;q=0.5", Some("fr")),
        ("en;q=0.5, fr;q=0.6", Some("fr")),
        ("fr;q=0, en", Some("en")),
        ("de-DE, *;q=0.1", None),
        ("fr;q=bogus", None),
        ("", None),
    ] {
        assert_eq!(language_of(accept), language, "Accept-Language: {accept:?}");
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let host = page.host();
    let french = page.send(
        "GET",
        "/",
        &[
            ("Host", &host),
            ("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8"),
        ],
        "",
    );
    assert!(
        french.body.contains("<html lang=\"fr\" data-lang=\"fr\">"),
        "a browser asking for French got a page that starts in English (hand test, \
         2026-10-22): {}",
        &french.body[..french.body.len().min(200)]
    );
    let english = page.send(
        "GET",
        "/",
        &[("Host", &host), ("Accept-Language", "en-US")],
        "",
    );
    assert!(english.body.contains("<html lang=\"en\" data-lang=\"en\">"));
    let unsaid = page.send("GET", "/", &[("Host", &host)], "");
    assert!(
        unsaid.body.contains("<html lang=\"en\">"),
        "with no Accept-Language the page must leave the choice to the browser's own language"
    );
}

#[test]
fn a_language_chosen_in_the_page_is_kept_in_the_node_s_home_and_wins_over_the_browser() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let host = page.host();
    assert_eq!(
        page.send("POST", "/api/language", &[("Host", &host)], "lang=fr")
            .status,
        403,
        "a language was set without the page's key"
    );
    assert_eq!(page.api("POST", "/api/language", "lang=de").status, 400);
    assert_eq!(page.api("POST", "/api/language", "lang=..%2Fx").status, 400);
    let chosen = page.api("POST", "/api/language", "lang=fr");
    assert_eq!(chosen.status, 200, "{}", chosen.body);
    assert_eq!(
        std::fs::read_to_string(home.join(LANGUAGE_FILE)).expect("language file"),
        "fr\n",
        "the choice was not kept beside the node: the next page, on another port, forgets it"
    );
    drop(page);
    // The next `itsanas settings`: a new server, a browser that asks for English.
    let next = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let host = next.host();
    let reply = next.send(
        "GET",
        "/",
        &[("Host", &host), ("Accept-Language", "en-US")],
        "",
    );
    assert!(
        reply.body.contains("data-lang=\"fr\""),
        "French was chosen in a page of this node and the next page opened in English"
    );

    // During a setup, before the node's home exists: nothing is made for it.
    let fresh = dir.path().join("fresh");
    let setup = Page::start(&fresh, dir.path(), Mode::Setup, &Seen::default());
    let early = setup.api("POST", "/api/language", "lang=fr");
    assert_eq!(early.status, 200, "{}", early.body);
    assert!(early.body.contains("\"kept\":false"), "{}", early.body);
    assert!(
        !fresh.exists(),
        "choosing a language made the node's home before setup did"
    );
}

#[test]
fn what_settings_hears_from_the_server_has_a_french_translation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    // Enough of a node for the control file; no daemon runs it.
    std::fs::write(home.join("keystore.bin"), b"x").expect("keystore");
    let page = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    let mut heard = Vec::new();
    for (path, body) in [
        ("/api/control", "action=pause&for=resume"),
        ("/api/control", "action=pause&for=resume"),
        ("/api/control", "action=sync-now"),
        ("/api/control", "action=resume"),
        ("/api/control", "action=resume"),
        ("/api/control", "action=sync-now"),
        ("/api/control", "action=interval&every=auto"),
        ("/api/disk", "action=export&path=%2Fx"),
    ] {
        let reply = page.api("POST", path, body);
        let field = if reply.status == 200 { "said" } else { "error" };
        heard.extend(json_field(&reply.body, field).lines().map(str::to_owned));
    }
    let french = french_keys();
    let untranslated: Vec<&String> = heard
        .iter()
        .filter(|line| !french.contains(*line))
        .collect();
    assert!(
        untranslated.is_empty(),
        "Settings shows what the server says line by line through i18n.js; these lines would \
         stay English in a French page: {untranslated:#?}"
    );
}
