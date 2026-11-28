//! One round of sync: dialling what the round decided, a session with each
//! peer, and what is said about it.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_crypto::DeviceId;
use itsanas_discover::HouseholdKey;
use itsanas_folder::Folder;
use itsanas_net::{PeerClient, session};
use itsanas_node::contact::Contact;
use itsanas_policy::Scope as PolicyScope;

use super::{
    app_peers::{app_peers_known, ask_app_mode, dial_app_peers, note_app_peer},
    dialling::{Outcome, counts_as_contact, dial, dial_configured, dial_introduced, dial_listed},
    pledge::{check_standing, hold_the_pledge, local, owners, report_keeping, take_on_hosting},
    reachability::publish,
    reconciling::reconcile_once,
    shutdown::{halted, stops_for_vault},
    snapshot::{OUTAGE_QUIET, Outage},
};
use crate::{
    config::format_size,
    control::{self, Next, Steering},
    discovery::{self, Neighbourhood},
    node::Node,
};

/// The refused set the drain last printed, and when.
///
/// Its own slot rather than a key in [`REFUSALS_REPORTED`], whose keys are peer
/// labels: a peer configured under the drain's key would have shared its quiet.
pub(super) type DrainRefusal = (Vec<DeviceId>, usize, Instant);

pub(super) static DRAIN_REFUSAL_REPORTED: std::sync::Mutex<Option<DrainRefusal>> =
    std::sync::Mutex::new(None);

/// Whether the drain's refusals are worth a line, given the last one printed.
///
/// The same refused set is said once per [`OUTAGE_QUIET`]: the forged segment
/// stays in the vault and every loop drains it again. A different set is said
/// at once -- held back with the old one, a second forged chain pushed five
/// minutes after the first went unsaid for the rest of the half hour.
pub(super) fn drain_refusal_due(
    previous: Option<&DrainRefusal>,
    report: &itsanas_sync::SyncReport,
    now: Instant,
) -> bool {
    previous.is_none_or(|(devices, chains, last)| {
        *devices != report.refused_devices
            || *chains != report.refused_chains
            || refusal_due(Some(*last), now)
    })
}

/// Say a chain pushed into this vault was refused, held back as push refusals are.
pub(super) fn report_drain_refusals(report: &itsanas_sync::SyncReport) {
    let Ok(mut reported) = DRAIN_REFUSAL_REPORTED.lock() else {
        return;
    };
    match crate::describe_refused_chains(report, crate::ChainSource::Vault) {
        Some(refused) => {
            let now = Instant::now();
            if drain_refusal_due(reported.as_ref(), report, now) {
                println!("{refused}");
                *reported = Some((report.refused_devices.clone(), report.refused_chains, now));
            }
        }
        None => {
            *reported = None;
        }
    }
}

/// Say it once when a round reached nobody.
pub(super) fn say_if_alone(
    node: &Node,
    neighbourhood: &Neighbourhood,
    reached: &BTreeSet<DeviceId>,
    warned_alone: &mut bool,
) {
    // Say it once, rather than leaving someone watching a silent
    // terminal wondering whether anything is happening. Silence is the
    // correct output for a working daemon and the worst possible
    // output for one that has nobody to talk to.
    if reached.is_empty() && !*warned_alone {
        *warned_alone = true;
        if node.config.peers.is_empty() && neighbourhood.is_empty() {
            println!("no other machines found yet.");
            println!("  on this network: start the daemon on another machine and it is found");
            println!("  elsewhere:       itsanas peer add <host:port>");
        } else {
            println!(
                "{} machine(s) known, none reachable this round.",
                node.config.peers.len() + neighbourhood.len()
            );
        }
    } else if !reached.is_empty() {
        *warned_alone = false;
    }
}

