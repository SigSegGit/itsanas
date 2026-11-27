//! What the daemon leaves behind for `itsanas status` and the trays: the
//! snapshot, how often it is written, and the outage it reports.

use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::pledge::{SIBLING_SEEN, local};
use crate::node::Node;

/// The longest the snapshot goes unwritten between rounds, or a round when
/// that is shorter ([`snapshot_due`]).
///
/// Its stamp is what says the daemon is alive: an icon reads `stale` past two
/// intervals (`brief_status`), the fleet's S row reads its age, and the
/// steering tests wait for a newer one as proof a pass of the loop went by.
/// Half a minute keeps all three; every pass of the loop (two seconds) cost an
/// idle node most of its disk writes (2026-10-27).
pub(super) const SNAPSHOT_EVERY: Duration = Duration::from_secs(30);

/// When the snapshot was last written, and for which interval: what
/// [`snapshot_due`] decides from, once a pass of the loop.
#[derive(Default)]
pub(super) struct SnapshotPace(Option<(Instant, Duration)>);

/// What the rounds found that the store does not keep: the snapshot's header
/// carries it to the icon, which cannot open the store.
#[derive(Default)]
pub(super) struct Seen {
    /// When a round of this process last reached another machine that counts
    /// ([`counts_as_contact`]), unix seconds. Not the listener's witness: it
    /// counts every connection accepted, a port scanner's and this machine's
    /// own included.
    pub(super) reached: Option<u64>,
    /// Why the vault's last write probe failed, while it fails.
    pub(super) unwritable: Option<String>,
}

impl SnapshotPace {
    /// Write the snapshot if it is due; `news` when this pass ran a round or
    /// took in a change.
    pub(super) fn after(
        &mut self,
        node: &Node,
        interval: Duration,
        bound: std::net::SocketAddr,
        news: bool,
        seen: &Seen,
    ) {
        let now = Instant::now();
        if snapshot_due(self.0, now, interval, news) {
            write_snapshot(node, interval, bound, seen);
            self.0 = Some((now, interval));
        }
    }
}

/// Whether this pass of the loop writes the snapshot: the first time, when
/// `news` (a round ran, the folder changed), when the interval is not the one
/// last written (the stamp is read against it), and otherwise once
/// [`SNAPSHOT_EVERY`] -- or a round, if shorter -- has passed since the last.
/// `last` is when the last one was written and for which interval.
pub(super) fn snapshot_due(
    last: Option<(Instant, Duration)>,
    now: Instant,
    interval: Duration,
    news: bool,
) -> bool {
    let Some((at, written_for)) = last else {
        return true;
    };
    news || written_for != interval
        || now.saturating_duration_since(at) >= SNAPSHOT_EVERY.min(interval)
}

/// How long a continuing coordinator outage stays quiet between complaints.
///
/// A coordinator that is down is down for hours, not for one round. Saying so
/// every round produces roughly six hundred identical lines a day, and a
/// journal nobody reads is the same as no journal on the morning something else
/// breaks. Found by running the daemon against a coordinator that was not
/// there, which is exactly the state MVP acceptance test I describes as normal.
pub(super) const OUTAGE_QUIET: Duration = Duration::from_secs(30 * 60);

/// [`OUTAGE_QUIET`] for tests outside this module, which cannot name a private
/// constant.
#[cfg(test)]
pub(crate) const OUTAGE_QUIET_FOR_TESTS: Duration = OUTAGE_QUIET;

/// What has already been said about the coordinator, so it is not said again.
#[derive(Debug)]
pub(super) struct Outage {
    /// Rounds that have failed since the last time anything was printed.
    silent_rounds: usize,
    /// When the next complaint is allowed.
    next_complaint: Instant,
    /// Whether the last round reached it.
    was_reachable: bool,
}

impl Outage {
    pub(super) fn new() -> Self {
        Self {
            silent_rounds: 0,
            next_complaint: Instant::now(),
            was_reachable: true,
        }
    }

