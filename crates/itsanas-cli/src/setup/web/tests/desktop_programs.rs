//! The desktop around the page: whether there is one, and the programs that
//! open the browser, the folder picker and a bookmark.

use super::*;

#[test]
fn a_desktop_is_needed_for_the_page_and_ssh_never_counts_as_one() {
    for (os, ssh, display, expected, why) in [
        ("windows", false, false, true, "a Windows session"),
        ("macos", false, false, true, "a Mac"),
        ("linux", false, true, true, "a Linux desktop"),
        (
            "linux",
            false,
            false,
            false,
            "a Linux console with no display",
        ),
        ("windows", true, false, false, "Windows over SSH"),
        ("linux", true, true, false, "SSH with X forwarding"),
    ] {
        assert_eq!(
            has_desktop(os, ssh, display),
            expected,
            "{why}: a browser would open where nobody sees it, or the terminal would be skipped"
        );
    }
}

#[test]
fn the_browser_is_opened_by_the_systems_own_program_with_the_url_as_one_argument() {
    let url = "http://127.0.0.1:5000/#t=0123456789abcdef0123456789abcdef&x=1";
    for (os, program, before) in [
        ("windows", "rundll32", vec!["url.dll,FileProtocolHandler"]),
        ("macos", "open", vec![]),
        ("linux", "xdg-open", vec![]),
    ] {
        let command = browser_command(os, url);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let mut expected: Vec<String> = before.iter().map(|arg| (*arg).to_owned()).collect();
        expected.push(url.to_owned());
        assert_eq!(command.get_program().to_string_lossy(), program, "{os}");
        assert_eq!(
            args, expected,
            "on {os} the URL is not one whole argument: a `&` would cut it, and the page would \
             open without its key"
        );
    }
}

#[test]
fn red_team_a_chosen_path_never_lands_inside_a_script() {
    let start = Path::new(r"C:\x'; Remove-Item -Recurse C:\ #");
    for os in ["windows", "macos", "linux"] {
        let command = super::picker_command(os, start);
        let text: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let in_script = text
            .iter()
            .any(|arg| arg.contains("Remove-Item") && !arg.starts_with("--filename="));
        assert!(
            !in_script,
            "{os}: the start folder was written into the chooser's script, so a folder name \
             runs as code: {text:?}"
        );
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "ITSANAS_PICK_START" && value == Some(start.as_os_str())),
            "{os}: the start folder does not reach the chooser"
        );
    }
}

#[test]
fn a_gtk_bookmark_escapes_what_a_url_cannot_hold() {
    assert_eq!(
        super::gtk_bookmark(Path::new("/home/sam/My Files/été")),
        "file:///home/sam/My%20Files/%C3%A9t%C3%A9",
        "a space or an accent in the folder makes the bookmark point nowhere"
    );
}
