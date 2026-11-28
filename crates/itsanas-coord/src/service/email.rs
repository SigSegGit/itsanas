//! The email messages: binding an address, its code, a recovery that needs a
//! mailed code once an address is verified, and a reminder of the name.
//!
//! What a stranger may ask ([`Request::AskRecovery`], [`Request::RemindUsername`])
//! is answered [`Response::Done`] whatever happens, and before any mail
//! leaves: the answer says nothing of the name or the address.

use std::sync::Mutex;
use std::time::Instant;

use itsanas_crypto::{DeviceId, UserId};

use super::CoordService;
use crate::directory::CodeCheck;
use crate::error::Result;
use crate::mail::{Mail, Purpose, fresh_code, plausible_address};
use crate::protocol::{EmailStatus, MAX_WIRE_EMAIL, MAX_WIRE_USERNAME, Request, Response};
use crate::service::limiter::EscrowLimiter;

/// What the member is told when this coordinator has no way to send mail.
pub const NO_MAIL: &str = "this coordinator sends no email; an address cannot be bound here";

const HELD: &str = "too many emails for now; try again in a few minutes, or tomorrow";

impl CoordService<'_> {
    /// Answer one of the email messages.
    pub(super) fn email(
        &self,
        request: &Request,
        caller: DeviceId,
        now_unix: u64,
        limiter: &Mutex<EscrowLimiter>,
        now: Instant,
    ) -> Result<Response> {
        match request {
            Request::BindEmail { address } => {
                let Some(owner) = self.member(caller, now_unix)? else {
                    return Ok(not_enrolled());
                };
                if let Some(address) = address {
                    self.bind(owner, address, now_unix)
                } else {
                    self.directory.unbind_email(owner)?;
                    Ok(Response::Done)
                }
            }
            Request::ConfirmEmail { code } => {
                let Some(owner) = self.member(caller, now_unix)? else {
                    return Ok(not_enrolled());
                };
                Ok(match self.directory.confirm_email(owner, code, now_unix)? {
                    CodeCheck::Accepted => Response::Done,
                    CodeCheck::Refused => Response::Refused(
                        "that is not the code, or it is over; `itsanas email set` sends another"
                            .to_owned(),
                    ),
                })
            }
            Request::Email => {
                let Some(owner) = self.member(caller, now_unix)? else {
                    return Ok(not_enrolled());
                };
                Ok(Response::Email(self.directory.email_of(owner)?.map(
                    |(address, verified)| EmailStatus { address, verified },
                )))
            }
            Request::AskRecovery { username } => {
                if username.len() <= MAX_WIRE_USERNAME {
                    self.mail_recovery_code(username, now_unix)?;
                }
                Ok(Response::Done)
            }
            Request::RecoverEscrow { username, code } => {
                self.recover(username, code, now_unix, limiter, now)
            }
            Request::RemindUsername { address } => {
                if address.len() <= MAX_WIRE_EMAIL {
                    self.remind(address, now_unix)?;
                }
                Ok(Response::Done)
            }
            _ => Ok(Response::Refused("not an email request".to_owned())),
        }
    }

    /// The account of a live, unrevoked device.
    fn member(&self, caller: DeviceId, now_unix: u64) -> Result<Option<UserId>> {
        Ok(self
            .directory
            .claim_for(caller)?
            .filter(|claim| !claim.claim.revoked && claim.verify(now_unix).is_ok())
            .map(|claim| claim.claim.owner))
    }

    fn bind(&self, owner: UserId, address: &str, now_unix: u64) -> Result<Response> {
        let Some(outbox) = self.outbox else {
            return Ok(Response::Refused(NO_MAIL.to_owned()));
        };
        if !plausible_address(address) {
            return Ok(Response::Refused(format!(
                "{address:?} is not an email address"
            )));
        }
        let code = fresh_code()?;
        let mail = Mail {
            to: address.to_owned(),
            subject: "ITSaNAS: your code".to_owned(),
            text: format!(
                "Your code is {code}. Type it where ITSaNAS asks for it; it works for 15 minutes.\n\nOnce this address is verified, recovering the account from its passphrase also needs a code sent here. The 24 words never need one.\n\nIf you did not ask for it, ignore this message: nothing is bound without the code."
            ),
        };
        if self
            .directory
            .owner_of_email(&address.to_lowercase())?
            .is_some_and(|other| other != owner)
        {
            return Ok(Response::Refused(
                "this address is another account's; each address names one account".to_owned(),
            ));
        }
        // Mailed first, bound after: a code held by the budget must not
        // replace the one already in the member's inbox.
        if outbox.send(Purpose::Verify, owner, mail, now_unix).is_err() {
            return Ok(Response::Refused(HELD.to_owned()));
        }
        match self.directory.bind_email(owner, address, &code, now_unix) {
            Ok(()) => Ok(Response::Done),
            Err(crate::error::CoordError::Rejected(why)) => Ok(Response::Refused(why.to_owned())),
            Err(error) => Err(error),
        }
    }

    fn mail_recovery_code(&self, username: &str, now_unix: u64) -> Result<()> {
        let Some(outbox) = self.outbox else {
            return Ok(());
        };
        let Some(account) = self.directory.account(username)? else {
            return Ok(());
        };
        let owner = account.user.id;
        let Some((address, true)) = self.directory.email_of(owner)? else {
            return Ok(());
        };
        let code = fresh_code()?;
        let mail = Mail {
            to: address,
            subject: "ITSaNAS: recovering your account".to_owned(),
            text: format!(
                "Somebody is recovering the account {username} from its passphrase. If it is you, the code is {code}; it works for 15 minutes.\n\nIf it is not you, ignore this message: without this code and the passphrase nothing opens."
            ),
        };
        // Lodged only when it was queued: a code that was never mailed must
        // not replace one that was.
        if outbox.send(Purpose::Recover, owner, mail, now_unix).is_ok() {
            self.directory.set_recovery_code(owner, &code, now_unix)?;
        }
        Ok(())
    }

    fn recover(
        &self,
        username: &str,
        code: &str,
        now_unix: u64,
        limiter: &Mutex<EscrowLimiter>,
        now: Instant,
    ) -> Result<Response> {
        if username.len() > MAX_WIRE_USERNAME {
            return Ok(Response::Refused("username too long".to_owned()));
        }
        if !self.escrow_allowed(limiter, username, now, now_unix)? {
            return Ok(Response::Refused(
                "too many recovery attempts for this account; try again later".to_owned(),
            ));
        }
        Ok(Response::Escrow(self.escrow_given(
            username,
            Some(code),
            now_unix,
        )?))
    }

    /// The container a recovery of `username` is handed: the real one, or the
    /// decoy when the account's verified address asks for a code and `code`
    /// is not the one waiting ([`Request::GetEscrow`] gives none).
    pub(super) fn escrow_given(
        &self,
        username: &str,
        code: Option<&str>,
        now_unix: u64,
    ) -> Result<Vec<u8>> {
        let gated = match self.directory.account(username)? {
            Some(account) => match self.directory.email_of(account.user.id)? {
                Some((_, true)) => Some(account.user.id),
                _ => None,
            },
            None => None,
        };
        if let Some(owner) = gated {
            let opened = match code {
                Some(code) if !code.trim().is_empty() => {
                    self.directory.take_recovery_code(owner, code, now_unix)? == CodeCheck::Accepted
                }
                _ => false,
            };
            if !opened {
                return self.directory.decoy_answer(username);
            }
        }
        self.directory.recovery_answer(username)
    }

    fn remind(&self, address: &str, now_unix: u64) -> Result<()> {
        let Some(outbox) = self.outbox else {
            return Ok(());
        };
        let Some(owner) = self.directory.owner_of_email(&address.to_lowercase())? else {
            return Ok(());
        };
        let Some(account) = self.directory.account_of(owner)? else {
            return Ok(());
        };
        let mail = Mail {
            to: address.to_owned(),
            subject: "ITSaNAS: your account name".to_owned(),
            text: format!(
                "The ITSaNAS account bound to this address is named {}.\n\nIf you did not ask, ignore this message.",
                account.username
            ),
        };
        let _ = outbox.send(Purpose::Remind, owner, mail, now_unix);
        Ok(())
    }
}

fn not_enrolled() -> Response {
    Response::Refused(
        "only an enrolled device of an account may bind its address; run `itsanas register` on this machine first".to_owned(),
    )
}