    /// Report a failed round, at most once per [`OUTAGE_QUIET`].
    pub(super) fn failed(&mut self, why: &str) {
        self.silent_rounds += 1;
        if Instant::now() < self.next_complaint {
            return;
        }
        if self.silent_rounds == 1 {
            println!("coordinator: unreachable ({why})");
            println!("  Peers already known keep syncing. New machines cannot be found.");
        } else {
            println!(
                "coordinator: still unreachable after {} rounds ({why})",
                self.silent_rounds
            );
        }
        self.next_complaint = Instant::now() + OUTAGE_QUIET;
        self.was_reachable = false;
    }

    /// Report a round that reached it, but only if the last one did not.
    pub(super) fn succeeded(&mut self) {
        if !self.was_reachable {
            println!("coordinator: reachable again");
        }
        self.silent_rounds = 0;
        self.next_complaint = Instant::now();
        self.was_reachable = true;
    }
}

/// Leave behind what `itsanas status` would have said.
///
/// The store allows one writer and this process is it, so while the daemon runs
/// every command that opens the store refuses -- including the one whose whole
/// job is to say what the node is doing. Somebody who has just dropped a file
/// into their synced folder and types `itsanas status` should not be told to
/// stop their node in order to find out whether it worked.
///
/// Written to a temporary name and renamed, so a reader never sees half of it,
/// and never sees an empty file because this process was interrupted between
/// truncating and writing. That failure is not hypothetical: the Windows
/// provisioning script destroyed a passphrase exactly that way on 2026-09-06.
///
/// Failing to write it is not worth interrupting a sync round over, and not
/// worth a line in the log every five minutes either. `status` says how old the
/// snapshot is, so a stale one announces itself.
/// The snapshot's first line: when, how often, and which program wrote it.
///
/// The version after what every reader parses (`brief_status`, `status`,
/// `snapshot_counts`, the fleet agent): it is the version of the *running
/// daemon*, which is what an `update` that restarted nothing leaves behind --
/// three laptop daemons on 0.1.8 while the program on disk was newer (hand
/// test, 2026-10-22). The fleet run compares it across machines. Then the
/// copies, when anything is stored (`safety.rs`): what the icon colours. Then
/// when a round last reached another machine, or since when none has
/// (`safety::reached_words`), ` unwritable` while the vault refuses a write
/// ([`Seen`]), then a failed challenge and a disk below the reserve
/// (`safety::alert_words`).
pub(super) fn snapshot_header(
    now: u64,
    every: u64,
    copies: Option<crate::safety::Copies>,
    reached: &str,
    unwritable: bool,
    alerts: &str,
) -> String {
    format!(
        "snapshot {now} every {every} version {}{}{reached}{}{alerts}",
        env!("CARGO_PKG_VERSION"),
        copies.map(|c| c.render()).unwrap_or_default(),
        if unwritable { " unwritable" } else { "" }
    )
}