/// One pass over every way this node knows of reaching a peer.
///
/// In order: the coordinator, if this round owes it anything; addresses
/// somebody typed into the configuration; the account's other devices where the
/// coordinator last said they were; and whatever announced itself on the local
/// network. Each is dialled with its device id pinned wherever one is known, so
/// an address that answers as somebody else is refused rather than trusted.
///
/// Returns who was reached, whether any of them counts as a contact
/// ([`counts_as_contact`]) and, if this round published, what it published:
/// whether that is worth asking to be probed is the caller's question.
#[allow(clippy::too_many_arguments)]
pub(super) fn one_round(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    contact: &mut Contact,
    bound: std::net::SocketAddr,
    outage: &mut Outage,
    scope: PolicyScope,
    folder: Option<&Folder>,
    interval: Duration,
) -> (BTreeSet<DeviceId>, bool, Option<String>) {
    if stops_for_vault(node, shutdown) {
        return (BTreeSet::new(), false, None);
    }
    hold_the_pledge(node);
    // The coordinator first, and only with a reason: this machine has not
    // published since it started, has moved, owes its hourly publication, or
    // last round reached a machine of the account at none of its addresses
    // (`contact.rs`). First because nothing it decides depends on who this
    // round reaches, and a publication that waited for the round would wait
    // behind every sync in it -- hours, on the first sync of a large folder,
    // during which nobody elsewhere can find this machine. It also fills the
    // book before the book is dialled. Failures here are ordinary: it may be
    // down, and a node whose peers are already known keeps working without it.
    let announced = publish(node, shutdown, contact, bound, outage);

    // The devices of other accounts this node hosts for, confirmed with the
    // coordinator before they store (§8 1c (ii)). After the contact above, so
    // a coordinator that just answered is the one asked.
    if !shutdown.load(Ordering::Relaxed) {
        check_standing(node);
    }

    // Placement for the whole round, after the coordinator so its claims are
    // the freshest this round will have, before the first dial so every push
    // of the round ranks with the same one (`placing`).
    let accounts = owners(node).accounts();
    let own = crate::placing::own_devices(
        contact.candidates().into_iter().map(|(device, _)| device),
        &accounts,
        node.store.owner(),
        node.store.members().unwrap_or_default(),
    );
    crate::placing::begin_round(
        node.config.placement,
        node.config.host_tier,
        &node.store,
        own,
        &accounts,
        itsanas_discover::now_unix(),
        interval.as_secs(),
    );

    // Configured peers next: somebody typed those in, so they are
    // wanted even if they are also on the local network — and being in
    // the configuration is itself the evidence that they are real, so
    // they are confirmed on contact without having to earn it.
    // Hosts of other accounts that proved themselves this round, at the
    // address that reached them: relays of last resort (§8 0v).
    let mut hosts = Vec::new();
    let mut reached = dial_configured(node, shutdown, neighbourhood, scope, &mut hosts);

    // The account's devices at the addresses the coordinator last gave, the
    // one that last worked first. Pinned: the coordinator supplies addresses
    // and is not trusted to say who lives at one.
    dial_listed(
        node,
        shutdown,
        neighbourhood,
        contact,
        &mut reached,
        scope,
        bound,
    );

    // Everybody so far was typed into the configuration or is a machine of
    // the account the coordinator listed: a contact. The strangers below
    // count only as `counts_as_contact` says.
    let mut in_touch = !reached.is_empty();

    // What the account's own machines just sent is written now, not after the
    // other accounts' hosts below, where the slow and older builds are (a
    // laptop's files waited 20 min behind them on 2026-10-06).
    if let Some(folder) = folder.filter(|_| !control::paused_on_disk(&node.home)) {
        reconcile_once(node, folder, false);
    }

    // The other machines in application mode, before every other host: they
    // asked for it, and the program's newest file is waiting for two of them
    // (HANDOVER §8 00).
    if local(node).app.load(Ordering::Relaxed) {
        dial_app_peers(node, shutdown, &mut reached, scope);
    }

    in_touch |= dial_introduced(
        node,
        shutdown,
        neighbourhood,
        contact,
        &mut reached,
        &mut hosts,
        scope,
    );

    let mut strangers_dialled = 0usize;
    for candidate in neighbourhood.dial_order(&HouseholdKey::of(&node.user)) {
        if halted(node, shutdown) {
            break;
        }
        if reached.contains(&candidate.device) {
            continue;
        }

        // Dialling costs a handshake and a round trip, and minting the
        // identity that provoked it cost an attacker nothing. Confirmed
        // peers are always dialled; unconfirmed ones are rationed, so a
        // flood cannot consume the interval that real syncing needs.
        let known = neighbourhood.is_confirmed(&candidate.device);
        if !known {
            if strangers_dialled >= discovery::NEW_PEERS_PER_ROUND {
                continue;
            }
            strangers_dialled += 1;
        }

        // A stranger that does not answer is not news — this network is
        // built out of machines that are usually off. One of your own
        // machines failing to answer is worth saying.
        let outcome = sync_once(
            node,
            &candidate.address.to_string(),
            None,
            Some(candidate.device),
            candidate.mine,
            false,
            scope,
        );

        if let Some(outcome) = outcome {
            reached.insert(outcome.device);
            in_touch |= counts_as_contact(candidate.mine, known, outcome.earned_trust);
            // Authenticating proves possession of a keypair that cost
            // nothing to generate. Only doing something a real host
            // does — storing our data, or serving us our own work —
            // earns a place a stranger cannot take.
            if outcome.earned_trust {
                neighbourhood.confirm(outcome.device);
                hosts.push((outcome.device, candidate.address.to_string()));
            }
        } else {
            neighbourhood.unanswered(&candidate.device, candidate.address);
        }
    }
    contact.hosts_reached(hosts, Instant::now());
    // A machine of the account that answered nowhere -- not at the book's
    // addresses, not on the LAN, not as a configured peer -- may have moved
    // or restarted on another port: the next round reads the coordinator
    // again instead of waiting for the hourly publication (`contact.rs`,
    // `REREAD_AFTER_MISS`). A round cut short dialled nobody it skipped.
    if !halted(node, shutdown) {
        contact.round_reached(&reached, Instant::now());
    }

    (reached, in_touch, announced)
}

