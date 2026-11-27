//! The claims a host has taken, by device: what it asks the coordinator
//! about, what the answers did, and which account each device is.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use itsanas_coord::claim::{MAX_DEVICES_PER_ACCOUNT, SignedClaim};
use itsanas_crypto::{DeviceId, UserId};
use itsanas_store::Vault;

use super::{
    ASK_AGAIN_AFTER, ASKING_AT_ONCE, ASKS_PER_MINUTE, Asker, ClaimBook, Held, Lent, STANDING_FOR,
    Standing, Verdict, WITHDRAWN,
};

impl ClaimBook {
    /// An empty book of [`MAX_CLAIMS`] places.
    ///
    /// [`MAX_CLAIMS`]: super::MAX_CLAIMS
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty book of `capacity` places, for a test that has to fill one.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            claims: Mutex::new(BTreeMap::new()),
            lent: Mutex::new(Lent::default()),
            capacity,
            asking: None,
            asked: Mutex::new(std::collections::VecDeque::new()),
            in_flight: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Ask `asker` about a device of another account than `own` when it
    /// presents a claim this book has not confirmed, up to
    /// [`ASKS_PER_MINUTE`]. Without it a device waits for [`ClaimBook::due`].
    #[must_use]
    pub fn asking(mut self, own: UserId, asker: Asker) -> Self {
        self.asking = Some((own, asker));
        self
    }

    /// Whether one more question fits in the last minute's budget, counted
    /// if it does.
    fn may_ask(&self) -> bool {
        let mut asked = self
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while asked
            .front()
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
        {
            asked.pop_front();
        }
        if asked.len() >= ASKS_PER_MINUTE {
            return false;
        }
        asked.push_back(Instant::now());
        true
    }