pub(super) fn write_snapshot(
    node: &Node,
    interval: Duration,
    bound: std::net::SocketAddr,
    seen: &Seen,
) {
    // Read before the copies are counted: a file taken in between is then
    // counted and not yet named, never named and not counted.
    let newest = newest_words(node);
    let Ok((text, counted)) =
        crate::render_status_and_copies(node, Some(&bound.to_string()), seen.unwritable.as_deref())
    else {
        return;
    };
    let now = itsanas_discover::now_unix();
    // When the shortfall began is carried by the file itself, so a restarted
    // daemon neither forgets an old one nor grants a new grace to it.
    let final_path = node.home.join(crate::node::SNAPSHOT);
    let old = std::fs::read_to_string(&final_path).unwrap_or_default();
    let old = old.lines().next().unwrap_or("");
    let copies = counted.and_then(|c| {
        crate::safety::Copies::this_round(
            c.have,
            c.want,
            true,
            crate::safety::Copies::parse(old),
            now,
        )
    });
    // So is the last contact (`safety::reached_words`).
    let reached = crate::safety::reached_words(seen.reached, old, now);
    // `render_status_and_copies` read this store a moment ago; a read that
    // fails now must not cost the snapshot, which is what says the node lives.
    let alerts = crate::safety::alert_words(
        crate::machines_failing(node, now).unwrap_or(0),
        itsanas_node::pledges::low_on_disk(&node.home),
    );
    // What the icon says in a colour, `status` says in words.
    let silence = crate::safety::isolated_for(&reached, now, interval.as_secs())
        .map(crate::safety::silence_line)
        .unwrap_or_default();

    // The first line is the time it was taken, in seconds, and the reader turns
    // that into an age. Storing the age itself would be wrong the moment it was
    // written, and formatting a date needs a calendar this project does not
    // carry a dependency for -- while "four minutes ago" is both what somebody
    // wants to know and arithmetic.
    // Then the interval, so a reader can tell a late snapshot from a stale
    // one: "stale" is two intervals without a round, and the interval is this
    // process's to know (`--interval`, the metered plan), nobody else's.
    let stamped = format!(
        "{}\n{silence}{text}",
        snapshot_header(
            now,
            interval.as_secs(),
            copies,
            &reached,
            seen.unwritable.is_some(),
            &format!("{alerts}{newest}")
        )
    );

    let pending = node.home.join(format!("{}.new", crate::node::SNAPSHOT));
    if std::fs::write(&pending, stamped).is_ok() {
        let _ = std::fs::rename(&pending, &final_path);
    }
}

/// ` newest M` on the snapshot's first line, in application mode only
/// (HANDOVER §8 00): M is the newest modification time, in Unix seconds, among
/// the files the `copies` count on the same line covers. A program that writes
/// one file at a time into the folder reads its newest file as on `HAVE` other
/// machines once `newest` has reached that file's modification time. Empty
/// otherwise: the files are read once more per snapshot, a cost only the
/// machines that asked for the line pay.
pub(super) fn newest_words(node: &Node) -> String {
    if !local(node).app.load(Ordering::Relaxed) {
        return String::new();
    }
    node.store
        .entries()
        .ok()
        .and_then(|entries| entries.iter().map(|(_, entry)| entry.modified_unix).max())
        .map(|newest| format!(" newest {newest}"))
        .unwrap_or_default()
}

