//! What the coordinator mails, and how much.
//!
//! Three things only: the code that verifies an address, the code a recovery
//! from the passphrase needs once an address is verified, and a reminder of
//! the account's name. Each message is queued ([`Outbox::send`]) and sent by
//! another thread (the coordinator binary's Mailjet sender), so a request is
//! answered before any mail leaves and its time says nothing of whether one
//! did.
//!
//! The provider's allowance is shared by every member, so a stranger asking
//! for recovery codes in a loop must not use up the day for everybody else:
//! each purpose has its own share of the day ([`DAILY`]), an account is mailed
//! at most [`PER_ACCOUNT_DAILY`] times a day and never twice within
//! [`ACCOUNT_SPACING_SECS`], and an address at most [`PER_ADDRESS_DAILY`]
//! times a day, so nobody can bury a third party's inbox under codes. The
//! server adds a budget per caller address on the messages that mail
//! (`server::names`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use itsanas_crypto::UserId;

use crate::protocol::MAX_WIRE_EMAIL;

/// One message for the sender thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    /// The address it goes to.
    pub to: String,
    /// Its subject line.
    pub subject: String,
    /// Its body, plain text.
    pub text: String,
}

/// Why a message is sent: each has its own share of the day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// The code that verifies an address.
    Verify = 0,
    /// The code a recovery needs.
    Recover = 1,
    /// A reminder of the account's name.
    Remind = 2,
}

/// Messages a day, by [`Purpose`]: 200 in all, the provider's free allowance.
pub const DAILY: [u32; 3] = [60, 110, 30];

/// Messages one account is sent a day, every purpose together.
pub const PER_ACCOUNT_DAILY: u32 = 5;

/// The least time between two messages to one account.
pub const ACCOUNT_SPACING_SECS: u64 = 60;

/// Messages one address is sent a day.
pub const PER_ADDRESS_DAILY: u32 = 3;

/// Messages waiting for the sender at most; past it a request is refused.
pub const QUEUE: usize = 64;

const DAY: u64 = 24 * 60 * 60;

/// The day's counts. Kept in memory: a restart gives the day back, which costs
/// at most one more day's allowance and never a member's recovery.
#[derive(Debug, Default)]
struct Counts {
    day: u64,
    sent: [u32; 3],
    accounts: HashMap<UserId, (u64, u32)>,
    addresses: HashMap<String, u32>,
}

/// The queue the sender thread reads, and the day's budget.
#[derive(Debug)]
pub struct Outbox {
    queue: SyncSender<Mail>,
    counts: Mutex<Counts>,
    spacing: u64,
}

/// Why a message was not queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// The day's share of this purpose, or the account's or the address's, is
    /// spent, or the account was mailed a moment ago.
    Budget,
    /// The sender is behind.
    Queue,
}

impl Outbox {
    /// An outbox and the end its sender reads.
    #[must_use]
    pub fn new() -> (Self, Receiver<Mail>) {
        let (queue, receiver) = sync_channel(QUEUE);
        (
            Self {
                queue,
                counts: Mutex::new(Counts::default()),
                spacing: ACCOUNT_SPACING_SECS,
            },
            receiver,
        )
    }

    /// The same outbox, with `seconds` between two mails to one account in
    /// place of [`ACCOUNT_SPACING_SECS`]; for tests that cannot wait a minute.
    #[must_use]
    pub const fn with_spacing(mut self, seconds: u64) -> Self {
        self.spacing = seconds;
        self
    }

    /// Queue `mail` to `account`, if the budgets allow it at `now`.
    ///
    /// # Errors
    ///
    /// [`Held`], and then nothing was counted.
    pub fn send(
        &self,
        purpose: Purpose,
        account: UserId,
        mail: Mail,
        now: u64,
    ) -> Result<(), Held> {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if counts.day != now / DAY {
            *counts = Counts {
                day: now / DAY,
                ..Counts::default()
            };
        }
        let folded = mail.to.to_lowercase();
        let (last, today) = counts.accounts.get(&account).copied().unwrap_or((0, 0));
        let spent = counts.sent[purpose as usize] >= DAILY[purpose as usize]
            || today >= PER_ACCOUNT_DAILY
            || (today > 0 && now < last.saturating_add(self.spacing))
            || counts.addresses.get(&folded).copied().unwrap_or(0) >= PER_ADDRESS_DAILY;
        if spent {
            return Err(Held::Budget);
        }
        match self.queue.try_send(mail) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => return Err(Held::Queue),
        }
        counts.sent[purpose as usize] += 1;
        counts.accounts.insert(account, (now, today + 1));
        *counts.addresses.entry(folded).or_insert(0) += 1;
        Ok(())
    }
}

/// Whether `address` can be mailed at all: one `@`, something before it, a
/// dotted name after it, no space or control character, and not too long.
/// Deliverability is the code's job: an address that takes no mail never
/// gets verified.
#[must_use]
pub fn plausible_address(address: &str) -> bool {
    if address.len() > MAX_WIRE_EMAIL
        || address.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let mut parts = address.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
}

