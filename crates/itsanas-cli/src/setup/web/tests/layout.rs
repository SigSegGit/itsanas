//! What the page looks like and links to: one column of cards, the theme in
//! the first byte, and the legal texts and links.

use super::*;

#[test]
fn settings_is_one_column_of_cards_with_the_usb_copy_shown() {
    assert!(
        super::APP_CSS.contains("nav[hidden] + main { grid-column: 1 / -1; }"),
        "with the steps' list hidden, Settings falls into the list's 260 px column (hand test, \
         2026-10-22)"
    );
    let html = super::INDEX_HTML;
    let settings = &html[html.find("data-page=\"settings\"").expect("settings")
        ..html.find("data-page=\"closed\"").expect("closed")];
    assert_eq!(
        settings.matches("<section class=\"card\"").count(),
        settings.matches("<h2").count(),
        "a heading of Settings is outside a card of its own"
    );
    let slot = &settings[settings
        .find("id=\"st-usb\"")
        .expect("the section \"Copy this account to a USB key\" is gone")..];
    let slot = &slot[..slot.find("</section>").expect("end of the section")];
    assert!(slot.contains("Copy this account to a USB key"));
    assert!(
        slot.starts_with("id=\"st-usb\" data-feature=\"usb_key\" aria-labelledby=\"st-h-usb\">"),
        "the USB copy section is hidden: the tray's \"Copy this account to a USB key\" opens \
         a page without it"
    );
    assert!(
        slot.contains("IS this account"),
        "the section copies an account without saying that whoever opens the key is it"
    );
}

/// The theme is in the first byte of the page, kept in the node's home for the
/// next page and the phone's own screens, and told to the shell: a theme set
/// only by a script would flash the other one first, and be lost at the next page.
/// Sabotage: drop the `data-theme` insertion in `index_html`; drop `save_theme`;
/// drop the `host.event`.
#[test]
fn red_team_a_chosen_theme_is_in_the_first_byte_and_survives_the_next_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    std::fs::create_dir_all(&home).expect("home");
    let told = Arc::new(Told::default());
    let page = Page::start_with(&home, dir.path(), Mode::Settings, &Seen::default(), |b| {
        b.host = told.clone();
    });
    let first = |page: &Page| {
        let host = page.host();
        page.send("GET", "/", &[("Host", &host)], "").body
    };
    assert!(
        !first(&page).contains("data-theme"),
        "a theme nobody chose is in the page"
    );
    let refused = page.api("POST", "/api/theme", "theme=purple");
    assert_eq!(refused.status, 400, "{}", refused.body);

    let dark = page.api("POST", "/api/theme", "theme=dark");
    assert_eq!(dark.status, 200, "{}", dark.body);
    assert!(
        first(&page).contains("<html data-theme=\"dark\" "),
        "the theme is not in the first byte of the page: the other one shows first"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(super::THEME_FILE)).expect("kept"),
        "dark\n",
        "the theme was not kept beside the node: the next page forgets it"
    );
    assert_eq!(
        *told.0.lock().expect("lock"),
        [("theme".to_owned(), "dark".to_owned())],
        "the shell was not told: a phone's own screens stay in the other theme"
    );
    drop(page);
    let next = Page::start(&home, dir.path(), Mode::Settings, &Seen::default());
    assert!(
        first(&next).contains("<html data-theme=\"dark\" "),
        "the next page started from the system's theme"
    );
    next.api("POST", "/api/theme", "theme=system");
    assert!(!first(&next).contains("data-theme"));

    let css = super::APP_CSS;
    assert!(
        css.contains(":root:not([data-theme=\"light\"])")
            && css.contains(":root[data-theme=\"dark\"]"),
        "the dark colours do not follow the system unless light is chosen, or apply when dark is"
    );
}

/// Nothing leaves the page but the three links of the legal block, each safe to
/// follow and each to a place that exists: no placeholder, no "coming soon".
/// Sabotage: drop a `rel`; add a link elsewhere.
#[test]
fn red_team_the_page_links_only_the_licence_the_source_and_the_security_policy() {
    let html = super::INDEX_HTML;
    let links: Vec<&str> = html.split("<a ").skip(1).collect();
    let external: Vec<&str> = links
        .iter()
        .copied()
        .filter(|link| link.contains("href=\"http"))
        .collect();
    assert_eq!(
        external.len(),
        3,
        "the legal block is not the licence, the source and the policy"
    );
    for link in external {
        let link = &link[..link.find('>').expect("a tag")];
        assert!(
            link.contains("rel=\"noopener noreferrer\"") && link.contains("target=\"_blank\""),
            "a link out of the page can read it back or is followed in the same tab: {link}"
        );
        assert!(
            link.contains("href=\"https://github.com/SigSegGit/itsanas"),
            "a link out of the page goes somewhere that is not the project: {link}"
        );
    }
    for placeholder in ["coming soon", "TODO", "lorem"] {
        assert!(
            !html.contains(placeholder),
            "the page carries a placeholder ({placeholder}) where a text should be"
        );
    }
}

/// The legal texts are in the program, reachable from the first screen and from
/// Settings, with no network. Sabotage: drop a route; drop a block from a screen.
#[test]
fn the_legal_texts_are_served_and_linked_from_both_screens() {
    for text in [
        super::LEGAL_NOTICE,
        super::LEGAL_PRIVACY,
        super::LEGAL_TERMS,
    ] {
        assert!(text.len() > 200, "a legal page is empty");
    }
    assert!(super::LEGAL_NOTICE.contains("Nicolas GIRARD"));
    let html = include_str!("../index.html");
    assert_eq!(
        html.matches("data-legal=\"/legal/privacy\"").count(),
        2,
        "the privacy policy must be one tap away on the first screen and in Settings"
    );
}