/// What this turn of the loop does, and whether this machine says it runs in
/// application mode meanwhile: only while it is on and the machine syncs in
/// full. On a metered link nobody should dial it every minute, and paused it
/// answers nobody's files.
pub(super) fn decide(
    node: &Node,
    steering: &mut Steering,
    scope: PolicyScope,
    (any, full): (Option<Instant>, Option<Instant>),
    base: Duration,
) -> Next {
    let rounds = control::Rounds { any, full };
    let app = steering.app_mode() && scope == PolicyScope::Everything && !steering.paused();
    local(node).app.store(app, Ordering::Relaxed);
    if !scope.connects() {
        return Next::Wait;
    }
    steering.next_with(Instant::now(), rounds, base, app && app_peers_known(node))
}

/// One round against one peer. Never propagates an error.
///
/// A peer being unreachable is the normal state of this network, not a fault:
/// the whole design is built around machines that are usually off. Treating it
/// as an error would mean the daemon exits every time someone shuts a laptop.
///
/// `expect` pins which device must answer. The device is reported even when the
/// round then failed — reaching a machine and falling out with it says nothing
/// about whether the machine exists — but `earned_trust` is only set when the
/// peer stored something of ours or served us our own work.
pub(super) fn sync_once(
    node: &Node,
    peer: &str,
    through: Option<&(DeviceId, String)>,
    expect: Option<DeviceId>,
    announce_failure: bool,
    ask_presences: bool,
    scope: PolicyScope,
) -> Option<Outcome> {
    // Bounded as a whole: one peer that stalls must not hold the round, and
    // with it the folder pass and every other peer (PEER_SESSION_BUDGET).
    let mut client = match dial(node, peer, through, expect) {
        Ok(client) => client,
        Err(error) => {
            if announce_failure {
                println!("{peer}: unreachable ({error})");
            }
            crate::placing::unreached(&node.store, expect);
            return None;
        }
    };
    let answered = client.peer_device();
    present_claim(node, peer, &mut client);
    let app_answer = ask_app_mode(node, peer, &mut client);

    // Make the peer prove it still holds what the ledger says it took, a
    // handful of chunks per round. Without this the ledger is a list of
    // promises with nothing checking any of them, and a node believes its data
    // is on three machines while two of them threw it away.
    //
    // A failure is news: it withdraws that record, so the chunk shows as
    // under-replicated, and the operator should know a peer is not holding what
    // it said. Nothing is deleted and nobody is blocked.
    // Fetch back anything this disk has lost. Bounded both ways: a slice of
    // the live chunks examined, a handful of chunks asked for. The peer is
    // doing this for free, and a node that lost a whole disk wants a restore,
    // not a repair.
    match session::repair(&node.store, &mut client, session::REPAIR_SCAN_PER_ROUND) {
        Ok(report) if report.changed_anything() => {
            if report.restored > 0 {
                println!(
                    "{peer}: restored {} chunk(s) this disk had lost",
                    report.restored
                );
            }
            if report.forged > 0 {
                println!(
                    concat!(
                        "{}: answered {} repair request(s) with bytes that ",
                        "are not what was asked for. Those records are withdrawn."
                    ),
                    peer, report.forged
                );
            }
        }
        Ok(_) => {}
        Err(error) => println!("{peer}: could not repair ({error})"),
    }

    match session::audit(&node.store, &mut client, session::CHALLENGES_PER_ROUND) {
        Ok(report) if report.found_a_liar() => {
            println!(
                "{peer}: FAILED {} of {} storage challenges — it is not holding \
                 what it said. Those chunks now count as unreplicated.",
                report.failed, report.asked
            );
            // Say it once, when the sanction actually starts, rather than
            // every round afterwards.
            if let Some(record) = report.record
                && record.consecutive_failures == itsanas_store::FAILURES_BEFORE_PAUSE
            {
                // `concat!` rather than a trailing backslash: fmt eats the
                // backslash and leaves the indentation inside the string, which
                // is how this line spent two commits telling an operator
                // "receives          the log". It also blocks implicit capture,
                // hence the named argument. `scripts/check-messages.py` guards
                // the rest of the codebase against the same thing.
                println!(
                    concat!(
                        "  {peer} is no longer being sent new data. It still ",
                        "receives the log and one chunk a round to answer for. ",
                        "{owed} answered rounds clear this."
                    ),
                    peer = peer,
                    owed = record.rounds_to_reinstatement()
                );
            }
        }
        Ok(report) if report.probing && report.confirmed > 0 => {
            println!("{peer}: answered for the chunk it was given — no longer paused.");
        }
        Ok(_) => {}
        // An audit that could not run is not an accusation. The peer may have
        // hung up, or this device may no longer hold a copy to check against.
        Err(error) => println!("{peer}: could not audit ({error})"),
    }

    // The scope the policy chose. On a metered link this exchanges the log and
    // downloads nothing, which is the difference between a tethered laptop
    // costing kilobytes a day and costing whatever the store happens to weigh.
    // `Nothing` maps to `Metadata`, not to `Everything`. It is unreachable
    // today — `conditions()` never produces `Network::None` or `Power::Low`
    // — and the version of this that said so in a comment mapped it to
    // `Everything`, so the day somebody wires up metered detection the state
    // meaning "do nothing" would have become the state meaning "download the
    // lot". A fallback belongs on the cheap side of the choice it cannot make.
    let wire = match scope {
        PolicyScope::Metadata | PolicyScope::Nothing => session::Scope::Metadata,
        PolicyScope::Everything => session::Scope::Everything,
    };
    // The round's placement, the same `Arc` for every push of the round;
    // `None` in `everyone`, which is the push as it always was.
    let placement = crate::placing::current();
    let earned_trust = match crate::keeping::round_placed(
        &node.store,
        &node.vault,
        &node.config.keeping(),
        &mut client,
        wire,
        placement.as_deref(),
    ) {
        Ok((report, keeping)) => {
            report_round(peer, &report);
            report_keeping(peer, &keeping);
            // A quiet round is the common case. Saying so every five minutes
            // would fill a journal with nothing and train the operator to
            // ignore it.
            report.peer_earned_trust()
        }
        Err(error) => {
            println!("{peer}: failed ({error})");
            false
        }
    };

    take_on_hosting(node, peer, &mut client);
    note_app_peer(node, answered, peer, through, app_answer, earned_trust);

    // One exchange on a connection already open. A failure costs this round
    // the answer and nothing else: the coordinator remains the other source.
    // Only of a machine the book lists: found on the LAN, a stranger is
    // pinned too, and its answer would be read for nothing (Rodin).
    let relayed = if ask_presences {
        client.presences().unwrap_or_else(|error| {
            println!("{peer}: could not ask where the account's machines are ({error})");
            None
        })
    } else {
        None
    };

    Some(Outcome {
        device: answered,
        earned_trust,
        relayed,
    })
}