    /// Whose each device that presented a claim here is, as its signed claim
    /// says: what placement reads to tell two hosts of one account from two
    /// of different accounts.
    ///
    /// Only claims the coordinator confirmed within [`STANDING_FOR`]. A
    /// signature proves the account issued a claim once, not that it still
    /// stands: a withdrawn device, or a stolen key's fresh claims nobody has
    /// confirmed, would otherwise count as a distinct account for
    /// durability, which is the overstatement placement must never make
    /// (red team, MEDIUM 8). Left out, such a device is `Unknown`, and every
    /// unknown device is one account. Bounded by the book's capacity, so a
    /// round never copies more than [`MAX_CLAIMS`] rows.
    ///
    /// [`MAX_CLAIMS`]: super::MAX_CLAIMS
    #[must_use]
    pub fn accounts(&self) -> BTreeMap<DeviceId, UserId> {
        self.lock()
            .iter()
            .filter(|(_, held)| {
                matches!(held.standing, Standing::Live(at) if at.elapsed() < STANDING_FOR)
            })
            .map(|(device, held)| (*device, held.owner))
            .collect()
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<DeviceId, Held>> {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take a claim `caller` presented.
    ///
    /// Checked for its signer, never its date: a Raspberry Pi with no
    /// real-time clock reads 1970 and would refuse every claim as from the
    /// future, which would make every peer "update required" (redteam). The
    /// date only orders a device's claims against each other.
    ///
    /// # Errors
    ///
    /// When it does not decode, is not signed by the account it names, names
    /// another device than the one TLS proved, is a withdrawal, would be one
    /// device more than [`MAX_DEVICES_PER_ACCOUNT`] for its account, or the
    /// book is full of accounts that hold something here.
    pub fn take(&self, caller: DeviceId, bytes: &[u8], vault: &Vault) -> Result<(), String> {
        let signed: SignedClaim =
            postcard::from_bytes(bytes).map_err(|_| "not a claim".to_owned())?;
        signed.verify_origin().map_err(|error| error.to_string())?;
        let claim = &signed.claim;
        let mut book = self.lock();
        if claim.revoked {
            // Signed with the account's key, a withdrawal is the account's word
            // whoever hands it over: a sibling passes it on to the hosts it
            // meets (`Node::introduce`), so a host learns it without a
            // coordinator, and the withdrawn device's old claim stays refused.
            return Err(Self::withdrawn(&mut book, claim, bytes));
        }
        if claim.device != caller {
            return Err("the claim is for another device than the one connected".to_owned());
        }
        let mut standing = Standing::Unconfirmed;
        let mut presented = Instant::now();
        let mut asked = None;
        match book.get(&caller) {
            Some(held) if held.owner != claim.owner => {
                return Err("this device is already claimed by another account".to_owned());
            }
            Some(held) if held.standing == Standing::Withdrawn => {
                return Err(WITHDRAWN.to_owned());
            }
            Some(held) if held.issued > claim.issued_unix => return Ok(()),
            Some(held) => {
                standing = held.standing;
                presented = held.presented;
                asked = held.asked;
            }
            None => {
                // Live ones: a withdrawn device keeps its row, refused, and
                // takes no slot from the account's machines.
                let siblings = book
                    .values()
                    .filter(|held| {
                        held.owner == claim.owner && held.standing != Standing::Withdrawn
                    })
                    .count();
                if siblings >= MAX_DEVICES_PER_ACCOUNT {
                    return Err(format!(
                        "this account already has its {MAX_DEVICES_PER_ACCOUNT} devices here"
                    ));
                }
                if book.len() >= self.capacity {
                    let empty = book.iter().find_map(|(device, held)| {
                        vault
                            .held_bytes_for(held.owner)
                            .is_ok_and(|bytes| bytes == 0)
                            .then_some(*device)
                    });
                    match empty {
                        Some(device) => {
                            book.remove(&device);
                        }
                        None => return Err("this host remembers no more devices".to_owned()),
                    }
                }
            }
        }
        book.insert(
            caller,
            Held {
                owner: claim.owner,
                pledged: claim.pledged_bytes,
                issued: claim.issued_unix,
                claim: bytes.to_vec(),
                standing,
                presented,
                asked,
            },
        );
        drop(book);
        // Asked outside the lock: it is a round trip to the coordinator, and
        // every other connection's `admits` waits on this lock.
        if let Some((own, asker)) = &self.asking {
            // Asked again at half of `STANDING_FOR`, as `due` does, so a withdrawal
            // reaches this host within the hour whichever comes first.
            let confirmed =
                matches!(standing, Standing::Live(at) if at.elapsed() < STANDING_FOR / 2);
            // Only a device never asked about: the rest wait for the round,
            // which asks oldest first, so presenting again buys nothing.
            if claim.owner != *own && !confirmed && asked.is_none() && self.may_ask() {
                use std::sync::atomic::Ordering;
                if self.in_flight.fetch_add(1, Ordering::AcqRel) < ASKING_AT_ONCE {
                    let verdict = asker(&signed);
                    self.in_flight.fetch_sub(1, Ordering::AcqRel);
                    self.note(caller, verdict);
                } else {
                    self.in_flight.fetch_sub(1, Ordering::AcqRel);
                }
            }
        }
        Ok(())
    }

    /// Up to `at_most` devices of other accounts than `own` to ask the
    /// coordinator about, with the claim each presented, most pressing first.
    ///
    /// First, confirmations past half of [`STANDING_FOR`], oldest first:
    /// those are real devices about to be refused. Then devices never
    /// answered, by when they presented. Last, devices the coordinator said
    /// it holds nothing for, once [`ASK_AGAIN_AFTER`] has passed. A withdrawn
    /// device is never asked again. Never in id order: ids are free to grind,
    /// and a book read in id order let eight junk devices take every round's
    /// questions for ever (redteam).
    #[must_use]
    pub fn due(&self, own: UserId, at_most: usize) -> Vec<(DeviceId, Vec<u8>)> {
        let book = self.lock();
        let mut due: Vec<((u8, Instant), DeviceId)> = book
            .iter()
            .filter(|(_, held)| held.owner != own)
            .filter_map(|(device, held)| {
                let key = match (held.standing, held.asked) {
                    (Standing::Live(at), _) if at.elapsed() >= STANDING_FOR / 2 => (0, at),
                    (Standing::Unconfirmed, None | Some((_, false))) => (1, held.presented),
                    (Standing::Unconfirmed, Some((at, true)))
                        if at.elapsed() >= ASK_AGAIN_AFTER =>
                    {
                        (2, at)
                    }
                    // Withdrawn for good, confirmed lately, or disowned lately.
                    _ => return None,
                };
                Some((key, *device))
            })
            .collect();
        due.sort_unstable();
        due.into_iter()
            .take(at_most)
            .filter_map(|(_, device)| book.get(&device).map(|held| (device, held.claim.clone())))
            .collect()
    }

    /// What the coordinator answered about `device`, heard now.
    pub fn note(&self, device: DeviceId, verdict: Verdict) {
        self.note_at(device, verdict, Instant::now());
    }

    fn note_at(&self, device: DeviceId, verdict: Verdict, at: Instant) {
        let mut book = self.lock();
        let Some(held) = book.get_mut(&device) else {
            return;
        };
        held.asked = Some((at, verdict == Verdict::Unenrolled));
        held.standing = match (held.standing, verdict) {
            (Standing::Withdrawn, _) | (_, Verdict::Withdrawn) => Standing::Withdrawn,
            (_, Verdict::Live) => Standing::Live(at),
            (_, Verdict::Unenrolled) => Standing::Unconfirmed,
            (kept, Verdict::NoAnswer) => kept,
        };
    }

    /// Read the coordinator's answer to [`Request::Standing`] about the
    /// claim `presented`, checked: a claim it hands back must be signed by
    /// the account, for the device and the account asked about. Anything
    /// else is [`Verdict::Unenrolled`] -- a coordinator can refuse a member,
    /// never vouch for one.
    ///
    /// [`Request::Standing`]: itsanas_coord::protocol::Request::Standing
    #[must_use]
    pub fn verdict(presented: &SignedClaim, answer: Option<&SignedClaim>) -> Verdict {
        match answer {
            Some(held)
                if held.verify_origin().is_ok()
                    && held.claim.owner == presented.claim.owner
                    && held.claim.device == presented.claim.device =>
            {
                if held.claim.revoked {
                    Verdict::Withdrawn
                } else {
                    Verdict::Live
                }
            }
            _ => Verdict::Unenrolled,
        }
    }
}

impl ClaimBook {
    /// Record `claim`, a withdrawal, in `book`: its device is refused here for
    /// good. At most [`MAX_DEVICES_PER_ACCOUNT`] withdrawn devices an account,
    /// besides its live ones, so an account cannot fill the book with
    /// withdrawals of ids it made up. What `take` answers.
    fn withdrawn(
        book: &mut std::collections::BTreeMap<DeviceId, Held>,
        claim: &itsanas_coord::claim::NodeClaim,
        bytes: &[u8],
    ) -> String {
        let known = book
            .get(&claim.device)
            .is_some_and(|held| held.owner == claim.owner);
        let withdrawn = book
            .values()
            .filter(|held| held.owner == claim.owner && held.standing == Standing::Withdrawn)
            .count();
        if known || withdrawn < MAX_DEVICES_PER_ACCOUNT {
            book.insert(
                claim.device,
                Held {
                    owner: claim.owner,
                    pledged: 0,
                    issued: claim.issued_unix,
                    claim: bytes.to_vec(),
                    standing: Standing::Withdrawn,
                    presented: Instant::now(),
                    asked: None,
                },
            );
        }
        WITHDRAWN.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::NodeClaim;

    use super::*;
    use crate::owners::UNCONFIRMED;
    use crate::owners::tests::{
        NOW, account, admits, claim, device, host, host_of, present, presents, signed,
    };

    #[test]
    fn red_team_placement_reads_only_confirmed_claims_as_accounts() {
        // Two hosts of one household must read as one account to placement,
        // or a chunk counts as spread over two accounts while one power cut
        // takes both. And a claim the coordinator has not confirmed, or has
        // withdrawn, names no account at all: counted as a distinct account
        // it would let a stolen key's fresh claims, or a withdrawn device,
        // stand in for a second household. Sabotages: key the map by owner;
        // keep unconfirmed or withdrawn claims.
        let host = host();
        let (alice, bob) = (account(0xA1), account(0xB0));
        present(&host, &alice, 0x21, 700);
        present(&host, &alice, 0x24, 700);
        presents(&host, &alice, 0x22, 700);
        present(&host, &bob, 0x23, 700);
        present(&host, &bob, 0x25, 700);
        host.book.note(device(0x25), Verdict::Withdrawn);

        let accounts = host.book.accounts();
        assert_eq!(accounts.len(), 3, "{accounts:?}");
        assert_eq!(accounts[&device(0x21)], alice.user_id());
        assert_eq!(accounts[&device(0x24)], alice.user_id());
        assert_eq!(accounts[&device(0x23)], bob.user_id());
        assert!(!accounts.contains_key(&device(0x22)), "unconfirmed");
        assert!(!accounts.contains_key(&device(0x25)), "withdrawn");
        assert!(!accounts.contains_key(&device(0x99)));
    }

    #[test]
    fn red_team_a_claim_for_another_device_is_refused() {
        // A claim is public once presented; replaying a friend's claim from
        // one's own key must not borrow the friend's pledge. Sabotage: drop
        // the device check.
        let host = host();
        let alice = account(0xA1);
        let theirs = claim(&alice, 0x11, 7_000, false);
        assert!(host.book.take(device(0x22), &theirs, &host.vault).is_err());
        assert!(admits(&host, device(0x22), alice.user_id(), 1).is_err());
    }

    #[test]
    fn red_team_a_device_the_coordinator_says_was_withdrawn_stores_nothing() {
        // §8 1c (ii). Every node holds the account key, so a withdrawn device
        // signs itself a fresh live claim; the coordinator's withdrawal is the
        // only word against it, and it is final. Sabotage: skip the verdict.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 700);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        host.book.note(device(0x11), Verdict::Withdrawn);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(WITHDRAWN.to_owned()),
            "a device the coordinator says was withdrawn still stored here"
        );
        let resigned = postcard::to_stdvec(
            &NodeClaim {
                owner: alice.user_id(),
                device: device(0x11),
                pledged_bytes: 700,
                issued_unix: NOW + 1_000,
                revoked: false,
            }
            .sign(&alice),
        )
        .unwrap();
        assert_eq!(
            host.book.take(device(0x11), &resigned, &host.vault),
            Err(WITHDRAWN.to_owned()),
            "a later live claim brought a withdrawn device back"
        );
        host.book.note(device(0x11), Verdict::Live);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(WITHDRAWN.to_owned()),
            "a later answer undid a withdrawal, which is final"
        );
    }

    #[test]
    fn red_team_no_word_from_the_coordinator_means_no_storing() {
        // Nicolas, 2026-10-05: a device this host has not confirmed with the
        // coordinator stores nothing here, and neither does one whose
        // confirmation has lapsed while the coordinator does not answer.
        // Otherwise a withdrawn device stores wherever the coordinator is out
        // of reach. Sabotage: admit an unconfirmed device; never let a
        // confirmation lapse.
        let host = host();
        let alice = account(0xA1);
        presents(&host, &alice, 0x11, 700);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(UNCONFIRMED.to_owned()),
            "a device nobody asked the coordinator about stored here"
        );
        let long_ago = Instant::now()
            .checked_sub(STANDING_FOR + Duration::from_secs(1))
            .expect("a monotonic clock older than two hours");
        host.book.note_at(device(0x11), Verdict::Live, long_ago);
        host.book.note(device(0x11), Verdict::NoAnswer);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(UNCONFIRMED.to_owned()),
            "a confirmation outlived STANDING_FOR while the coordinator was silent"
        );
    }

    #[test]
    fn a_silent_coordinator_keeps_a_fresh_confirmation_and_an_empty_answer_ends_it() {
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 700);
        host.book.note(device(0x11), Verdict::NoAnswer);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        host.book.note(device(0x11), Verdict::Unenrolled);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(UNCONFIRMED.to_owned())
        );
    }

    #[test]
    fn this_hosts_own_devices_need_no_word_from_the_coordinator() {
        // A household keeps replicating to itself with the coordinator down,
        // or with none configured: the bargain is between accounts.
        let host = host();
        let own = account(0x01);
        presents(&host, &own, 0x31, 0);
        assert_eq!(admits(&host, device(0x31), own.user_id(), 1 << 30), Ok(()));
        assert_eq!(host.book.due(own.user_id(), 8), Vec::new());
    }

    #[test]
    fn due_names_the_unconfirmed_and_the_ageing_and_never_the_withdrawn() {
        let host = host();
        let alice = account(0xA1);
        presents(&host, &alice, 0x11, 700);
        present(&host, &alice, 0x12, 700);
        presents(&host, &alice, 0x13, 700);
        host.book.note(device(0x13), Verdict::Withdrawn);
        let due: Vec<DeviceId> = host
            .book
            .due(host.store.owner(), 8)
            .into_iter()
            .map(|(device, _)| device)
            .collect();
        assert_eq!(due, vec![device(0x11)]);
        let ageing = Instant::now()
            .checked_sub(STANDING_FOR / 2)
            .expect("a monotonic clock older than an hour");
        host.book.note_at(device(0x12), Verdict::Live, ageing);
        assert_eq!(host.book.due(host.store.owner(), 8).len(), 2);
        assert_eq!(host.book.due(host.store.owner(), 1).len(), 1, "unbounded");
    }

    #[test]
    fn red_team_junk_devices_do_not_take_every_question_from_a_real_one() {
        // Found by the redteam agent: device keys are free, and the round
        // asked about the book in id order, eight a round. Twenty junk
        // devices the coordinator does not know, presented first, then one
        // real device: the round must reach the real one. Sabotage: no
        // back-off for a device answered `Unenrolled`.
        let host = host();
        for seed in 0x40..0x54u8 {
            presents(&host, &account(seed), seed, 1);
            host.book.note(device(seed), Verdict::Unenrolled);
        }
        let bob = account(0xB2);
        presents(&host, &bob, 0x22, 700);
        let due: Vec<DeviceId> = host
            .book
            .due(host.store.owner(), 8)
            .into_iter()
            .map(|(device, _)| device)
            .collect();
        assert_eq!(
            due,
            vec![device(0x22)],
            "junk the coordinator already disowned was asked about again ahead of a real device"
        );
    }

    #[test]
    fn red_team_a_flood_of_newcomers_does_not_let_a_real_confirmation_lapse() {
        // A confirmation past half its life is a real device about to be
        // refused; it goes before every never-asked newcomer, however many.
        // Sabotage: order `due` by id, or by presentation alone.
        let host = host();
        // The newcomers present first, so ordering by presentation alone
        // would put the real device last.
        for seed in 0x40..0x54u8 {
            presents(&host, &account(seed), seed, 1);
        }
        let bob = account(0xB2);
        present(&host, &bob, 0xF0, 700);
        let ageing = Instant::now()
            .checked_sub(STANDING_FOR / 2)
            .expect("a monotonic clock older than an hour");
        host.book.note_at(device(0xF0), Verdict::Live, ageing);
        let due = host.book.due(host.store.owner(), 1);
        assert_eq!(
            due.first().map(|(device, _)| *device),
            Some(device(0xF0)),
            "newcomers were asked about before a live device whose confirmation was ageing"
        );
    }

    #[test]
    fn red_team_a_coordinator_cannot_vouch_with_a_claim_for_another_device_or_account() {
        // The answer is checked like any claim: a coordinator that answers
        // with a live claim of another device, or a forged one, confirms
        // nothing. Sabotage: drop either check in `verdict`.
        let alice = account(0xA1);
        let presented = signed(&claim(&alice, 0x11, 700, false));
        let other_device = signed(&claim(&alice, 0x12, 700, false));
        let other_account = signed(&claim(&account(0xB2), 0x11, 700, false));
        let mut forged = signed(&claim(&alice, 0x11, 700, true));
        forged.claim.revoked = false;
        for (answer, what) in [
            (&other_device, "another device"),
            (&other_account, "another account"),
            (&forged, "a forged claim"),
        ] {
            assert_eq!(
                ClaimBook::verdict(&presented, Some(answer)),
                Verdict::Unenrolled,
                "a coordinator vouched with {what}"
            );
        }
        assert_eq!(
            ClaimBook::verdict(&presented, Some(&signed(&claim(&alice, 0x11, 1, true)))),
            Verdict::Withdrawn
        );
        assert_eq!(
            ClaimBook::verdict(&presented, Some(&presented)),
            Verdict::Live
        );
        assert_eq!(ClaimBook::verdict(&presented, None), Verdict::Unenrolled);
    }

    #[test]
    fn a_device_is_asked_about_when_it_presents_so_it_stores_on_its_first_round() {
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let own = account(0x01).user_id();
        let book = ClaimBook::new().asking(
            own,
            Box::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Verdict::Live
            }),
        );
        let host = host_of(book);
        let alice = account(0xA1);
        presents(&host, &alice, 0x11, 700);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        presents(&host, &alice, 0x11, 700);
        presents(&host, &account(0x01), 0x31, 0);
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "asked again about a confirmed device, or about this host's own"
        );
    }

    #[test]
    fn red_team_presenting_again_does_not_buy_another_question() {
        // One junk device presenting on every connection would otherwise spend
        // the whole minute's budget on itself. Answered once, it waits for the
        // round's back-off. Sabotage: ask whenever it is not confirmed.
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let book = ClaimBook::new().asking(
            account(0x01).user_id(),
            Box::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Verdict::Unenrolled
            }),
        );
        let host = host_of(book);
        for _ in 0..10 {
            presents(&host, &account(0xEE), 0x66, 1);
        }
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a device the coordinator disowned bought a question with every presentation"
        );
    }

    #[test]
    fn red_team_a_flood_of_presentations_does_not_become_a_flood_of_questions() {
        // Device keys are free, so asking on every presentation would make
        // this host an amplifier against its coordinator. Sabotage: no budget.
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let book = ClaimBook::new().asking(
            account(0x01).user_id(),
            Box::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Verdict::Unenrolled
            }),
        );
        let host = host_of(book);
        let flood = u8::try_from(ASKS_PER_MINUTE).expect("a small budget") + 20;
        for seed in 0..flood {
            presents(
                &host,
                &account(0x80u8.wrapping_add(seed)),
                0x80u8.wrapping_add(seed),
                1,
            );
        }
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            ASKS_PER_MINUTE,
            "every presentation became a question to the coordinator"
        );
    }

    #[test]
    fn red_team_a_withdrawn_device_is_forgotten_and_one_too_many_is_refused() {
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 7_000);
        assert!(
            host.book
                .take(device(0x11), &claim(&alice, 0x11, 7_000, true), &host.vault)
                .is_err()
        );
        assert!(
            admits(&host, device(0x11), alice.user_id(), 1).is_err(),
            "a device its account withdrew still stores on the strength of its old claim"
        );
        let full = 0x20 + u8::try_from(MAX_DEVICES_PER_ACCOUNT).expect("small");
        for seed in 0x20..full {
            present(&host, &alice, seed, 1);
        }
        assert!(
            host.book
                .take(device(full), &claim(&alice, full, 1, false), &host.vault)
                .is_err(),
            "one device too many added its pledge to the account's"
        );
    }

    #[test]
    fn red_team_empty_claims_cannot_lock_newcomers_out() {
        // Found by the redteam agent: free keys fill the book with claims
        // that store nothing, and every honest newcomer is told the host
        // remembers no more devices. Sabotage: refuse when full.
        let host = host_of(ClaimBook::with_capacity(5));
        let squatter = account(0xEE);
        for seed in 0x60..0x65u8 {
            present(&host, &squatter, seed, 1);
        }
        let alice = account(0xA1);
        assert_eq!(
            host.book.take(
                device(0x11),
                &claim(&alice, 0x11, 7_000, false),
                &host.vault
            ),
            Ok(()),
            "a book full of claims holding nothing refused an honest newcomer"
        );
    }

    #[test]
    fn a_claim_is_checked_for_its_signer_not_its_date() {
        // Found by the redteam agent: a host on a Pi with no clock reads 1970
        // and would refuse every claim as from the future. A claim dated far
        // ahead of any clock is still taken.
        let host = host();
        let alice = account(0xA1);
        let ahead = NodeClaim {
            owner: alice.user_id(),
            device: device(0x11),
            pledged_bytes: 7_000,
            issued_unix: u64::MAX / 2,
            revoked: false,
        }
        .sign(&alice);
        assert_eq!(
            host.book.take(
                device(0x11),
                &postcard::to_stdvec(&ahead).unwrap(),
                &host.vault
            ),
            Ok(())
        );
    }
}
