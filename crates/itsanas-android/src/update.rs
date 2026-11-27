//! The application's own update: whether the signed release offers a newer
//! APK, and whether the file downloaded is that APK.

use std::path::Path;

use itsanas_cli::shell::{Answer, Failure};

/// [`Java_fr_ngas_itsanas_Native_checkUpdate`] without the JNI.
pub(super) fn update_offer(
    manifest: &str,
    signature: &str,
    trust: &itsanas_release::Trust,
    running: itsanas_release::Version,
) -> Answer {
    let offered = itsanas_release::android_update(manifest.as_bytes(), signature, trust, running)
        .map_err(|error| release_failure(&error))?;
    let manifest =
        itsanas_release::Manifest::parse(manifest).map_err(|error| release_failure(&error))?;
    Ok(match offered {
        None => serde_json::json!({ "update": false }),
        Some(entry) => serde_json::json!({
            "update": true,
            "version": manifest.version.to_string(),
            "size": entry.size,
            "sha256": entry.sha256.iter().fold(String::new(), |mut hex, byte| {
                let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
                hex
            }),
        }),
    }
    .to_string())
}

/// [`Java_fr_ngas_itsanas_Native_checkApk`] without the JNI.
pub(super) fn apk_matches(
    manifest: &str,
    signature: &str,
    trust: &itsanas_release::Trust,
    running: itsanas_release::Version,
    path: &Path,
) -> Answer {
    let entry = itsanas_release::android_update(manifest.as_bytes(), signature, trust, running)
        .map_err(|error| release_failure(&error))?
        .ok_or_else(|| Failure::Usage("this release offers no newer application".to_owned()))?;
    entry
        .check_file(path)
        .map_err(|error| release_failure(&error))?;
    Ok("ok".to_owned())
}

pub(super) fn release_failure(error: &itsanas_release::ReleaseError) -> Failure {
    Failure::Usage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(
        version: &str,
        apk: Option<&[u8]>,
    ) -> (tempfile::TempDir, String, String, itsanas_release::Trust) {
        let dir = tempfile::tempdir().expect("scratch");
        std::fs::write(
            dir.path().join("itsanas-x86_64-unknown-linux-gnu"),
            b"linux".repeat(50),
        )
        .unwrap();
        if let Some(apk) = apk {
            std::fs::write(dir.path().join("itsanas-android-apk"), apk).unwrap();
        }
        let version = itsanas_release::Version::parse(version).unwrap();
        let manifest = itsanas_release::Manifest::from_dir(version, dir.path()).unwrap();
        let text = manifest.to_text();
        let key = itsanas_release::ReleaseKey::generate().unwrap();
        let (_, signature) = key.sign_manifest(text.as_bytes()).unwrap();
        let trust = itsanas_release::Trust::from_pinned(Some(key.public_bytes())).unwrap();
        (dir, text, signature, trust)
    }

    /// The application's own update, as the phone sees it: a newer signed
    /// release with an APK is offered with the hash the download must have,
    /// the downloaded file is accepted only when it is that APK, and a file
    /// changed after the check is refused. Sabotage: drop `check_file` in
    /// `apk_matches` -- red on the swapped file, which would be installed.
    #[test]
    fn red_team_the_phone_installs_only_the_apk_the_signed_release_names() {
        let running = itsanas_release::Version::parse("0.1.13").unwrap();
        let apk = b"PK the application".repeat(200);
        let (dir, text, signature, trust) = signed("0.1.14", Some(&apk));
        let offer: serde_json::Value =
            serde_json::from_str(&update_offer(&text, &signature, &trust, running).unwrap())
                .unwrap();
        assert_eq!(offer["update"], true);
        assert_eq!(offer["version"], "0.1.14");
        assert_eq!(offer["size"], apk.len());

        let download = dir.path().join("download");
        std::fs::write(&download, &apk).unwrap();
        assert_eq!(
            apk_matches(&text, &signature, &trust, running, &download).unwrap(),
            "ok"
        );

        std::fs::write(&download, b"PK something else".repeat(200)).unwrap();
        assert!(
            apk_matches(&text, &signature, &trust, running, &download).is_err(),
            "a file that is not the signed APK was accepted for install"
        );

        let (_d, text, signature, trust) = signed("0.1.13", Some(&apk));
        let offer = update_offer(&text, &signature, &trust, running).unwrap();
        assert_eq!(
            offer, r#"{"update":false}"#,
            "the running version offered as an update"
        );
        let other = itsanas_release::ReleaseKey::generate().unwrap();
        let stranger = itsanas_release::Trust::from_pinned(Some(other.public_bytes())).unwrap();
        assert!(
            update_offer(&text, &signature, &stranger, running).is_err(),
            "a release signed by another key read as up to date or as an update"
        );
    }
}
