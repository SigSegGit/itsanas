//! `itsanas-release`: make the release key once, write a manifest, sign it,
//! check it.
//!
//! Nicolas normally never types these: `scripts/sign-release.*` runs them, and
//! the release workflow runs `manifest`. Every failure is one line on stderr
//! saying what to do; the passphrase is read hidden from the terminal, never
//! from an argument or the environment, and never printed.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use itsanas_release::{
    Manifest, RELEASE_KEY, ReleaseError, ReleaseKey, Trust, Version, verify_release, verify_signed,
};
use zeroize::Zeroizing;

/// A passphrase shorter than this is refused at keygen. It guards every
/// member's machine, so it gets the bar the account passphrase gets.
const MIN_PASSPHRASE_CHARS: usize = 12;

#[derive(Parser)]
#[command(
    name = "itsanas-release",
    version,
    about = "Make, sign and check ITSaNAS releases"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the release signing key, sealed under a passphrase (once, ever)
    Keygen {
        /// Where to write the sealed key file; refused if it already exists
        #[arg(long)]
        out: PathBuf,
    },
    /// Write manifest.txt for the binaries itsanas-<target>[.exe] in a directory
    Manifest {
        /// The release version, e.g. 0.2.0 (a leading v is dropped)
        #[arg(long)]
        version: String,
        /// The directory holding the binaries
        #[arg(long)]
        dir: PathBuf,
        /// Where to write it (default: <dir>/manifest.txt)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Sign a manifest with the key file; writes <manifest>.sig
    Sign {
        manifest: PathBuf,
        /// The sealed key file keygen wrote
        #[arg(long)]
        key: PathBuf,
    },
    /// Check a manifest's signature
    Verify {
        manifest: PathBuf,
        signature: PathBuf,
        /// Public key to check against (default: the one pinned in this build)
        #[arg(long)]
        key: Option<String>,
        /// Also refuse it unless it is newer than this version
        #[arg(long)]
        running: Option<String>,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Keygen { out } => keygen(&out),
        Command::Manifest { version, dir, out } => manifest(&version, &dir, out),
        Command::Sign { manifest, key } => sign(&manifest, &key),
        Command::Verify {
            manifest,
            signature,
            key,
            running,
        } => verify(&manifest, &signature, key.as_deref(), running.as_deref()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, ReleaseError> {
    fs::read(path).map_err(|e| ReleaseError::Io(format!("could not read {}: {e}", path.display())))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), ReleaseError> {
    fs::write(path, bytes)
        .map_err(|e| ReleaseError::Io(format!("could not write {}: {e}", path.display())))
}

fn ask(prompt: &str) -> Result<Zeroizing<String>, ReleaseError> {
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|_| {
            ReleaseError::Io(
                "no terminal to ask the passphrase on: run this in a terminal window".into(),
            )
        })
}

fn keygen(out: &Path) -> Result<(), ReleaseError> {
    // Checked before asking anything, and again by create_new below: replacing
    // an existing key file would destroy the only key every node trusts.
    if out.exists() {
        return Err(ReleaseError::Io(format!(
            "{} already exists and may be your release key: refusing to replace it",
            out.display()
        )));
    }
    println!(
        "Choose a passphrase for the release key (at least {MIN_PASSPHRASE_CHARS} characters)."
    );
    println!("It is asked every time you publish a version. It is not shown as you type.");
    let first = ask("Passphrase: ")?;
    if first.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(ReleaseError::Io(format!(
            "that passphrase is shorter than {MIN_PASSPHRASE_CHARS} characters: run it again with a longer one"
        )));
    }
    let second = ask("Same passphrase again: ")?;
    if *first != *second {
        return Err(ReleaseError::Io(
            "the two passphrases differ: run it again".into(),
        ));
    }
    let key = ReleaseKey::generate()?;
    let sealed = key.seal(&first)?;
    write_new(out, &sealed)?;
    print_keygen_advice(out, &key.public_hex());
    Ok(())
}

/// Create the key file, never replacing one, readable by its owner only where
/// the system has such a notion (Windows keeps a profile's files private
/// already). The contents are sealed either way.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ReleaseError> {
    let io =
        |e: std::io::Error| ReleaseError::Io(format!("could not write {}: {e}", path.display()));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}

