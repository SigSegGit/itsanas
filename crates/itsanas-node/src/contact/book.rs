//! The address book on disk (`<home>/address-book`): its two versions, and
//! loading and saving a [`Contact`](super::Contact) across a restart.

use std::path::{Path, PathBuf};

use itsanas_coord::claim::{ClaimedPresence, SignedClaim, SignedPresence};
use itsanas_crypto::UserId;
use serde::{Deserialize, Serialize};

use super::Contact;
use crate::{NodeError, Result};

/// The address book's version. A file of another version is read as empty,
/// except version 1, which is version 2 with no claims and is read as such:
/// dropping it would forget that the coordinator signs, and re-open the
/// downgrade once on the upgrade.
const BOOK_VERSION: u32 = 2;

/// The address book as written to `<home>/address-book`.
#[derive(Serialize, Deserialize)]
struct BookFile {
    version: u32,
    signs: bool,
    entries: Vec<BookEntry>,
}

#[derive(Serialize, Deserialize)]
struct BookEntry {
    presence: SignedPresence,
    worked: Option<u64>,
    claim: Option<SignedClaim>,
}

/// Version 1 of the file, before claims.
#[derive(Serialize, Deserialize)]
struct BookFileV1 {
    version: u32,
    signs: bool,
    entries: Vec<BookEntryV1>,
}

#[derive(Serialize, Deserialize)]
struct BookEntryV1 {
    presence: SignedPresence,
    worked: Option<u64>,
}

impl From<BookFileV1> for BookFile {
    fn from(old: BookFileV1) -> Self {
        Self {
            version: BOOK_VERSION,
            signs: old.signs,
            entries: old
                .entries
                .into_iter()
                .map(|entry| BookEntry {
                    presence: entry.presence,
                    worked: entry.worked,
                    claim: None,
                })
                .collect(),
        }
    }
}

impl Contact {
    /// Where the address book lives under a node's home.
    #[must_use]
    pub fn path(home: &Path) -> PathBuf {
        home.join("address-book")
    }

    /// The contact a daemon starts with: the address book at `path`, or an
    /// empty one.
    ///
    /// Every entry goes through the door the coordinator's list goes through:
    /// its signature is checked ([`SignedPresence::verify_origin`]) and
    /// [`MAX_DEVICES`] and [`MAX_ADDRESSES`] bound what is kept. The file sits
    /// on disk, where anything with this user's rights can edit it, and an
    /// address in it is dialled before the coordinator is asked anything.
    ///
    /// A claim is checked against `owner`, this node's account
    /// ([`ClaimedPresence::verify_for`]); one that fails is dropped and the
    /// address kept, dialled but never relayed.
    ///
    /// What this does not defend: somebody with this user's rights can also set
    /// `signs` false or mark a genuine but stale address as having worked. Such
    /// a person can rewrite the configuration too; the check is against an
    /// address nobody signed, not against the owner of the home directory.
    ///
    /// A missing file is a first start. An unreadable one is an empty book and
    /// a sentence in the second value, never a failed start: the book is a
    /// cache, and the coordinator refills it on the first round.
    ///
    /// [`MAX_ADDRESSES`]: super::MAX_ADDRESSES
    /// [`MAX_DEVICES`]: super::MAX_DEVICES
    #[must_use]
    pub fn load(path: &Path, owner: UserId) -> (Self, Option<String>) {
        let mut contact = Self::new();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (contact, None),
            Err(error) => {
                return (
                    contact,
                    Some(format!(
                        "could not read {}: {error}; starting with an empty address book",
                        path.display()
                    )),
                );
            }
        };
        // The version comes first in every version of the file, so it can be
        // read before knowing the shape of the rest.
        let decoded = match postcard::take_from_bytes::<u32>(&bytes) {
            Ok((1, _)) => postcard::from_bytes::<BookFileV1>(&bytes).map(BookFile::from),
            _ => postcard::from_bytes::<BookFile>(&bytes),
        };
        let file: BookFile = match decoded {
            Ok(file) if file.version == BOOK_VERSION => file,
            Ok(file) => {
                return (
                    contact,
                    Some(format!(
                        "{} is version {}, this node reads {BOOK_VERSION}; starting with an empty address book",
                        path.display(),
                        file.version
                    )),
                );
            }
            Err(error) => {
                return (
                    contact,
                    Some(format!(
                        "{} is damaged ({error}); starting with an empty address book",
                        path.display()
                    )),
                );
            }
        };
        contact.signs = file.signs;
        let offered = file.entries.len();
        let mut forged = 0usize;
        let mut foreign = 0usize;
        for entry in file.entries {
            if entry.presence.verify_origin().is_err() {
                forged += 1;
                continue;
            }
            let claim = entry.claim.filter(|claim| {
                let kept = ClaimedPresence {
                    presence: entry.presence.clone(),
                    claim: claim.clone(),
                }
                .verify_for(owner)
                .is_ok();
                foreign += usize::from(!kept);
                kept
            });
            let device = entry.presence.presence.device;
            let address = entry.presence.presence.address.clone();
            contact.insert(device, &address, Some(entry.presence), claim, entry.worked);
        }
        contact.changed = false;
        let mut problems = Vec::new();
        if forged > 0 {
            problems.push(format!(
                "{forged} of {offered} addresses their device never signed, dropped"
            ));
        }
        if foreign > 0 {
            problems.push(format!(
                "{foreign} claims that are not a live device of this account, dropped \
                 (their addresses are kept and never relayed)"
            ));
        }
        let warning = (!problems.is_empty())
            .then(|| format!("{} held {}", path.display(), problems.join("; ")));
        (contact, warning)
    }

    /// Write the address book to `path` if anything in it changed since it was
    /// loaded or last saved. Returns whether it wrote.
    ///
    /// Only signed addresses are written: an unsigned one is on an old
    /// coordinator's word, and a file is no place to launder it into
    /// something [`Self::load`] would have to take on trust. Written to a
    /// temporary file and renamed over the old one, so a power cut leaves the
    /// old book or the new one, never an empty file.
    ///
    /// # Errors
    ///
    /// If the file cannot be written or renamed. The book stays marked
    /// changed, so the next round tries again.
    pub fn save(&mut self, path: &Path) -> Result<bool> {
        if !self.changed {
            return Ok(false);
        }
        let entries = self
            .book
            .values()
            .flatten()
            .filter_map(|candidate| {
                candidate.presence.clone().map(|presence| BookEntry {
                    presence,
                    worked: candidate.worked,
                    claim: candidate.claim.clone(),
                })
            })
            .collect();
        let bytes = postcard::to_stdvec(&BookFile {
            version: BOOK_VERSION,
            signs: self.signs,
            entries,
        })?;
        let temporary = path.with_extension("tmp");
        let io = |error| NodeError::Io {
            path: path.to_owned(),
            source: error,
        };
        // Synced before the rename: without it a crash can leave the new name
        // pointing at data that never reached the disk.
        let mut file = std::fs::File::create(&temporary).map_err(io)?;
        std::io::Write::write_all(&mut file, &bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(io)?;
        self.changed = false;
        Ok(true)
    }
}