/// When this node last reached the account, after a round: now if the round
/// did, else when one of the account's machines last dialled in, else as it was.
pub(super) fn last_contact(in_touch: bool, before: Option<u64>) -> Option<u64> {
    if in_touch {
        return Some(itsanas_discover::now_unix());
    }
    let inbound = SIBLING_SEEN.load(Ordering::Relaxed);
    if inbound > 0 {
        return before.max(Some(inbound));
    }
    before
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::DeviceId;

    use super::super::pledge::probe_vault;

    #[test]
    fn the_snapshot_says_which_version_wrote_it_and_still_reads_as_before() {
        let header = snapshot_header(1_792_000_000, 300, None, "", false, "");
        assert!(
            header.ends_with(&format!(" version {}", env!("CARGO_PKG_VERSION"))),
            "the snapshot does not say which program wrote it: a daemon an update left \
             behind looks like the others ({header})"
        );
        // `brief_status` reads the words in this order; the tray's colour
        // comes from it.
        let words: Vec<&str> = header.split_whitespace().take(4).collect();
        assert_eq!(words, ["snapshot", "1792000000", "every", "300"]);
        let text = format!("{header}\naccount\n  files           3\n");
        assert_eq!(
            crate::setup::verify::snapshot_counts(&text),
            Some((1_792_000_000, 3)),
            "the version broke the reader of the stamp and the file count"
        );
    }

    /// The idle bench of 2026-10-27, read with inotify: `status.snapshot`
    /// was rewritten at every pass of the loop, every two seconds, whatever
    /// the interval -- 150 rewrites a five-minute round, some 4 KB of
    /// journal each, the largest share of an idle node's disk writes (164 KB
    /// a round at a one-minute round, against 60 at ten seconds), and the
    /// whole status, copies included, counted again each time. A day of
    /// two-second passes at the default round: written after every round,
    /// at once when the folder changed or the interval did, otherwise at
    /// most every [`SNAPSHOT_EVERY`] -- and never left longer, so a stale
    /// icon still means a stuck daemon.
    #[test]
    fn red_team_an_idle_daemon_writes_its_snapshot_a_few_times_a_round_not_every_pass() {
        let start = Instant::now();
        let pass = Duration::from_secs(2);
        let interval = Duration::from_secs(300);
        let passes = 24 * 3600 / 2;
        let mut last: Option<(Instant, Duration)> = None;
        let (mut writes, mut longest) = (0u32, Duration::ZERO);
        for i in 0..passes {
            let now = start + pass * i;
            // Off the half-minute grid, so a round's write is not the
            // heartbeat's by chance.
            let round = i % 150 == 7;
            if snapshot_due(last, now, interval, round) {
                if let Some((at, _)) = last {
                    longest = longest.max(now - at);
                }
                writes += 1;
                last = Some((now, interval));
            } else {
                assert!(
                    !round,
                    "pass {i}: a round ran and the snapshot was not written"
                );
            }
        }
        assert!(
            writes <= 3_200,
            "{writes} snapshots in a day of five-minute rounds: written at nearly every \
             two-second pass, 43200 a day"
        );
        assert!(
            longest <= SNAPSHOT_EVERY,
            "the snapshot went {longest:?} unwritten: an icon reads a live daemon as stale"
        );

        let now = start + pass * passes;
        let fresh = Some((now, interval));
        assert!(
            snapshot_due(fresh, now, Duration::from_secs(3600), false),
            "the interval changed and the snapshot kept the old one: a daemon moved to an \
             hourly round reads as stale a minute later"
        );
        let short = Duration::from_secs(10);
        assert!(
            snapshot_due(Some((now, short)), now + short, short, false),
            "at a ten-second round the snapshot waited longer than a round"
        );
        assert!(
            !snapshot_due(fresh, now + pass, interval, false),
            "nothing happened two seconds later and the snapshot was written again"
        );
    }

    /// The wiring of MVP.md test L, through the daemon's own writer: a node
    /// holding a file and no peer writes `copies 0 of 2` with no grace (it
    /// was never safe), the icon reads `exposed`, and the next round keeps
    /// that. The version is still read where the fleet agent reads it.
    /// Sabotage: the header without the copies -- `healthy`, red.
    #[test]
    fn red_team_the_snapshot_a_lone_node_writes_turns_its_icon_from_green() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        node.store
            .write_file("notes.txt", b"only here so far")
            .expect("write");
        let bound: std::net::SocketAddr = "127.0.0.1:9797".parse().unwrap();
        let interval = Duration::from_secs(300);

        write_snapshot(&node, interval, bound, &Seen::default());
        let text = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
        let header = text.lines().next().unwrap_or("").to_owned();
        assert!(
            header.contains(&format!(
                " version {} copies 0 of 2 reached never since ",
                env!("CARGO_PKG_VERSION")
            )),
            "the snapshot of a node with a file and no peer does not say it has no copy \
             elsewhere: {header}"
        );
        let now = itsanas_discover::now_unix();
        let brief = crate::brief_status(&home, true, now);
        assert!(
            brief.starts_with("exposed ") && brief.ends_with(" 0 2"),
            "the icon of a node whose only copy is its own disk reads {brief:?}"
        );
        write_snapshot(&node, interval, bound, &Seen::default());
        let again = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
        assert!(
            again
                .lines()
                .next()
                .unwrap_or("")
                .contains(" copies 0 of 2 reached "),
            "a second round granted a grace to a node that was never safe: {}",
            again.lines().next().unwrap_or("")
        );
        let version = header
            .split(" version ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next());
        assert_eq!(version, Some(env!("CARGO_PKG_VERSION")));
    }

    /// `isolated` (ARCHITECTURE §7) reads when a round last reached another
    /// machine off the header. A daemon restarted on a laptop offline for a
    /// day has reached nobody yet: the day must stay, not turn into a fresh
    /// start, nor vanish. Sabotage: stamp now every round, or take only this
    /// process's own contact.
    #[test]
    fn red_team_the_last_contact_outlives_a_round_and_a_restart_that_reached_nobody() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let bound: std::net::SocketAddr = "127.0.0.1:9797".parse().unwrap();
        let interval = Duration::from_secs(300);
        let header = || {
            let text = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
            text.lines().next().unwrap_or("").to_owned()
        };
        let day_ago = itsanas_discover::now_unix() - 24 * 3600;
        std::fs::write(
            home.join(crate::node::SNAPSHOT),
            format!("snapshot {day_ago} every 300 version 0.1.13 reached {day_ago}\n"),
        )
        .expect("snapshot");

        write_snapshot(&node, interval, bound, &Seen::default());
        assert_eq!(
            crate::safety::last_reached(&header()),
            Some(day_ago),
            "a restart that reached nobody forgot, or reset, a day without contact: the \
             laptop reads green again ({})",
            header()
        );
        let now = itsanas_discover::now_unix();
        let seen = Seen {
            reached: Some(now),
            unwritable: None,
        };
        write_snapshot(&node, interval, bound, &seen);
        assert_eq!(crate::safety::last_reached(&header()), Some(now));
        write_snapshot(&node, interval, bound, &Seen::default());
        assert_eq!(
            crate::safety::last_reached(&header()),
            Some(now),
            "a round that reached nobody moved the last contact"
        );
    }

    /// The probe's failure goes through the daemon's own writer to both
    /// readers: the icon's header word and `status`'s reason. Sabotage:
    /// leave ` unwritable` off the header.
    #[test]
    fn red_team_a_vault_the_probe_could_not_write_reaches_the_icon_and_status() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let bound: std::net::SocketAddr = "127.0.0.1:9797".parse().unwrap();
        let seen = Seen {
            reached: None,
            unwritable: Some("Read-only file system (os error 30)".to_owned()),
        };
        write_snapshot(&node, Duration::from_secs(300), bound, &seen);
        let text = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
        let brief = crate::brief_status(&home, true, itsanas_discover::now_unix());
        assert!(
            brief.starts_with("unwritable "),
            "the icon of a node whose vault refused the probe reads {brief:?}: it goes on \
             promising room it cannot give"
        );
        assert!(
            text.contains("cannot be written (Read-only file system (os error 30))"),
            "the snapshot `status` prints does not say why the vault takes nothing:\n{text}"
        );
    }

    /// ARCHITECTURE §7, "a proof-of-storage challenge fails". Through
    /// `write_snapshot`: the icon reads `failing`, `status` names the machine
    /// (it used to go to stdout, so a running node's `status` could not), and
    /// an answered round clears it, or the icon stays orange for good.
    /// Sabotage: write no count, count every machine that ever failed, or
    /// print the list instead of keeping it.
    #[test]
    fn red_team_a_failed_challenge_reaches_the_icon_and_status_and_clears() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let bound: std::net::SocketAddr = "127.0.0.1:9797".parse().unwrap();
        let seen = Seen {
            reached: None,
            unwritable: None,
        };
        let liar = DeviceId::from_bytes([7; 32]);
        node.store.note_audit(&liar, false).expect("audit");
        write_snapshot(&node, Duration::from_secs(300), bound, &seen);
        let text = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
        let brief = crate::brief_status(&home, true, itsanas_discover::now_unix());
        assert!(
            brief.starts_with("failing ") && brief.ends_with(" 1"),
            "a machine that lied about holding this account's data was drawn {brief:?}: its \
             copies stopped counting and nobody was told"
        );
        assert!(
            text.contains("peers that have failed a storage challenge")
                && text.contains(&liar.short()),
            "`status` on a running node does not say which machine failed:\n{text}"
        );

        node.store.note_audit(&liar, true).expect("audit");
        write_snapshot(&node, Duration::from_secs(300), bound, &seen);
        let brief = crate::brief_status(&home, true, itsanas_discover::now_unix());
        assert!(
            brief.starts_with("healthy "),
            "a machine that answered again still reads {brief:?}: an orange icon nothing can \
             turn back is a colour nobody reads"
        );
    }

    /// A machine that failed and then left never answers again. Sabotage: drop
    /// the age from `machines_failing`.
    #[test]
    fn red_team_a_machine_that_failed_and_left_does_not_keep_the_icon_orange() {
        let base = tempfile::tempdir().expect("temp dir");
        let (node, _) =
            itsanas_node::Node::create(&base.path().join(".itsanas"), "a passphrase", "camille")
                .expect("create");
        let liar = DeviceId::from_bytes([7; 32]);
        node.store.note_audit(&liar, false).expect("audit");
        let now = itsanas_discover::now_unix();
        assert_eq!(crate::machines_failing(&node, now).expect("count"), 1);
        assert_eq!(
            crate::machines_failing(&node, now + itsanas_store::holders::LIVE_FOR + 60)
                .expect("count"),
            0,
            "a machine gone for good keeps the icon orange for ever: nothing the person can \
             do turns it back, and a colour that never changes is one nobody reads"
        );
    }

    /// What each word reads as, and which one wins. `isolated` over `failing`:
    /// a node cut off cannot audit, so the record would hide the silence for as
    /// long as the silence lasts. `lowdisk` over `short`: such a disk is usually
    /// both, and only this one says the node's own writes are next. Sabotage:
    /// remove either arm, or swap the order.
    #[test]
    fn red_team_a_failing_machine_and_a_full_disk_are_never_drawn_green() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let now = 1_000_000;
        let brief = |words: &str| {
            std::fs::write(
                home.join(crate::node::SNAPSHOT),
                format!("snapshot {} every 300 version 0.1.15{words}\n", now - 60),
            )
            .expect("snapshot");
            crate::brief_status(&home, true, now)
        };
        assert_eq!(
            brief(" copies 2 of 2 failing 3"),
            "failing 60 3",
            "machines holding nothing of what they claimed were drawn green"
        );
        assert_eq!(
            brief(" copies 2 of 2 lowdisk 1000"),
            "lowdisk 60 1000",
            "a disk about to fail every write was drawn green"
        );
        assert_eq!(
            brief(" copies 2 of 2 failing 1 lowdisk 1000"),
            "failing 60 1",
            "the lower alert hid the one about the person's own copies"
        );
        assert_eq!(
            brief(&format!(
                " copies 2 of 2 reached {} failing 1",
                now - 25_200
            )),
            "isolated 60 25200",
            "a failed challenge hid a node cut off from every machine"
        );
    }

    /// ARCHITECTURE §7, "online node count below the replication floor": a
    /// write made while a machine is off reaches fewer machines, so the
    /// account's complete copies drop and the icon says `exposed` past its
    /// grace. This is why that row has no word of its own. Sabotage: count a
    /// copy that holds only some of the chunks as complete.
    #[test]
    fn red_team_a_write_made_while_a_machine_is_off_reads_exposed() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let now = itsanas_discover::now_unix();
        let save = |name: &str, body: &[u8]| {
            let source = base.path().join(name);
            std::fs::write(&source, body).expect("source");
            node.store
                .write_stream(name, std::fs::File::open(&source).expect("open"))
                .expect("write");
            node.store.flush_segment().expect("flush");
            node.store.stat(name).expect("stat").expect("saved").chunks
        };
        let on = DeviceId::from_bytes([1; 32]);
        let off = DeviceId::from_bytes([2; 32]);
        for machine in [&on, &off] {
            node.store.note_audit(machine, true).expect("audit");
            node.store.note_seen_at(machine, now).expect("seen");
        }
        let both = save("both.txt", b"on both machines");
        node.store.record_holders(&both, &on).expect("record");
        node.store.record_holders(&both, &off).expect("record");
        let before = node.store.coverage(now).expect("coverage");
        assert_eq!(before.complete_elsewhere, 2, "the setup is not two copies");

        let one_machine = save("second.txt", b"saved with one machine off");
        node.store
            .record_holders(&one_machine, &on)
            .expect("record");
        let after = node.store.coverage(now).expect("coverage");
        assert_eq!(
            after.complete_elsewhere, 1,
            "a file on one machine counted as two complete copies: nothing warns that new \
             writes miss their target while a machine is off"
        );

        let two_copies = crate::safety::Copies {
            have: before.complete_elsewhere,
            want: crate::SAFE_COPIES,
            since: None,
        };
        let short = crate::safety::Copies::this_round(
            after.complete_elsewhere,
            crate::SAFE_COPIES,
            true,
            Some(two_copies),
            now,
        )
        .expect("something is stored");
        assert!(
            !short.exposed(now, 300) && short.exposed(now + 600, 300),
            "the shortfall does not read `exposed` past its grace: {short:?}"
        );
    }

    /// The silence the icon shows, in the command a person runs next
    /// (ROADMAP M7). Sabotage: drop the line, or print it on any contact.
    #[test]
    fn red_team_status_says_in_words_that_no_round_reached_anybody() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let bound: std::net::SocketAddr = "127.0.0.1:9797".parse().unwrap();
        let now = itsanas_discover::now_unix();
        let read = |reached: u64| {
            let seen = Seen {
                reached: Some(reached),
                unwritable: None,
            };
            let _ = std::fs::remove_file(home.join(crate::node::SNAPSHOT));
            write_snapshot(&node, Duration::from_secs(300), bound, &seen);
            std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot")
        };
        let silent = read(now - 7 * 3600);
        assert!(
            silent.contains("NOT SYNCED") && silent.contains("for 7 h"),
            "the icon is orange and `status` does not say why:\n{silent}"
        );
        let fresh = read(now);
        assert!(
            !fresh.contains("NOT SYNCED"),
            "a node that reached a machine just now says it did not:\n{fresh}"
        );
    }

    /// `probe_vault`, which `sync_loop` calls once a round: it sets
    /// `unwritable` while the vault refuses, leaves alone a node that
    /// promised nobody (pledging nothing, or away from its vault's disk),
    /// and clears the word once the disk takes writes again -- or the icon
    /// stays orange for good. A plain file stands for the refusing vault, so
    /// this runs on Windows too. Sabotage: never set it, never clear it, or
    /// probe a node that pledged nothing.
    #[test]
    fn red_team_the_round_probe_says_unwritable_while_the_vault_refuses_and_stops_after() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (mut node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        node.config.pledge_bytes = 100 << 20;
        let mut unwritable = None;
        probe_vault(&node, &mut unwritable);
        assert_eq!(
            unwritable, None,
            "a vault that takes writes failed the round probe: every host reads orange"
        );

        let refusing = base.path().join("not-a-directory");
        std::fs::write(&refusing, b"a file where the vault should be").expect("file");
        let real = node.config.vault.replace(refusing);
        probe_vault(&node, &mut unwritable);
        assert!(
            unwritable.is_some(),
            "a vault that takes no write passed the round probe: its host goes on promising \
             room every push to it fails on"
        );

        for (pledge, away, who) in [
            (0, false, "pledging nothing"),
            (100 << 20, true, "away from its vault's disk"),
        ] {
            node.config.pledge_bytes = pledge;
            node.away = away;
            let mut quiet = None;
            probe_vault(&node, &mut quiet);
            assert_eq!(
                quiet, None,
                "a node {who} was told its vault takes no write: it promised nobody"
            );
        }
        node.config.pledge_bytes = 100 << 20;
        node.away = false;

        node.config.vault = real;
        probe_vault(&node, &mut unwritable);
        assert_eq!(
            unwritable, None,
            "a vault that takes writes again still reads unwritable: the icon stays orange \
             for good"
        );
    }
}