/// A fresh code of eight digits.
///
/// # Errors
///
/// If the system gives no randomness.
pub fn fresh_code() -> itsanas_crypto::Result<String> {
    let random = itsanas_crypto::SymmetricKey::random()?;
    let mut eight = [0u8; 8];
    eight.copy_from_slice(&random.expose()[..8]);
    Ok(format!("{:08}", u64::from_le_bytes(eight) % 100_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(to: &str) -> Mail {
        Mail {
            to: to.to_owned(),
            subject: String::new(),
            text: String::new(),
        }
    }

    fn account(byte: u8) -> UserId {
        UserId::from_bytes([byte; 32])
    }

    #[test]
    fn red_team_recovery_codes_asked_in_a_loop_leave_the_verifications_their_share() {
        // One stranger asking recovery codes for every name on a list must
        // not stop a new member from verifying their address the same day.
        let (outbox, receiver) = Outbox::new();
        let share = DAILY[Purpose::Recover as usize];
        for n in 0..share {
            let byte = u8::try_from(n).unwrap();
            let to = format!("x{n}@a.example");
            assert_eq!(
                outbox.send(Purpose::Recover, account(byte), mail(&to), 1000),
                Ok(())
            );
            let _ = receiver.try_recv();
        }
        assert_eq!(
            outbox.send(Purpose::Recover, account(250), mail("late@a.example"), 1000),
            Err(Held::Budget),
            "the recovery share is spent"
        );
        assert_eq!(
            outbox.send(Purpose::Verify, account(251), mail("new@a.example"), 1000),
            Ok(()),
            "a verification still goes out: the recovery share is not the day's"
        );
    }

    #[test]
    fn red_team_one_account_is_not_mailed_twice_in_a_minute_nor_past_its_day() {
        let (outbox, receiver) = Outbox::new();
        let one = account(1);
        assert_eq!(
            outbox.send(Purpose::Recover, one, mail("a@b.example"), 1000),
            Ok(())
        );
        assert_eq!(
            outbox.send(Purpose::Recover, one, mail("a@b.example"), 1030),
            Err(Held::Budget),
            "a second code within the minute"
        );
        let mut at = 1000;
        for _ in 1..PER_ACCOUNT_DAILY {
            at += ACCOUNT_SPACING_SECS;
            let _ = receiver.try_recv();
            assert!(
                outbox
                    .send(Purpose::Verify, one, mail(&format!("{at}@b.example")), at)
                    .is_ok()
            );
        }
        at += ACCOUNT_SPACING_SECS;
        assert_eq!(
            outbox.send(Purpose::Remind, one, mail("z@b.example"), at),
            Err(Held::Budget),
            "the account's day is spent, whatever the purpose"
        );
        assert_eq!(
            outbox.send(Purpose::Remind, one, mail("z@b.example"), at + DAY),
            Ok(()),
            "the next day gives it back"
        );
    }

    #[test]
    fn red_team_a_third_partys_address_takes_three_codes_a_day_at_most() {
        // Binding a stranger's address over and over, from many accounts,
        // would otherwise bury that inbox.
        let (outbox, receiver) = Outbox::new();
        for n in 0..3 {
            assert!(
                outbox
                    .send(
                        Purpose::Verify,
                        account(n),
                        mail("Victim@Mail.example"),
                        1000
                    )
                    .is_ok()
            );
            let _ = receiver.try_recv();
        }
        assert_eq!(
            outbox.send(
                Purpose::Verify,
                account(9),
                mail("victim@mail.example"),
                1000
            ),
            Err(Held::Budget),
            "the fourth, under another spelling of the same address"
        );
    }

    #[test]
    fn a_full_queue_is_said_and_counts_nothing() {
        let (outbox, _receiver) = Outbox::new();
        for n in 0..QUEUE {
            let byte = u8::try_from(n).unwrap();
            assert!(
                outbox
                    .send(
                        Purpose::Recover,
                        account(byte),
                        mail(&format!("{n}@q.example")),
                        5
                    )
                    .is_ok()
            );
        }
        assert_eq!(
            outbox.send(Purpose::Recover, account(200), mail("late@q.example"), 5),
            Err(Held::Queue)
        );
    }

    #[test]
    fn only_a_mailable_shape_is_taken() {
        for good in ["a@b.example", "first.last+tag@mail.example.org"] {
            assert!(plausible_address(good), "{good}");
        }
        for bad in [
            "",
            "a",
            "a@b",
            "@b.example",
            "a@@b.example",
            "a b@c.example",
            "a@.example",
            "a@b..example",
            "a@b.example.",
            "a\n@b.example",
        ] {
            assert!(!plausible_address(bad), "{bad:?}");
        }
        assert!(!plausible_address(&format!(
            "{}@b.example",
            "a".repeat(MAX_WIRE_EMAIL)
        )));
    }

    #[test]
    fn a_code_is_eight_digits() {
        for _ in 0..50 {
            let code = fresh_code().unwrap();
            assert_eq!(code.len(), 8);
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
    }
}