/// The address book's bytes, both versions, pinned (HANDOVER §8 17): #435
/// changed a presence and every laptop lost its book on the upgrade, found by
/// chance in a log. A version whose figure moves needs a reader for the old
/// file first (as `BookFileV1` is), then a row; a shipped row is never edited.
#[cfg(test)]
mod disk_bytes {
    use itsanas_coord::claim::{NodeClaim, Presence};
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

    use super::{BookEntry, BookEntryV1, BookFile, BookFileV1};

    const PINNED: &[(&str, u64)] = &[
        ("BookFile v2", 4_736_550_793_510_739_214),
        ("BookFile v1", 1_760_494_900_918_826_549),
    ];

    #[test]
    fn red_team_the_address_book_keeps_its_bytes() {
        // Sabotage: reorder `BookEntry`'s `worked` and `claim`.
        let owner = UserKeys::derive(&MasterSecret::from_bytes([1; 32]));
        let keys = DeviceKeys::from_seed(&SecretBytes::new([2; 32]));
        let presence = Presence {
            device: keys.device_id(),
            address: "a:1".to_owned(),
            at_unix: 1_700_000_001,
            head: 4,
        }
        .sign(&keys);
        let claim = NodeClaim {
            owner: owner.user_id(),
            device: keys.device_id(),
            pledged_bytes: 3 << 30,
            issued_unix: 1_700_000_000,
            revoked: false,
        }
        .sign(&owner);
        let fold = |bytes: Vec<u8>| itsanas_testkit::fold(&bytes);
        let found = [
            (
                "BookFile v2",
                fold(
                    postcard::to_stdvec(&BookFile {
                        version: 2,
                        signs: true,
                        entries: vec![BookEntry {
                            presence: presence.clone(),
                            worked: Some(1_700_000_002),
                            claim: Some(claim),
                        }],
                    })
                    .expect("encode"),
                ),
            ),
            (
                "BookFile v1",
                fold(
                    postcard::to_stdvec(&BookFileV1 {
                        version: 1,
                        signs: true,
                        entries: vec![BookEntryV1 {
                            presence,
                            worked: Some(1_700_000_002),
                        }],
                    })
                    .expect("encode"),
                ),
            ),
        ];
        itsanas_testkit::check_pinned(&found, PINNED);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use itsanas_coord::claim::Presence;
    use itsanas_crypto::{DeviceKeys, SecretBytes};

    use crate::contact::tests::{HOME, claimed_at, device, found, me, owner, signed_at};

    use super::*;

    /// What 2b.3 needs the book for: the pairs a peer may be handed, kept
    /// across a restart. An address that came without a claim, or unsigned,
    /// stays dialled and never leaves.
    #[test]
    fn claimed_addresses_are_relayable_across_a_restart_and_nothing_else_is() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let pi = claimed_at(&owner(1), 2, "192.168.1.20:9797");
        let unclaimed = signed_at(3, "192.168.1.30:9797");
        let unsigned = (device(9), "192.168.1.99:9797".to_owned());

        let mut before = Contact::new();
        let presences = [pi.presence.clone(), unclaimed.clone()];
        let mut listed = found(&presences);
        listed.push(unsigned);
        before.read(
            &listed,
            &presences,
            std::slice::from_ref(&pi),
            Instant::now(),
        );
        assert_eq!(
            before.relayable(),
            vec![pi.clone()],
            "the book would hand a peer an address with no claim of this account on it"
        );
        before.save(&path).expect("save");

        let (after, warning) = Contact::load(&path, me());
        assert_eq!(warning, None, "a book this node wrote was not read cleanly");
        assert_eq!(
            after.relayable(),
            vec![pi],
            "the claim was lost across a restart, so a rebooted machine has nothing to relay"
        );
    }