fn print_keygen_advice(out: &Path, public: &str) {
    println!();
    println!("Release key created: {}", out.display());
    println!("Public key: {public}");
    println!();
    println!("Before anything else:");
    println!(
        "  1. Copy {} to a USB stick and put the stick in a drawer,",
        out.display()
    );
    println!("     away from this PC. That copy is what saves you if this disk dies.");
    println!("  2. Write the passphrase on paper and keep it somewhere else than the stick.");
    println!("  3. Paste the public key above into RELEASE_KEY in");
    println!("     crates/itsanas-release/src/lib.rs, in a pull request, so nodes trust it.");
    println!();
    println!("Lose both copies of the file, or the passphrase, and no node can be updated");
    println!("by a signed release again until it is reinstalled by hand.");
}

fn manifest(version: &str, dir: &Path, out: Option<PathBuf>) -> Result<(), ReleaseError> {
    let version = Version::parse(version.strip_prefix('v').unwrap_or(version))?;
    let manifest = Manifest::from_dir(version, dir)?;
    let out = out.unwrap_or_else(|| dir.join("manifest.txt"));
    write(&out, manifest.to_text().as_bytes())?;
    println!("Wrote {} for release {version}:", out.display());
    for file in &manifest.files {
        println!("  {} ({} bytes)", file.name, file.size);
    }
    Ok(())
}

fn sign(manifest_path: &Path, key_path: &Path) -> Result<(), ReleaseError> {
    let bytes = read(manifest_path)?;
    let sealed = read(key_path)?;
    let passphrase = ask("Release key passphrase: ")?;
    let key = ReleaseKey::unseal(&sealed, &passphrase)?;
    // A signature by a key the nodes do not trust would publish a release
    // every node refuses. Say so now, while nothing is uploaded.
    if let Some(pinned) = RELEASE_KEY
        && pinned != key.public_bytes()
    {
        return Err(ReleaseError::Io(format!(
            "{} is not the release key this checkout trusts: use the right key file",
            key_path.display()
        )));
    }
    let (manifest, signature) = key.sign_manifest(&bytes)?;
    let sig_path = PathBuf::from(format!("{}.sig", manifest_path.display()));
    write(&sig_path, signature.as_bytes())?;
    println!(
        "Signed release {} ({}).",
        manifest.version,
        targets(&manifest)
    );
    println!("Signature: {}", sig_path.display());
    if RELEASE_KEY.is_none() {
        println!("Note: no release key is pinned in this checkout yet; nodes will not trust");
        println!(
            "this signature until the public key {} is pasted into RELEASE_KEY.",
            key.public_hex()
        );
    }
    Ok(())
}

fn targets(manifest: &Manifest) -> String {
    manifest
        .files
        .iter()
        .map(|f| f.target.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn verify(
    manifest_path: &Path,
    signature_path: &Path,
    key: Option<&str>,
    running: Option<&str>,
) -> Result<(), ReleaseError> {
    let bytes = read(manifest_path)?;
    let signature =
        String::from_utf8(read(signature_path)?).map_err(|_| ReleaseError::NotSigned)?;
    let trust = match key {
        Some(hex) => Trust::from_hex(hex)?,
        None => Trust::pinned()?,
    };
    let manifest = match running {
        Some(running) => verify_release(&bytes, &signature, &trust, Version::parse(running)?)?,
        None => verify_signed(&bytes, &signature, &trust)?,
    };
    println!(
        "Good signature: release {} ({}).",
        manifest.version,
        targets(&manifest)
    );
    let this = Version::running();
    if manifest.version > this {
        println!("It is newer than this build ({this}).");
    } else {
        println!(
            "It is not newer than this build ({this}); a node running {this} would not install it."
        );
    }
    Ok(())
}
