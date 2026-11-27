//! A file read to its end only if it stayed what it was when opened.

use std::path::{Path, PathBuf};

/// A file read to its end, checked at the end against what it was when
/// opened: the same length and the same modification time, or the read fails
/// and says why. What `write_stream` takes is then a version of the file that
/// existed on disk, not the half of one a writer had reached.
pub(crate) struct Steady<R> {
    inner: R,
    real: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
    moved: std::rc::Rc<std::cell::Cell<bool>>,
}

impl<R> Steady<R> {
    pub(crate) fn new(inner: R, real: &Path, opened: &std::fs::Metadata) -> Self {
        Self {
            inner,
            real: real.to_owned(),
            len: opened.len(),
            modified: opened.modified().ok(),
            moved: std::rc::Rc::default(),
        }
    }

    /// Set once the end was reached and the file was no longer what it was.
    pub(crate) fn moved(&self) -> std::rc::Rc<std::cell::Cell<bool>> {
        std::rc::Rc::clone(&self.moved)
    }
}

impl<R: std::io::Read> std::io::Read for Steady<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read == 0 && !buf.is_empty() {
            let now = std::fs::metadata(&self.real)?;
            if now.len() != self.len || now.modified().ok() != self.modified {
                self.moved.set(true);
                return Err(std::io::Error::other(
                    "the file changed while it was read; it is read again on the next pass",
                ));
            }
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
    use itsanas_store::ChunkerConfig;
    use itsanas_store::Store;
    use std::io::{Read, Write};

    /// Appends to the file on disk after the first piece it hands over: a
    /// writer still at work while the pass reads.
    struct StillWritten {
        inner: std::fs::File,
        real: PathBuf,
        appended: bool,
    }

    impl Read for StillWritten {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let room = buf.len().min(4096);
            let read = self.inner.read(&mut buf[..room])?;
            if !self.appended {
                self.appended = true;
                let mut more = std::fs::OpenOptions::new().append(true).open(&self.real)?;
                more.write_all(&vec![7u8; 50_000])?;
                more.sync_all()?;
            }
            Ok(read)
        }
    }

    /// THE ATTACK, by accident: a database dumped, a large copy, read by a
    /// pass before its writer finished. What was taken was half a file, a
    /// version that never existed, sent to every machine until the next pass
    /// replaced it, and the only one left if this machine went first. The
    /// read now fails at its end when the file is not what it was opened as,
    /// and the store records nothing. Sabotage: drop the check at the end of
    /// `Steady::read`.
    #[test]
    fn red_team_a_file_still_being_written_is_not_taken_half_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open_for_testing(
            dir.path().join("store"),
            UserKeys::derive(&MasterSecret::from_bytes([0xA1; 32])),
            DeviceKeys::from_seed(&SecretBytes::new([1; 32])),
            ChunkerConfig::default(),
        )
        .expect("store");
        let real = dir.path().join("dump.db");
        std::fs::write(&real, vec![3u8; 200_000]).expect("first half");

        let file = std::fs::File::open(&real).expect("open");
        let opened = file.metadata().expect("metadata");
        let steady = Steady::new(
            StillWritten {
                inner: file,
                real: real.clone(),
                appended: false,
            },
            &real,
            &opened,
        );
        let moved = steady.moved();
        let taken = store.write_stream("dump.db", steady);

        assert!(
            taken.is_err(),
            "a file that grew while it was read was taken as it was half-way"
        );
        assert!(
            moved.get(),
            "the read failed for another reason than the file moving"
        );
        assert!(
            store.stat("dump.db").expect("stat").is_none(),
            "the half-read file is in the account: every machine gets a version that never existed"
        );

        let whole = std::fs::File::open(&real).expect("reopen");
        let opened = whole.metadata().expect("metadata");
        let again = store.write_stream("dump.db", Steady::new(whole, &real, &opened));
        assert_eq!(
            again.expect("a file no longer written is taken").size,
            250_000,
            "the next read did not take the whole file"
        );
    }
}