    /// A machine re-publishes the same address every hour with a new date.
    /// The book keeps the latest signature, not the first: the one it holds
    /// is the one a peer is handed, and a receiver has only the date to tell
    /// a fresh presence from a replayed one.
    #[test]
    fn the_same_address_signed_again_later_keeps_the_later_signature() {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([2; 32]));
        let at = |at_unix| {
            Presence {
                device: keys.device_id(),
                address: "192.168.1.20:9797".to_owned(),
                at_unix,
                head: 0,
            }
            .sign(&keys)
        };
        let mut contact = Contact::new();
        for signed in [at(100), at(200), at(150)] {
            contact.read(
                &found(std::slice::from_ref(&signed)),
                std::slice::from_ref(&signed),
                &[],
                Instant::now(),
            );
        }
        let held = contact.book[&keys.device_id()][0]
            .presence
            .as_ref()
            .map(|signed| signed.presence.at_unix);
        assert_eq!(
            held,
            Some(200),
            "the book kept an older signature, which a peer would be handed as if current"
        );
    }

    /// The file is editable by anything running as this user. A claim in it
    /// is checked as the wire's is: another account's genuine claim, put on
    /// one of the listed machines, would otherwise be handed to every peer of
    /// this account as one of its own.
    #[test]
    fn red_team_an_address_book_edited_to_hold_another_accounts_claim_does_not_relay_it() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let theirs = claimed_at(&owner(2), 2, "203.0.113.67:9797");
        let file = BookFile {
            version: BOOK_VERSION,
            signs: true,
            entries: vec![BookEntry {
                presence: theirs.presence.clone(),
                worked: None,
                claim: Some(theirs.claim),
            }],
        };
        std::fs::write(&path, postcard::to_stdvec(&file).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path, me());
        assert!(
            contact.relayable().is_empty(),
            "another account's machine would be relayed as one of this account's"
        );
        assert!(
            warning.is_some_and(|why| why.contains("1 claims")),
            "a tampered claim was dropped without a word"
        );
    }

    /// The book's format changed under running machines. Read as empty, the
    /// old file would forget that the coordinator signs, and the first round
    /// after the upgrade would accept an unsigned list again.
    #[test]
    fn red_team_the_upgrade_does_not_reopen_the_downgrade() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let pi = signed_at(2, "192.168.1.20:9797");
        let old = BookFileV1 {
            version: 1,
            signs: true,
            entries: vec![BookEntryV1 {
                presence: pi.clone(),
                worked: Some(5),
            }],
        };
        std::fs::write(&path, postcard::to_stdvec(&old).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path, me());
        assert_eq!(warning, None, "a version-1 book was refused");
        assert!(
            !contact.due(Some(HOME), Instant::now()).accept_unsigned,
            "the upgrade forgot that the coordinator signs: an unsigned list is \
             accepted again on the first round"
        );
        assert_eq!(
            contact.candidates(),
            found(&[pi]),
            "the old addresses were lost"
        );
    }

    /// 2b.1 closed the fallback to an unsigned list once the coordinator had
    /// signed in front of the process, and left it open at every start: a
    /// hostile coordinator only had to wait for a reboot and hang up. The
    /// memory now lives in the address book.
    #[test]
    fn red_team_a_restart_does_not_reopen_the_downgrade() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let pi = signed_at(2, "192.168.1.20:9797");

        let mut before = Contact::new();
        before.read(
            &found(std::slice::from_ref(&pi)),
            std::slice::from_ref(&pi),
            &[],
            Instant::now(),
        );
        before.signed();
        assert!(
            before.save(&path).expect("save"),
            "a changed book was not written"
        );

        let (after, warning) = Contact::load(&path, me());
        assert_eq!(warning, None, "a book this node wrote was not read cleanly");
        assert!(
            !after.due(Some(HOME), Instant::now()).accept_unsigned,
            "after a restart the first round would read an unsigned list from a \
             coordinator that has signed: the downgrade is back at every reboot"
        );
    }

    /// The file sits on disk, where anything running as this user can edit
    /// it, and its addresses are dialled before the coordinator is asked
    /// anything. Loading it goes through the same check as the wire.
    #[test]
    fn red_team_an_address_book_edited_to_hold_a_forged_presence_loses_it_on_load() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let genuine = signed_at(2, "192.168.1.20:9797");
        let mut moved = signed_at(3, "192.168.1.30:9797");
        moved.presence.address = "203.0.113.66:9797".to_owned();

        let file = BookFile {
            version: BOOK_VERSION,
            signs: true,
            entries: vec![
                BookEntry {
                    presence: genuine.clone(),
                    worked: None,
                    claim: None,
                },
                BookEntry {
                    presence: moved,
                    worked: Some(u64::MAX),
                    claim: None,
                },
            ],
        };
        std::fs::write(&path, postcard::to_stdvec(&file).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path, me());
        assert_eq!(
            contact.candidates(),
            found(&[genuine]),
            "an address its device never signed was loaded and would be dialled first"
        );
        assert!(
            warning.is_some_and(|why| why.contains("1 of 2")),
            "a tampered address book was loaded without a word"
        );
    }

    /// What the book is for: the address that answered is still dialled first
    /// after a restart, and an address an old coordinator handed out unsigned
    /// is dialled but never written, where the next load would have had to
    /// take it on trust.
    #[test]
    fn the_address_that_worked_is_still_first_after_a_restart() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let public = signed_at(2, "203.0.113.20:9797");
        let home_lan = signed_at(2, "192.168.1.20:9797");
        let pi = public.presence.device;
        let unsigned = (device(9), "192.168.1.99:9797".to_owned());

        let mut before = Contact::new();
        let mut listed = found(&[public.clone(), home_lan.clone()]);
        listed.push(unsigned.clone());
        before.read(&listed, &[public, home_lan], &[], Instant::now());
        before.worked(pi, "192.168.1.20:9797", 1_000);
        assert!(
            before.candidates().contains(&unsigned),
            "an unsigned address was not dialled"
        );
        before.save(&path).expect("save");
        assert!(
            !before.save(&path).expect("save"),
            "an unchanged book was written again"
        );

        let (after, _) = Contact::load(&path, me());
        let order = after.candidates();
        assert_eq!(
            order.first().map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "the address that worked lost its rank across a restart"
        );
        assert!(
            !order.contains(&unsigned),
            "an unsigned address was laundered through the file"
        );
    }

    /// Found by Rodin on 2026-09-29: a Pi with no real-time clock reads 1970
    /// until NTP answers. The address that just worked must still outrank one
    /// that worked yesterday by a clock that was right then.
    #[test]
    fn red_team_a_clock_back_in_1970_does_not_rank_a_stale_address_first() {
        let pi = device(2);
        let mut contact = Contact::new();
        contact.read(
            &[
                (pi, "203.0.113.20:9797".to_owned()),
                (pi, "192.168.1.20:9797".to_owned()),
            ],
            &[],
            &[],
            Instant::now(),
        );
        contact.worked(pi, "203.0.113.20:9797", 1_700_000_000);
        contact.worked(pi, "192.168.1.20:9797", 40);
        assert_eq!(
            contact
                .candidates()
                .first()
                .map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "a success recorded before NTP ranked below yesterday's address, which \
             now costs a connect timeout every round"
        );
    }

    #[test]
    fn a_damaged_address_book_is_an_empty_one_not_a_failed_start() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        std::fs::write(&path, [0xff; 3]).expect("write");
        let (contact, warning) = Contact::load(&path, me());
        assert_eq!(
            contact.candidates(),
            [] as [(itsanas_crypto::DeviceId, std::string::String); 0]
        );
        assert!(
            warning.is_some(),
            "a damaged book was dropped without a word"
        );
        assert_eq!(
            Contact::load(&home.path().join("absent"), me()).1,
            None,
            "a first start is not a warning"
        );
    }
}