/// Tell a host whose this device is and what it offers, before offering it
/// anything (§8 1c). A refusal is said once here rather than once per chunk.
///
/// The host answers with its own claim, kept in this daemon's book: a machine
/// behind a router is never dialled, so this is the only way it learns whose
/// the hosts it pulls for are, and what they offer.
pub(crate) fn present_claim(node: &Node, peer: &str, client: &mut PeerClient) {
    let mut siblings = Vec::new();
    match client.present_claim(node.claim_bytes(itsanas_discover::now_unix())) {
        Ok(Some(theirs)) => {
            // The host's own claim: a sibling of this account when it says so,
            // which is the only way a machine behind a router learns one it
            // dials and that never dials it.
            siblings.push(theirs.clone());
            if let Err(why) = owners(node).take(client.peer_device(), &theirs, &node.vault) {
                println!("{peer}: its own claim does not check out ({why}); not hosting for it");
            }
        }
        Ok(None) => {}
        Err(error) => {
            println!("{peer}: refused this device's claim ({error}); it will store nothing here");
        }
    }
    // This account's roster, before any chain is applied this session: the
    // host itself if it is ours, the siblings it knows, and the ones that
    // presented here. Asked after presenting, which is what the host answers
    // on.
    siblings.extend(client.claims(node.store.owner()).unwrap_or_default());
    if let Err(error) = node.admit_roster(&siblings) {
        println!("{peer}: this account's device list could not be updated ({error})");
    }
}

