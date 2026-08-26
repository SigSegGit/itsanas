//! Prints the published test-user fixtures and their pinned digests.
//!
//! Everything here is derived deterministically from constants in
//! `itsanas-testkit`, so running this on any machine must produce byte-identical
//! output. That is what makes the values safe to publish in the documentation
//! and to pin in source. Run with:
//!
//! ```text
//! cargo run -p itsanas-testkit --bin generate-fixtures
//! ```

use itsanas_testkit::{CORPUS_DIGEST, corpus_digest, everyone};

fn main() {
    for user in everyone() {
        let public = user.keys.public();

        println!("=== {} ===", user.username);
        println!("recovery phrase  {}", user.recovery_phrase);
        println!("master secret    {}", hex(user.master.expose()));
        println!("user id          {}", user.keys.user_id());
        println!("agreement key    {}", hex(&public.agreement));
        println!("canary           {}", user.canary);
        println!("plaintext bytes  {}", user.plaintext_bytes());
        println!("files:");
        for file in &user.files {
            println!(
                "  {:<28} {:>9} bytes  {}",
                file.path,
                file.content.len(),
                file.actual_digest()
            );
        }
        println!();
    }

    let actual = corpus_digest();
    println!("corpus digest    {actual}");
    if actual == CORPUS_DIGEST {
        println!("pinned digest    matches");
    } else {
        println!("pinned digest    {CORPUS_DIGEST}  <-- STALE, update CORPUS_DIGEST");
    }
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}
