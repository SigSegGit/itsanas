//! The per-address budget on questions about a name, and which requests it
//! counts.

use std::net::IpAddr;
use std::time::Instant;

use super::{NAME_BLOCK_SHARES, NAME_TRACKED, NAME_WINDOW};
use crate::protocol::Request;
use crate::service::EscrowLimiter;

/// The requests whose answer says something about a name.
pub(super) const fn is_about_a_name(request: &Request) -> bool {
    matches!(
        request,
        Request::Lookup { .. }
            | Request::GetEscrow { .. }
            | Request::Register(_)
            | Request::RegisterInvited { .. }
    )
}

/// [`NAME_QUESTIONS`](super::NAME_QUESTIONS) per address (an IPv6 /64), and [`NAME_BLOCK_SHARES`]
/// times that per IPv6 /48.
#[derive(Debug)]
pub struct NameBudget {
    addresses: EscrowLimiter,
    blocks: EscrowLimiter,
}

impl NameBudget {
    /// `allowed` questions per address per [`NAME_WINDOW`].
    #[must_use]
    pub fn new(allowed: u32) -> Self {
        Self::with_capacity(allowed, NAME_TRACKED)
    }

    /// The same, remembering at most `tracked` addresses and as many blocks.
    #[must_use]
    pub fn with_capacity(allowed: u32, tracked: usize) -> Self {
        Self {
            addresses: EscrowLimiter::with(allowed, NAME_WINDOW, tracked),
            blocks: EscrowLimiter::with(
                allowed.saturating_mul(NAME_BLOCK_SHARES),
                NAME_WINDOW,
                tracked,
            ),
        }
    }

    /// Whether `from` may ask one more question about a name at `now`; counts
    /// it when it may.
    ///
    /// Both tables are checked before either is charged: a /64 whose /48 is
    /// spent must take no row, or one /48 fills the address table and every
    /// address not yet seen is refused for the hour.
    pub fn allow(&mut self, from: IpAddr, now: Instant) -> bool {
        let address = address_key(from);
        let block = block_key(from);
        if !self.addresses.would_allow(&address, now)
            || block
                .as_ref()
                .is_some_and(|block| !self.blocks.would_allow(block, now))
        {
            return false;
        }
        self.addresses.allow(&address, now);
        if let Some(block) = block {
            self.blocks.allow(&block, now);
        }
        true
    }
}

/// The /48 an IPv6 address is in; `None` for an IPv4 one.
fn block_key(from: IpAddr) -> Option<String> {
    let IpAddr::V6(v6) = from else {
        return None;
    };
    if v6.to_ipv4_mapped().is_some() {
        return None;
    }
    let s = v6.segments();
    Some(format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2]))
}

/// The requests about a name that are only a question, counted whatever the
/// answer: unlike a registration, nothing an honest machine does repeats them.
pub(super) const fn is_a_question(request: &Request) -> bool {
    matches!(request, Request::Lookup { .. } | Request::GetEscrow { .. })
}

/// The key [`NAME_QUESTIONS`](super::NAME_QUESTIONS) is counted under: an IPv4 address, or the /64
/// an IPv6 one is in. A subscriber is handed a whole /64 and can use any of
/// its 2^64 addresses, so counting each would count nothing.
fn address_key(from: IpAddr) -> String {
    match from {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(
            || {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            },
            |v4| v4.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE ATTACK: one question each from many /64s of one /48, which a
    /// provider hands out by the thousand. Each /64 stays inside its own
    /// budget; the /48 does not.
    #[test]
    fn red_team_many_ipv6_blocks_of_one_site_share_its_budget() {
        let mut budget = NameBudget::new(1);
        let now = Instant::now();
        let shares = usize::try_from(NAME_BLOCK_SHARES).expect("small");
        let in_site = |index: usize| -> IpAddr {
            format!("2001:db8:7:{index:x}::1")
                .parse()
                .expect("an address")
        };
        for index in 0..shares {
            assert!(
                budget.allow(in_site(index), now),
                "/64 number {index} was refused its first question"
            );
        }
        assert!(
            !budget.allow(in_site(shares), now),
            "one more /64 of the same /48 bought one more question: a list costs a script a /64 per name"
        );
        assert!(
            budget.allow("2001:db8:8::1".parse().expect("an address"), now),
            "another site was refused for the first one's questions"
        );
        assert!(
            budget.allow("203.0.113.9".parse().expect("an address"), now),
            "an IPv4 address was counted in an IPv6 site"
        );
    }

    /// THE ATTACK: one /48 asks one question from each of its /64s. Past the
    /// /48's budget every one is refused, and if the refusal still recorded
    /// the /64, the address table filled and every address it had not seen,
    /// IPv4 or IPv6, was refused name questions and recovery for the hour.
    /// Sabotage: record the /64 before checking the /48.
    #[test]
    fn red_team_one_ipv6_site_cannot_fill_the_address_table() {
        let tracked = 64;
        let mut budget = NameBudget::with_capacity(1, tracked);
        let now = Instant::now();
        for index in 0..tracked {
            let from: IpAddr = format!("2001:db8:7:{index:x}::1")
                .parse()
                .expect("an address");
            let _ = budget.allow(from, now);
        }
        assert!(
            budget.allow("203.0.113.9".parse().expect("an address"), now),
            "one IPv6 site filled the address table and locked a stranger out"
        );
    }

    /// A full escrow table refuses every recovery of a name it is not already
    /// counting, and the name budget is what prices filling it. The ceiling is
    /// written in the ROADMAP with its arithmetic; this keeps the numbers
    /// there equal to the constants here. Sabotage: change `ESCROW_TRACKED`.
    #[test]
    fn a_full_escrow_table_is_a_documented_ceiling() {
        use super::super::NAME_QUESTIONS;
        let roadmap = include_str!("../../../../docs/ROADMAP.md");
        let per_hour = u64::try_from(crate::service::ESCROW_TRACKED).expect("small")
            * (NAME_WINDOW.as_secs() / crate::service::ESCROW_WINDOW.as_secs());
        let ipv4 = per_hour.div_ceil(u64::from(NAME_QUESTIONS));
        let ipv6 = per_hour.div_ceil(u64::from(NAME_QUESTIONS * NAME_BLOCK_SHARES));
        for said in [
            format!("`ESCROW_TRACKED` ({})", crate::service::ESCROW_TRACKED),
            format!("{ipv4} IPv4 addresses or {ipv6} IPv6 /48s"),
        ] {
            assert!(
                roadmap.contains(&said),
                "the ROADMAP's escrow ceiling no longer says {said:?}"
            );
        }
    }

    /// An IPv6 subscriber holds a whole /64; counted address by address, one
    /// line holds 2^64 budgets.
    #[test]
    fn red_team_every_address_of_one_ipv6_block_shares_one_name_budget() {
        let key = |text: &str| address_key(text.parse().expect("an address"));
        assert_eq!(
            key("2001:db8:1:2::1"),
            key("2001:db8:1:2:ffff:ffff:ffff:ffff"),
            "two addresses of one /64 were counted apart"
        );
        assert_ne!(
            key("2001:db8:1:2::1"),
            key("2001:db8:1:3::1"),
            "two /64s shared one count"
        );
        assert_eq!(
            key("::ffff:203.0.113.9"),
            key("203.0.113.9"),
            "an IPv4 address reaching a dual-stack socket was counted apart from itself"
        );
        assert_ne!(key("203.0.113.9"), key("203.0.113.10"));
    }
}