/// When each peer's refusals were last reported.
///
/// A host with pledge 0 refuses every round, and so does a stranger on the
/// network that pledged nothing: printing it every round is a line every five
/// minutes per peer for ever, which is the journal nobody reads that
/// `OUTAGE_QUIET` exists to prevent for the coordinator.
pub(super) static REFUSALS_REPORTED: std::sync::Mutex<std::collections::BTreeMap<String, Instant>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Whether a refusal is worth a line, given when the last one was printed.
///
/// The first always is; after that, once per [`OUTAGE_QUIET`].
pub(crate) fn refusal_due(previous: Option<Instant>, now: Instant) -> bool {
    previous.is_none_or(|last| now.duration_since(last) >= OUTAGE_QUIET)
}

/// What a round with one peer moved, and what that peer refused.
///
/// A quiet round prints nothing, but a refusal is printed even when nothing
/// moved: a round where everything was refused is the round that most needs
/// saying, and it used to be silent because it changed nothing. Repeats are
/// held back by [`refusal_due`]; a round without refusals resets it, so a peer
/// that starts refusing again is reported at once.
pub(super) fn report_round(peer: &str, report: &session::RoundReport) {
    if report.changed_anything() {
        println!(
            "{peer}: sent {} ({} chunks, {} segments), received {} files, {} conflicts{}",
            format_size(report.push.bytes_sent),
            report.push.chunks_accepted,
            report.push.segments_accepted,
            report.pull.adopted,
            report.pull.conflicted,
            if report.pull.deferred > 0 {
                format!(", {} deferred", report.pull.deferred)
            } else {
                String::new()
            }
        );
    }
    if let Some(no_room) = crate::describe_no_room(&report.pull) {
        println!("{peer}: {no_room}");
    }
    if let Some(refused) = crate::describe_refused_chains(&report.pull, crate::ChainSource::Peer) {
        println!("{peer}: {refused}");
    }
    let Ok(mut reported) = REFUSALS_REPORTED.lock() else {
        return;
    };
    match crate::describe_refusal(&report.push) {
        Some(refused) => {
            let now = Instant::now();
            if refusal_due(reported.get(peer).copied(), now) {
                println!("{peer}: {refused}");
                reported.insert(peer.to_owned(), now);
            }
        }
        None => {
            reported.remove(peer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drain says a refused set once per half hour, and a new one at once.
    ///
    /// The forged segment stays in the vault and every loop drains it again,
    /// so the same set is held back; a second forged chain is news, and keyed
    /// on one fixed label it went unsaid for the rest of the window.
    /// Sabotage: drop the comparison of the refused devices (or of the count)
    /// in `drain_refusal_due` -- the second chain then waits half an hour.
    #[test]
    fn the_drain_says_a_new_refused_chain_at_once_and_the_same_one_once_a_window() {
        let first = itsanas_sync::SyncReport {
            refused_chains: 1,
            refused_devices: vec![DeviceId::from_bytes([0xA1; 32])],
            ..itsanas_sync::SyncReport::default()
        };
        let start = Instant::now();
        assert!(
            drain_refusal_due(None, &first, start),
            "the first refusal went unsaid"
        );

        let said = (first.refused_devices.clone(), first.refused_chains, start);
        let soon = start + Duration::from_secs(300);
        assert!(
            !drain_refusal_due(Some(&said), &first, soon),
            "the same refused chain was said again five minutes later: every loop repeats it"
        );
        assert!(
            drain_refusal_due(Some(&said), &first, start + OUTAGE_QUIET),
            "the same refused chain was never said again after the quiet window"
        );

        let another = itsanas_sync::SyncReport {
            refused_chains: 1,
            refused_devices: vec![DeviceId::from_bytes([0xB2; 32])],
            ..itsanas_sync::SyncReport::default()
        };
        assert!(
            drain_refusal_due(Some(&said), &another, soon),
            "a different device's forged chain, pushed five minutes after the first, went \
             unsaid for the rest of the half hour"
        );
        // Past the names a report carries, only the count shows a new chain.
        let more = itsanas_sync::SyncReport {
            refused_chains: 2,
            refused_devices: first.refused_devices.clone(),
            ..itsanas_sync::SyncReport::default()
        };
        assert!(
            drain_refusal_due(Some(&said), &more, soon),
            "a chain refused beyond the named ones went unsaid for the rest of the half hour"
        );
    }
}
