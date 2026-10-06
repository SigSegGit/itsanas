//! `itsanas setup --answers`, run twice on a throwaway home, for real.
//!
//! The engine's unit tests (`src/setup/tests.rs`) drive `Setup` with a
//! scripted person. This one drives the binary a provisioning script would:
//! the answers file parsed, the passphrase from `ITSANAS_PASSPHRASE`, the
//! words printed once, and -- the point of Nicolas's "resilient install" -- a
//! second run that finds everything done, shows no new words, and leaves the
//! keys byte for byte as they were.
//!
//! # Why this is `#[ignore]`d
//!
//! Each run derives the keystore key (64 MiB of Argon2id, slow on purpose)
//! several times; the release job of ignored tests pays seconds for it.
//! `service = false`: a test never installs a real service or scheduled task.
//!
//! # What it does not cover
//!
//! The service, the tray and the native windows (`--text`): those need a
//! desktop session and a service manager, which no CI job has.

use std::{
    path::Path,
    process::{Command, Stdio},
};

const PASSPHRASE: &str = "setup-integration-passphrase";

fn setup(home: &Path, answers: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_itsanas"))
        .arg("--home")
        .arg(home)
        .arg("setup")
        .arg("--answers")
        .arg(answers)
        .env("ITSANAS_PASSPHRASE", PASSPHRASE)
        .env_remove("ITSANAS_INSTANCE")
        .stdin(Stdio::null())
        .output()
        .expect("run itsanas setup");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "a setup from a complete answers file failed, so a provisioning script would stop \
         half way:\n{printed}"
    );
    printed
}

/// The 24 words as the first run printed them: the numbered grid, `N. word`.
fn words_printed(printed: &str) -> Vec<String> {
    let tokens: Vec<&str> = printed.split_whitespace().collect();
    tokens
        .windows(2)
        .filter(|pair| {
            pair[0]
                .strip_suffix('.')
                .and_then(|number| number.parse::<usize>().ok())
                .is_some_and(|number| (1..=24).contains(&number))
                && pair[1].bytes().all(|b| b.is_ascii_lowercase())
        })
        .map(|pair| pair[1].to_owned())
        .collect()
}

#[test]
#[ignore = "derives the keystore key several times: run with the release ignored tests"]
fn setup_run_twice_from_answers_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let folder = dir.path().join("folder");
    let answers = dir.path().join("answers.toml");
    std::fs::write(
        &answers,
        format!(
            "# what a provisioning script would write\n\
             folder = '{}'\n\
             account = \"new\"\n\
             username = \"camille\"\n\
             pledge = \"10M\"\n\
             service = false\n\
             tray = false\n\
             verify_seconds = 5\n",
            folder.display()
        ),
    )
    .expect("answers");

    let first = setup(&home, &answers);
    let words = words_printed(&first);
    assert_eq!(
        words.len(),
        24,
        "the first run did not print the 24 words once, so the account it made could never be \
         restored:\n{first}"
    );
    assert!(folder.is_dir(), "the folder to sync was not created");
    let keystore = std::fs::read(home.join("keystore.bin")).expect("keystore");
    let config = std::fs::read_to_string(home.join("config")).expect("config");

    let second = setup(&home, &answers);
    assert!(
        words_printed(&second).is_empty(),
        "the second run showed recovery words again: a person would write down words of an \
         account that is not the one on this machine:\n{second}"
    );
    assert!(
        second.contains("already done"),
        "the second run did not find the steps done:\n{second}"
    );
    assert_eq!(
        std::fs::read(home.join("keystore.bin")).expect("keystore"),
        keystore,
        "the second run rewrote the keystore: this machine's keys, and so its device id, \
         changed under a setup that had nothing to do"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("config")).expect("config"),
        config,
        "the second run changed the configuration although every answer was the same"
    );

    let log = std::fs::read_to_string(home.join("setup.log")).expect("setup.log");
    let grid_line = words[..4].join(" ");
    assert!(
        !words.windows(2).any(|pair| log.contains(&pair.join(" "))) && !log.contains(&grid_line),
        "setup.log holds the recovery words: anyone sent the log for help could take the \
         account"
    );
}
