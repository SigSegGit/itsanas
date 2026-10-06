//! What a node must refuse before it replaces its own binary, and what it must
//! accept so the refusals are not a release process that refuses everything.

use std::fs;
use std::path::PathBuf;

use itsanas_release::{
    FileEntry, Manifest, RELEASE_KEY, ReleaseError, ReleaseKey, Trust, Version, verify_release,
    verify_signed,
};

/// A temporary directory removed on drop, so no test leaves files behind.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce).expect("randomness");
        let dir = std::env::temp_dir().join(format!(
            "itsanas-release-test-{tag}-{}",
            u64::from_le_bytes(nonce)
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn v(text: &str) -> Version {
    Version::parse(text).expect("test version")
}

/// A release directory with two fake binaries, its manifest, and the key that
/// signed it.
struct Release {
    dir: Scratch,
    key: ReleaseKey,
    manifest: Manifest,
    text: Vec<u8>,
    signature: String,
}

fn release(version: &str) -> Release {
    let dir = Scratch::new("release");
    fs::write(
        dir.0.join("itsanas-x86_64-unknown-linux-gnu"),
        b"linux binary bytes, not really a program".repeat(100),
    )
    .unwrap();
    fs::write(
        dir.0.join("itsanas-x86_64-pc-windows-msvc.exe"),
        b"windows binary bytes".repeat(50),
    )
    .unwrap();
    let manifest = Manifest::from_dir(v(version), &dir.0).expect("manifest from the directory");
    let text = manifest.to_text().into_bytes();
    let key = ReleaseKey::generate().unwrap();
    let (signed, signature) = key.sign_manifest(&text).expect("signing a good manifest");
    assert_eq!(signed, manifest);
    Release {
        dir,
        key,
        manifest,
        text,
        signature,
    }
}

fn trust(key: &ReleaseKey) -> Trust {
    Trust::from_pinned(Some(key.public_bytes())).unwrap()
}

fn binary(release: &Release) -> (&FileEntry, PathBuf) {
    let entry = release
        .manifest
        .file_for("x86_64-unknown-linux-gnu")
        .expect("the linux binary is in the manifest");
    (entry, release.dir.0.join(&entry.name))
}

#[test]
fn a_release_signed_by_the_trusted_key_is_accepted_end_to_end() {
    let r = release("0.2.0");
    let got = verify_release(&r.text, &r.signature, &trust(&r.key), v("0.1.0"))
        .expect("a release signed by the trusted key, newer than the running one, must install, or no tester ever gets an update");
    assert_eq!(got, r.manifest);
    assert_eq!(got.files.len(), 2);
    for entry in &got.files {
        entry.check_file(&r.dir.0.join(&entry.name)).expect(
            "an intact download must pass its size and hash checks, or every update is refused",
        );
    }
    assert_eq!(Manifest::parse(&got.to_text()).unwrap(), got);
}

#[test]
fn red_team_a_manifest_signed_by_another_key_is_refused() {
    let r = release("0.2.0");
    let stranger = ReleaseKey::generate().unwrap();
    let (_, forged) = stranger.sign_manifest(&r.text).unwrap();
    assert_eq!(
        verify_release(&r.text, &forged, &trust(&r.key), v("0.1.0")),
        Err(ReleaseError::NotSigned),
        "a release signed by someone else's key was accepted: anyone could push a binary onto every member's machine"
    );
    assert_eq!(
        verify_signed(&r.text, "not a signature", &trust(&r.key)),
        Err(ReleaseError::NotSigned),
        "a garbage signature file was not refused as unsigned"
    );
}

#[test]
fn red_team_one_changed_byte_in_a_signed_manifest_is_refused() {
    let r = release("0.2.0");
    let trusted = trust(&r.key);
    // Every byte, not one chosen byte: a check that covered the hashes and not
    // the version line, or the reverse, would pass a sample and fail here.
    for i in 0..r.text.len() {
        let mut changed = r.text.clone();
        changed[i] ^= 0x01;
        assert!(
            verify_signed(&changed, &r.signature, &trusted).is_err(),
            "byte {i} of a signed manifest was changed and it was still accepted: an attacker could swap in another binary's hash under Nicolas's signature"
        );
    }
    let mut longer = r.text.clone();
    longer.extend_from_slice(
        b"next-key 0000000000000000000000000000000000000000000000000000000000000000\n",
    );
    assert_eq!(
        verify_signed(&longer, &r.signature, &trusted),
        Err(ReleaseError::NotSigned),
        "a line appended to a signed manifest was accepted: an attacker could name their own next key"
    );
}

#[test]
fn red_team_an_older_or_equal_version_is_refused_as_a_downgrade() {
    let r = release("0.2.0");
    let trusted = trust(&r.key);
    for running in ["0.2.0", "0.3.0", "0.10.0", "1.0.0"] {
        assert_eq!(
            verify_release(&r.text, &r.signature, &trusted, v(running)),
            Err(ReleaseError::NotNewer {
                offered: v("0.2.0"),
                running: v(running)
            }),
            "a node running {running} accepted the correctly signed 0.2.0: an old release with a known hole could be replayed onto every node"
        );
    }
    assert!(
        verify_release(&r.text, &r.signature, &trusted, v("0.1.9")).is_ok(),
        "a node running 0.1.9 refused 0.2.0: nobody would ever be updated"
    );
}

#[test]
fn red_team_a_file_whose_size_or_hashes_do_not_match_is_refused() {
    let r = release("0.2.0");
    let (entry, path) = binary(&r);

    let mut wrong_blake = entry.clone();
    wrong_blake.blake3[0] ^= 1;
    let mut wrong_sha = entry.clone();
    wrong_sha.sha256[31] ^= 1;
    for (what, bad) in [("BLAKE3", wrong_blake), ("SHA-256", wrong_sha)] {
        assert_eq!(
            bad.check_file(&path),
            Err(ReleaseError::WrongContent {
                name: entry.name.clone()
            }),
            "a file whose {what} differs from the signed one was accepted: a swapped binary would be installed"
        );
    }

    // Same length, different bytes: the size check cannot catch this one.
    let mut swapped = fs::read(&path).unwrap();
    swapped[10] ^= 0xff;
    fs::write(&path, &swapped).unwrap();
    assert_eq!(
        entry.check_file(&path),
        Err(ReleaseError::WrongContent {
            name: entry.name.clone()
        }),
        "a binary with one byte changed was accepted: a tampered download would be installed"
    );

    // Longer than signed: a payload appended to a real binary.
    swapped[10] ^= 0xff;
    swapped.extend_from_slice(b"appended");
    fs::write(&path, &swapped).unwrap();
    assert!(
        matches!(entry.check_file(&path), Err(ReleaseError::WrongSize { found, expected, .. }) if found == expected + 8),
        "a download longer than the signed binary was not refused for its size"
    );
}

#[test]
fn red_team_a_truncated_download_is_refused() {
    let r = release("0.2.0");
    let (entry, path) = binary(&r);
    let whole = fs::read(&path).unwrap();
    for keep in [whole.len() - 1, whole.len() / 2, 0] {
        fs::write(&path, &whole[..keep]).unwrap();
        let refused = entry.check_file(&path);
        assert_eq!(
            refused,
            Err(ReleaseError::WrongSize {
                name: entry.name.clone(),
                expected: whole.len() as u64,
                found: keep as u64
            }),
            "a download cut at {keep} of {} bytes was not refused as incomplete: a half binary would replace a working one, or the tester is told it is forged",
            whole.len()
        );
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("download it again"),
            "the refusal of a truncated download does not tell the tester what to do"
        );
    }
}

#[test]
fn red_team_the_key_file_with_a_wrong_passphrase_is_refused() {
    let key = ReleaseKey::generate().unwrap();
    let passphrase = "correct horse battery staple";
    let sealed = key.seal(passphrase).unwrap();

    let opened = ReleaseKey::unseal(&sealed, passphrase)
        .expect("the right passphrase opens the key file, or Nicolas can never sign again");
    assert_eq!(opened.public_bytes(), key.public_bytes());

    let wrong = ReleaseKey::unseal(&sealed, "correct horse battery stapler");
    assert!(
        matches!(wrong, Err(ReleaseError::WrongPassphrase)),
        "a wrong passphrase opened the release key: whoever copies the file can sign releases"
    );
    let mut tampered = sealed.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(
        matches!(
            ReleaseKey::unseal(&tampered, passphrase),
            Err(ReleaseError::WrongPassphrase)
        ),
        "a damaged key file was opened as if intact"
    );
    assert!(
        matches!(
            ReleaseKey::unseal(&sealed[..10], passphrase),
            Err(ReleaseError::WrongPassphrase)
        ),
        "a truncated key file was not refused"
    );

    // One plain line, no crypto words: Nicolas reads this at 11 pm before a release.
    let message = ReleaseError::WrongPassphrase.to_string();
    for jargon in ["AEAD", "decrypt", "Argon", "tag", "\n"] {
        assert!(
            !message.contains(jargon),
            "the wrong-passphrase message says '{jargon}': '{message}' does not tell a person what to do"
        );
    }
    // The file holds neither the passphrase nor the secret in clear.
    let pass = passphrase.as_bytes();
    assert!(
        !sealed.windows(pass.len()).any(|w| w == pass),
        "the passphrase is written in the key file"
    );
}

#[test]
fn red_team_with_no_release_key_pinned_every_manifest_is_refused() {
    let refused = Trust::from_pinned(None);
    assert!(
        matches!(refused, Err(ReleaseError::NoKeyPinned)),
        "a build with no release key pinned produced a trust set: it would install whatever a download claims, or fail later as a forgery and send a tester hunting"
    );
    assert_eq!(
        refused.unwrap_err().to_string().lines().count(),
        1,
        "the no-key refusal is not one line"
    );
    // And this build, today, has none: every verification through the pinned
    // key refuses, whatever is signed by whom.
    if RELEASE_KEY.is_none() {
        assert!(
            matches!(Trust::pinned(), Err(ReleaseError::NoKeyPinned)),
            "this build pins no key yet accepted a release key: a self-update would trust nothing in particular"
        );
    }
}

#[test]
fn the_release_key_is_pinned_until_nicolas_changes_it_on_purpose() {
    // Changing this value changes which releases every node installs. It must
    // be a deliberate line in a reviewed diff: Nicolas pastes the public key
    // keygen printed, and updates this test in the same pull request.
    assert_eq!(
        RELEASE_KEY, None,
        "RELEASE_KEY changed: if this is not Nicolas pasting his own public key, every node would trust someone else's releases"
    );
}

#[test]
fn a_next_key_named_by_a_signed_manifest_is_trusted_once_learned() {
    let old = ReleaseKey::generate().unwrap();
    let new = ReleaseKey::generate().unwrap();
    let r = release("0.2.0");
    let mut rotation = r.manifest.clone();
    rotation.next_key = Some(new.public_bytes());
    let text = rotation.to_text();
    let (_, sig) = old.sign_manifest(text.as_bytes()).unwrap();

    let mut trusted = trust(&old);
    let seen = verify_signed(text.as_bytes(), &sig, &trusted).unwrap();
    assert_eq!(seen.next_key, Some(new.public_bytes()));

    let mut later = r.manifest.clone();
    later.version = v("0.3.0");
    let later_text = later.to_text();
    let (_, later_sig) = new.sign_manifest(later_text.as_bytes()).unwrap();
    assert_eq!(
        verify_signed(later_text.as_bytes(), &later_sig, &trusted),
        Err(ReleaseError::NotSigned),
        "a new key was trusted before any signed manifest named it"
    );
    trusted.learn(&seen).unwrap();
    assert!(
        verify_signed(later_text.as_bytes(), &later_sig, &trusted).is_ok(),
        "after a rotation signed by the old key, releases signed by the new one are refused: nodes stop updating"
    );
}

#[test]
fn a_manifest_names_only_files_derived_from_their_target() {
    let r = release("0.2.0");
    let text = String::from_utf8(r.text.clone()).unwrap();
    let evil = text.replace("itsanas-x86_64-unknown-linux-gnu ", "../../.profile ");
    assert!(
        matches!(Manifest::parse(&evil), Err(ReleaseError::Damaged(_))),
        "a manifest could name the file it is saved as: a signed release could write anywhere on a tester's disk"
    );
    assert!(
        r.key.sign_manifest(b"hello").is_err(),
        "the release key signed bytes that are not a manifest"
    );
}

#[test]
fn versions_compare_as_numbers_not_as_text() {
    assert!(
        v("0.10.0") > v("0.9.0"),
        "0.10.0 sorted before 0.9.0: nodes would refuse the tenth minor release"
    );
    assert!(
        v("1.0.0") > v("0.99.99") && v("0.2.0") > v("0.1.99"),
        "a higher patch or minor outranked a higher major or minor: a node would call an old release newer and install it"
    );
    for bad in ["v0.2.0", "0.2", "0.2.0.1", "0.+2.0", "0..0", ""] {
        assert!(
            Version::parse(bad).is_err(),
            "'{bad}' parsed as a version: a node could misjudge what is newer"
        );
    }
    assert_eq!(Version::running().to_string(), env!("CARGO_PKG_VERSION"));
}
