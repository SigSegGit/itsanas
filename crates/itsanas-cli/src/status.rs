//! `status`: the full report, the daemon's snapshot, and the brief line.

use std::{fmt::Write as _, path::Path};

use crate::SAFE_COPIES;
use crate::coverage::{coverage_report, report_unreliable_peers};
use crate::device_command::{describe_announce, describe_folder};
use crate::leaving::departure_refusal;
use crate::opening::{PASSPHRASE_ENV, open, passphrase_available};
use crate::{
    config::format_size,
    error::{CliError, Result},
    node::{Node, SNAPSHOT},
};
use crate::{control, daemon, placing, safety, update};

/// What this node is costing the disk, in the three parts a person can act on.
///
/// # Why this is measured and not implied
///
/// `keep` bounds one of these numbers and reads as though it bounds the disk.
/// On the trial device it did not, and by a factor of twenty: told to keep 200
/// KiB, the node's directory held 4.3 MiB — 907 KiB of content, and the rest
/// index and vault. Somebody deciding whether this fits on a phone needs the
/// total, and the breakdown to know which setting moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiskUse {
    /// This account's own content. The number `keep` bounds.
    mine: u64,
    /// Sealed data held for other people, and this account's own log relayed
    /// between its devices. The number `pledge` bounds.
    vault: u64,
    /// The databases: the index and the vault's own store.
    ///
    /// Bounded by nothing, and proportional to the number of files and log
    /// entries rather than to their size. On a nearly empty account it is most
    /// of the total.
    indexes: u64,
}

impl DiskUse {
    const fn total(self) -> u64 {
        self.mine
            .saturating_add(self.vault)
            .saturating_add(self.indexes)
    }
}

/// Measure it.
///
/// Only the files sitting directly in the store and vault directories are
/// stat'ed — the databases. Content is already counted, and walking a blob
/// store of a million files to add up what the index already knows would make
/// `status` cost a minute on the machines that most need it.
pub(crate) fn disk_use(node: &Node) -> Result<DiskUse> {
    fn databases(root: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(root) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len())
            .sum()
    }

    Ok(DiskUse {
        mine: node.store.stats()?.bytes_on_disk,
        vault: node.vault.stats()?.bytes,
        indexes: databases(node.store.root()) + databases(&node.vault_dir()),
    })
}

/// `status`'s "listen" line: where the daemon writing the snapshot really
/// listens, when it is not the configured address.
///
/// `daemon --listen` overrides the configuration for one run, and a snapshot
/// that printed the configured port sent a reader to one nobody was on (hand
/// test, 2026-10-22). `register` reads this line back (`daemon_listening`).
pub(crate) fn listen_line(node: &Node, listening: Option<&str>) -> String {
    match listening.filter(|bound| *bound != node.config.listen) {
        Some(bound) => format!(
            "  listen          {bound} (where this run is bound; configured {})",
            node.config.listen
        ),
        None => format!("  listen          {}", node.config.listen),
    }
}

/// `status`'s "used" line, and the warning when the disk can no longer
/// hold the pledge, or the vault no longer takes a write: `unwritable` is
/// why the last probe of it failed (`pledges::probe_write`).
/// Free space on the vault's disk as the pledge sees it: the reserve file
/// (`reserve = on`) holds this node's own pledge, so it counts as free to it.
pub(crate) fn free_for_pledge(node: &Node) -> Option<u64> {
    fs4::available_space(node.vault_dir())
        .ok()
        .map(|free| free.saturating_add(itsanas_node::pledges::reserved(&node.vault_dir())))
}

pub(crate) fn used_line(node: &Node, hosted: u64, unwritable: Option<&str>) -> String {
    let mut used = format!("  used            {}", format_size(hosted));
    // The reserve file holds the pledge: free to it, never a reason to say
    // the disk cannot keep it.
    let reserved = itsanas_node::pledges::reserved(&node.vault_dir());
    if node.config.reserve {
        let _ = write!(
            used,
            "
  held on disk    {} (reserve = on)",
            format_size(reserved)
        );
    }
    let free = free_for_pledge(node);
    let keepable = itsanas_node::pledges::keepable(node.config.pledge_bytes, hosted, free);
    if keepable < node.config.pledge_bytes {
        let _ = write!(
            used,
            "\n  CANNOT KEEP    this disk can hold only {} of the {} pledged; it takes no more \
             chunks until space is freed or `itsanas pledge` is lowered",
            format_size(keepable),
            format_size(node.config.pledge_bytes)
        );
    }
    if let Some(why) = unwritable {
        let _ = write!(
            used,
            "\n  CANNOT KEEP    the vault at {} cannot be written ({why}); every chunk sent to \
             it fails until the disk takes writes again",
            node.vault_dir().display()
        );
    }
    used
}

/// The text `itsanas status` prints, built rather than printed.
///
/// Separated from the command so the daemon can write the same text to a
/// snapshot after every round. The store allows one writer, so with the daemon
/// up `status` cannot open it -- and answering "the node is busy" to somebody
/// asking what their node is doing is the least useful thing this program could
/// say.
///
/// `listening` is where the daemon writing the snapshot really listens, and
/// `None` from `status` itself, which runs no listener.
pub(crate) fn render_status(node: &Node, listening: Option<&str>) -> Result<String> {
    render_status_and_copies(node, listening, None).map(|(text, _)| text)
}

/// [`render_status`], and the copies its coverage counted, for the snapshot's
/// first line (`safety.rs`): counted once a round, not twice.
pub(crate) fn render_status_and_copies(
    node: &Node,
    listening: Option<&str>,
    unwritable: Option<&str>,
) -> Result<(String, Option<safety::Copies>)> {
    let mut out = String::new();

    // A local macro, so the body below reads the way it did when it printed.
    // Defined after `out` because a macro_rules body resolves names where it is
    // written, and expanding to a block rather than a `let` statement because
    // half these calls sit in match arms, where a statement is not an
    // expression. Writing to a String cannot fail, which is why the result is
    // dropped rather than propagated.
    macro_rules! w {
        () => {{ let _ = writeln!(out); }};
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }
    let store = node.store.stats()?;
    let vault = node.vault.stats()?;

    w!("account");
    w!("  username        {}", node.config.username);
    w!("  user id         {}", node.store.owner());
    w!("  device          {}", node.store.device_id());
    w!("  home            {}", node.home.display());
    match &node.config.folder {
        Some(folder) => w!("{}", describe_folder(folder)),
        None => w!("  synced folder   none (`itsanas folder <path>`)"),
    }
    w!();
    w!("your data");
    w!("  files           {}", store.files);
    w!("  live chunks     {}", store.live_chunks);
    w!("  on disk         {}", format_size(store.bytes_on_disk));
    match node.config.keep_bytes {
        None => w!("  keeping         all of it here"),
        Some(keep) if store.bytes_on_disk >= keep => w!(
            concat!(
                "  keeping         at most {} here, which is reached; new content ",
                "stays listed and is fetched when opened"
            ),
            format_size(keep)
        ),
        Some(keep) => w!(
            "  keeping         at most {} here ({} to go)",
            format_size(keep),
            format_size(keep.saturating_sub(store.bytes_on_disk))
        ),
    }
    w!("  log segments    {}", store.segments);
    if store.unsealed_entries > 0 {
        w!(
            "  unannounced     {} (run `itsanas sync` to publish)",
            store.unsealed_entries
        );
    }
    if store.pending_collection > 0 {
        w!("  awaiting gc     {} chunks", store.pending_collection);
    }

    // The question a backup tool exists to answer, and the one it is easiest
    // to leave unanswered: does this data exist anywhere other than this disk?
    // A count of files says nothing about that.
    w!();
    let copies = coverage_report(node, &mut out)?;
    w!("  placements     {} recorded", store.holder_records);

    report_unreliable_peers(node, &mut out)?;

    // In every mode: the figures are read before anyone acts on them.
    w!();
    out.push_str(&placing::report(node.config.placement, &node.store)?);
    // The vault holds two different things. Reporting them as one number
    // tells the operator they are hosting for a stranger when they are only
    // relaying their own account between their own machines.
    let own_in_vault = node.vault.stats_for(node.store.owner())?;
    let hosted_owners = vault
        .owners
        .saturating_sub(usize::from(own_in_vault.segments > 0));
    let hosted_bytes = vault.bytes.saturating_sub(own_in_vault.bytes);
    let hosted_chunks = vault.chunks.saturating_sub(own_in_vault.chunks);

    w!();
    let disk = disk_use(node)?;
    w!("disk used by this node");
    w!("  your content    {}", format_size(disk.mine));
    w!("  vault           {}", format_size(disk.vault));
    w!("  indexes         {}", format_size(disk.indexes));
    w!("  total           {}", format_size(disk.total()));
    if node.config.keep_bytes.is_some() {
        w!("  `keep` bounds the first line only; the rest is index and vault.");
    }

    w!();
    w!("hosting for other people");
    w!(
        "  pledged         {}",
        format_size(node.config.pledge_bytes)
    );
    w!("{}", used_line(node, hosted_bytes, unwritable));
    w!("  peers hosted    {hosted_owners}");
    w!("  chunks held     {hosted_chunks}");
    w!(
        "  segments held   {}",
        vault.segments.saturating_sub(own_in_vault.segments)
    );
    w!();
    w!("relaying for your own devices");
    w!(
        "  segments held   {} (so this machine can pass your other devices' \
         work along)",
        own_in_vault.segments
    );
    w!();
    w!("network");
    w!("{}", listen_line(node, listening));
    // What other machines are told, which is the number that decides whether
    // anybody outside this LAN can reach this one -- and which differs from
    // `listen` exactly when somebody has set it up to.
    w!("  announced       {}", describe_announce(node));
    if node.config.peers.is_empty() {
        w!("  peers           none configured (`itsanas peer add <host:port>`)");
    } else {
        for peer in &node.config.peers {
            w!("  peer            {peer}");
        }
    }

    Ok((out, copies))
}

/// How long ago, in words, for a reader who wants to know whether to trust it.
///
/// Rounded down and deliberately coarse: the question this answers is "is this
/// current enough to act on", and a snapshot four minutes old and one four
/// minutes and fifty seconds old have the same answer.
pub(crate) fn describe_age(seconds: u64) -> String {
    match seconds {
        0..=5 => "just now".to_owned(),
        6..=89 => format!("{seconds} seconds ago"),
        90..=5399 => format!("{} minutes ago", seconds / 60),
        5400..=86_399 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// What the daemon last reported, whether or not it is still running.
///
/// Takes no passphrase, and that is the point: the snapshot is a plain file in
/// the node home, so printing it needs no key. Returning the text rather than
/// printing it is what makes the age line testable. `running` decides the
/// first line only — a snapshot from a stopped node must not read as a live
/// one.
pub(crate) fn snapshot_status(home: &Path, running: bool) -> Result<String> {
    let text = std::fs::read_to_string(home.join(SNAPSHOT)).map_err(|_| {
        CliError::Usage(
            "this node is running and has not written a snapshot yet. Wait for its first sync round, or stop it and ask again."
                .to_owned(),
        )
    })?;

    let (stamp, body) = text.split_once('\n').unwrap_or(("", text.as_str()));
    let taken = stamp
        .strip_prefix("snapshot ")
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok());
    // The two cases are not the same claim and must not read as one. A daemon
    // holding the store means the snapshot is a recent report of a live node;
    // no daemon means it is the last thing a stopped node said, which may be
    // from last week.
    let subject = if running {
        "this node is running"
    } else {
        "nothing is running this node"
    };
    let header = match taken {
        Some(taken) => format!(
            "{subject}, so what follows is what it reported {}.",
            describe_age(itsanas_discover::now_unix().saturating_sub(taken))
        ),
        // A snapshot whose first line is not a stamp is one this version did
        // not write. Print it rather than refuse, and do not put an age on it
        // that was never measured.
        None => format!("{subject}; the snapshot it left has no time on it."),
    };
    let now = itsanas_discover::now_unix();
    let header = match control::Control::read(home) {
        Ok(asked) if asked.pause_holds(now) && asked.until.is_some() => format!(
            "{header}\nSyncing is PAUSED {}: nothing moves until then, or `itsanas resume`.",
            asked.lasts(now)
        ),
        Ok(control::Control {
            paused_since: Some(since),
            until: None,
            ..
        }) => format!(
            "{header}\nSyncing is PAUSED here, since {}: nothing moves until `itsanas resume`.",
            describe_age(now.saturating_sub(since))
        ),
        Ok(_) => header,
        Err(why) => format!(
            "{header}\nThe control file cannot be read ({why}): the daemon keeps what it last \
             understood, and one starting now starts paused."
        ),
    };

    Ok(format!("{header}\n\n{body}"))
}

/// The node's state in one word, for a tray icon: what it shows, not what it
/// hopes. `short` when the disk can no longer hold the pledge, `unwritable`
/// when the vault refuses a write, `isolated` when no round has reached
/// another machine for hours (all orange). `healthy` only while a daemon holds the store **and** its last
/// snapshot is within two intervals; a daemon alive but silent longer than
/// that is `stale` -- a green icon over a hung daemon is the failure a tray
/// exists to prevent (HANDOVER §8 f). The age follows in seconds.
pub(crate) fn print_brief(home: &Path) {
    let running = Node::exists(home) && itsanas_store::Store::is_locked(Node::store_path(home));
    println!(
        "{}",
        brief_status(home, running, itsanas_discover::now_unix())
    );
}

pub(crate) fn brief_status(home: &Path, running: bool, now: u64) -> String {
    if departure_refusal(home).is_some() {
        return "departed".to_owned();
    }
    if !running {
        return "stopped".to_owned();
    }
    let Ok(text) = std::fs::read_to_string(home.join(SNAPSHOT)) else {
        return "unknown".to_owned();
    };
    let mut words = text.lines().next().unwrap_or("").split_whitespace();
    let (Some("snapshot"), Some(taken)) = (
        words.next(),
        words.next().and_then(|w| w.parse::<u64>().ok()),
    ) else {
        return "unknown".to_owned();
    };
    let every = match (
        words.next(),
        words.next().and_then(|w| w.parse::<u64>().ok()),
    ) {
        (Some("every"), Some(every)) => every.max(1),
        _ => daemon::DEFAULT_INTERVAL.as_secs(),
    };
    let age = now.saturating_sub(taken);
    if Node::store_path(home)
        .join(itsanas_store::store::TWIN)
        .exists()
    {
        // Another disk holds this identity: nothing written here reaches
        // anybody, whatever else is true. Red, before every other word.
        format!("twin {age}")
    } else if age > every.saturating_mul(2) {
        format!("stale {age}")
    } else {
        // A control file nobody can read is not "healthy": the daemon is
        // keeping what it last understood, or started paused (`control/schedule.rs`),
        // and the icon should say that something needs a look.
        let header = text.lines().next().unwrap_or("");
        let copies = safety::Copies::parse(header);
        let isolated = safety::isolated_for(header, now, every);
        match control::Control::read(home) {
            Err(_) => format!("unknown {age}"),
            Ok(asked) if asked.pause_holds(now) => format!("paused {age}"),
            // Running, and this account's files are not where they are meant
            // to be yet (MVP.md test L). Before `short`: that one is about the
            // room promised to others, this one about the person's own files.
            // Back from a long silence (a key out of its drawer): copies are
            // being proved again, not lost. Blue, for at most ISOLATED_AFTER;
            // the seconds since it came back follow.
            Ok(_)
                if copies.is_some_and(|c| c.exposed(now, every))
                    && safety::catching_up(header, now).is_some() =>
            {
                format!(
                    "catching-up {age} {}",
                    safety::catching_up(header, now).unwrap_or_default()
                )
            }
            Ok(_) if copies.is_some_and(|c| c.exposed(now, every)) => {
                let copies = copies.unwrap_or(safety::Copies {
                    have: 0,
                    want: SAFE_COPIES,
                    since: None,
                });
                format!("exposed {age} {} {}", copies.have, copies.want)
            }
            // Running every round and reaching nobody: a fresh snapshot over
            // copies a week can still count (`safety.rs`, ARCHITECTURE §7).
            // The silence follows, in seconds.
            Ok(_) if isolated.is_some() => {
                format!("isolated {age} {}", isolated.unwrap_or_default())
            }
            // A machine failed its last storage challenge: what it holds no
            // longer counts (`status` says which). After `isolated`: a node
            // cut off cannot audit, so this word would otherwise hide the
            // silence for as long as the silence lasts. The count follows.
            Ok(_) if safety::failing_machines(header).is_some() => {
                format!(
                    "failing {age} {}",
                    safety::failing_machines(header).unwrap_or_default()
                )
            }
            // The vault refused the daemon's last write probe: room on the
            // disk, and every chunk sent to it fails (`status` says why).
            Ok(_) if header.split_whitespace().any(|word| word == "unwritable") => {
                format!("unwritable {age}")
            }
            // Below the reserve: the next writes fail, this node's own index
            // first. Before `short`, which such a disk usually is as well and
            // which only speaks of the room promised to others. The free bytes
            // follow.
            Ok(_) if safety::low_disk(header).is_some() => {
                format!(
                    "lowdisk {age} {}",
                    safety::low_disk(header).unwrap_or_default()
                )
            }
            // Running, but the disk can no longer hold what it promised: not
            // green (`status` says CANNOT KEEP and why).
            Ok(_) if itsanas_node::pledges::short_of_pledge(home) => format!("short {age}"),
            Ok(_) => format!("healthy {age}"),
        }
    }
}

pub(crate) fn status(home: &Path, brief: bool) -> Result<()> {
    if brief {
        print_brief(home);
        return Ok(());
    }
    // First, so it is seen; written by the daemon's daily look (`updates =
    // notify`, the default).
    if let Some(line) = update::notice(home) {
        println!("{line}");
    }
    // Ask whether the daemon holds the store *before* asking anybody for a
    // passphrase. `open` resolves the passphrase first, so this path -- whose
    // entire purpose is to answer while the daemon is running, which is the
    // normal state of a working machine -- was unreachable exactly when it
    // applied: the command demanded the keystore secret and would then have
    // printed a plaintext file. The prompt guarded nothing a `cat` of the
    // snapshot would not bypass, and it made "is my node healthy?" a question
    // you could not ask without unsealing your keys.
    if Node::exists(home) && itsanas_store::Store::is_locked(Node::store_path(home)) {
        print!("{}", snapshot_status(home, true)?);
        return Ok(());
    }

    // No daemon, and no way to ask for a passphrase: a live read is impossible,
    // so the snapshot is strictly better than an error about environment
    // variables. This is the state a machine is in for the whole window
    // between installing and starting the daemon -- which is exactly when
    // somebody asks whether the thing works.
    if Node::exists(home) && !passphrase_available() {
        if home.join(SNAPSHOT).exists() {
            print!("{}", snapshot_status(home, false)?);
            return Ok(());
        }
        // A node that exists and has never finished a sync round. Answering
        // with advice about environment variables tells somebody who just
        // installed this that they have done something wrong, when the real
        // answer is that nothing has run yet and they should start it.
        return Err(CliError::Usage(format!(
            "this node has never finished a sync round, so it has nothing to \
             report yet. Start it with `itsanas daemon`, or set {PASSPHRASE_ENV} \
             to open the node and read its state directly."
        )));
    }

    match open(home) {
        Ok(node) => {
            print!("{}", render_status(&node, None)?);
            Ok(())
        }
        // Still handled, because the daemon can take the lock between the probe
        // above and this open -- while the passphrase is being typed, which is
        // exactly when it is slowest. Rare, and the original bug if it were
        // dropped.
        Err(CliError::Store(itsanas_store::StoreError::Locked(_))) => {
            print!("{}", snapshot_status(home, true)?);
            Ok(())
        }
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::brief_status;
    use super::describe_age;
    use super::snapshot_status;
    use crate::leaving::record_departure;
    use crate::node::SNAPSHOT;
    use crate::test_fixtures::fake_node;

    /// A tray shows what this says, so a daemon that holds the store but has
    /// not written a snapshot for more than two intervals is `stale`, never
    /// `healthy`. Sabotage: drop the age check.
    #[test]
    fn red_team_a_silent_daemon_is_stale_never_healthy() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let now = 1_000_000;
        let write = |taken: u64| {
            std::fs::write(
                home.join(SNAPSHOT),
                format!("snapshot {taken} every 300\n  files 3\n"),
            )
            .expect("snapshot");
        };

        write(now - 60);
        assert_eq!(brief_status(&home, true, now), "healthy 60");
        // A pledge the disk cannot hold: orange, never green.
        std::fs::write(
            home.join(itsanas_node::pledges::STATE_FILE),
            format!("pledge={}\nheld=0\nupdated={now}\n", u64::MAX / 2),
        )
        .expect("pledge state");
        assert_eq!(
            brief_status(&home, true, now),
            "short 60",
            "a node that cannot keep its pledge showed green in the tray"
        );
        std::fs::remove_file(home.join(itsanas_node::pledges::STATE_FILE)).expect("remove");
        write(now - 3 * 300);
        assert_eq!(
            brief_status(&home, true, now),
            "stale 900",
            "three intervals without a round read as healthy: a green icon over a hung daemon"
        );
        assert_eq!(brief_status(&home, false, now), "stopped");
        record_departure(&home, now).expect("depart");
        assert_eq!(brief_status(&home, true, now), "departed");
    }

    /// MVP.md test L: the icon is green only when the account has its copies
    /// elsewhere. Files on this disk alone are `exposed`, with the counts the
    /// trays print; a shortfall that follows a safe state is left unsaid for
    /// its grace and said after; and it is said before `short`, which is
    /// about the room promised to others, not the person's own files.
    /// Sabotage: drop the `exposed` arm, or put it after `short`.
    #[test]
    fn red_team_files_on_this_machine_alone_are_never_drawn_green() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let now = 1_000_000;
        let write = |copies: &str| {
            std::fs::write(
                home.join(SNAPSHOT),
                format!(
                    "snapshot {} every 300 version 0.1.13{copies}\n  files 3\n",
                    now - 60
                ),
            )
            .expect("snapshot");
        };

        write(" copies 0 of 2");
        assert_eq!(
            brief_status(&home, true, now),
            "exposed 60 0 2",
            "a node whose files exist on its own disk only was drawn green: the backup that \
             was never running"
        );
        write(" copies 1 of 2");
        assert_eq!(brief_status(&home, true, now), "exposed 60 1 2");
        write(&format!(" copies 1 of 2 since {}", now - 300));
        assert_eq!(
            brief_status(&home, true, now),
            "healthy 60",
            "a file saved a round ago turned the icon orange: orange would mean every save"
        );
        write(&format!(" copies 1 of 2 since {}", now - 2 * 300));
        assert_eq!(
            brief_status(&home, true, now),
            "exposed 60 1 2",
            "past its grace"
        );
        write(" copies 2 of 2");
        assert_eq!(brief_status(&home, true, now), "healthy 60");
        write("");
        assert_eq!(
            brief_status(&home, true, now),
            "healthy 60",
            "nothing stored, no question"
        );

        write(" copies 0 of 2");
        std::fs::write(
            home.join(itsanas_node::pledges::STATE_FILE),
            format!("pledge={}\nheld=0\nupdated={now}\n", u64::MAX / 2),
        )
        .expect("pledge state");
        assert_eq!(
            brief_status(&home, true, now),
            "exposed 60 0 2",
            "a pledge the disk cannot hold hid that the person's own files are on this disk only"
        );
    }

    /// A machine whose store recorded a twin writes nothing that reaches
    /// anybody; its icon says so in red, whatever else the snapshot says, and
    /// even over a fresh, healthy-looking one. Sabotage: drop the `twin` arm.
    #[test]
    fn red_team_a_twin_is_drawn_red_over_a_healthy_snapshot() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let now = 2_000_000;
        std::fs::write(
            home.join(SNAPSHOT),
            format!(
                "snapshot {} every 300 version 0.1.22 copies 2 of 2 reached {}\n",
                now - 60,
                now - 60
            ),
        )
        .expect("snapshot");
        assert_eq!(brief_status(&home, true, now), "healthy 60");
        let store = itsanas_node::node::Node::store_path(&home);
        std::fs::create_dir_all(&store).expect("store dir");
        std::fs::write(
            store.join(itsanas_store::store::TWIN),
            "segment ab12 seen 1",
        )
        .expect("twin");
        assert_eq!(
            brief_status(&home, true, now),
            "twin 60",
            "a machine whose writes reach nobody was drawn green"
        );
    }

    /// A key out of its drawer after a month reaches a machine again: its
    /// holder records aged out, it counts no copy, and it read `exposed`,
    /// red, "only on this machine", while the rounds proved the copies again.
    /// It reads `catching-up` (blue) from the contact that ended the silence,
    /// for at most `ISOLATED_AFTER`, and never for a laptop closed an evening;
    /// past the bound a copy still missing is said as it is (HANDOVER §8 0e
    /// step 5). Sabotage: drop the `catching-up` arm, or keep `back` for ever.
    #[test]
    fn red_team_a_key_back_after_a_month_reads_catching_up_not_only_on_this_machine() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let month_ago = 1_000_000;
        let back = month_ago + 30 * 24 * 3600;
        let header_after = |previous: &str, seen: Option<u64>, now: u64| {
            format!(
                " copies 0 of 2{}",
                crate::safety::reached_words(seen, previous, now)
            )
        };
        // The round that reaches a machine after the month.
        let first = header_after(&format!(" reached {month_ago}"), Some(back), back);
        assert!(
            first.contains(&format!(" back {back}")),
            "the return was not noted: {first}"
        );
        // An evening closed is no return.
        let evening = header_after(&format!(" reached {}", back - 3 * 3600), Some(back), back);
        assert!(
            !evening.contains(" back "),
            "a laptop closed an evening read as back: {evening}"
        );

        let write = |rest: &str, now: u64| {
            std::fs::write(
                home.join(SNAPSHOT),
                format!(
                    "snapshot {} every 300 version 0.1.22{rest}
  files 3
",
                    now - 60
                ),
            )
            .expect("snapshot");
        };
        let later = back + 3600;
        let carried = header_after(&first, Some(later), later);
        write(&carried, later);
        assert_eq!(
            brief_status(&home, true, later),
            "catching-up 60 3600",
            "a key back an hour ago, proving its copies again, was drawn red"
        );
        let past = back + crate::safety::ISOLATED_AFTER + 60;
        let expired = header_after(&carried, Some(past), past);
        assert!(
            !expired.contains(" back "),
            "catching up never ended: {expired}"
        );
        write(&expired, past);
        assert_eq!(
            brief_status(&home, true, past),
            "exposed 60 0 2",
            "a copy still missing hours after the return was hidden behind catching up"
        );
    }

    /// ARCHITECTURE §7, "no successful sync round in N minutes": a daemon
    /// that runs every round and reaches nobody writes a fresh snapshot each
    /// time, so `stale` never fires, and `exposed` waits a week for holders to
    /// age out of `LIVE_FOR`. The last contact the header carries says it; the
    /// snapshot's own age cannot. Sabotage: drop the `isolated` arm, or read
    /// the snapshot's age in place of the contact's.
    #[test]
    fn red_team_a_daemon_that_reaches_nobody_is_never_drawn_green() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let now = 1_000_000;
        let write = |every: u64, rest: &str| {
            std::fs::write(
                home.join(SNAPSHOT),
                format!(
                    "snapshot {} every {every} version 0.1.13{rest}\n  files 3\n",
                    now - 60
                ),
            )
            .expect("snapshot");
        };

        write(300, &format!(" copies 2 of 2 reached {}", now - 7 * 3600));
        assert_eq!(
            brief_status(&home, true, now),
            "isolated 60 25200",
            "a laptop that reached no other machine for seven hours was drawn green: it is \
             told only when its copies expire, a week later"
        );
        write(300, &format!(" copies 2 of 2 reached {}", now - 300));
        assert_eq!(brief_status(&home, true, now), "healthy 60");
        write(300, &format!(" copies 2 of 2 reached {}", now - 5 * 3600));
        assert_eq!(
            brief_status(&home, true, now),
            "healthy 60",
            "a laptop closed for a few hours turned orange: orange would mean every evening"
        );
        // An hourly round is twelve hours of silence before it says so: six
        // would be six rounds, and one slow coordinator.
        write(3600, &format!(" copies 2 of 2 reached {}", now - 7 * 3600));
        assert_eq!(brief_status(&home, true, now), "healthy 60");
        write(3600, &format!(" copies 2 of 2 reached {}", now - 12 * 3600));
        assert_eq!(brief_status(&home, true, now), "isolated 60 43200");

        // Files on this disk alone say so first: that is the loss.
        write(300, &format!(" copies 0 of 2 reached {}", now - 7 * 3600));
        assert_eq!(brief_status(&home, true, now), "exposed 60 0 2");
        // Never reached anybody: no contact to age, and nothing to say here.
        write(300, " copies 2 of 2");
        assert_eq!(brief_status(&home, true, now), "healthy 60");
    }

    /// ARCHITECTURE §7, "pledged space ... unwritable": free space is not the
    /// only way to break the promise. A vault remounted read-only after disk
    /// errors has room and takes nothing, and every push to it fails while the
    /// icon would read green. The daemon's probe puts ` unwritable` on the
    /// header and the reason in `status`. Sabotage: drop the `unwritable` arm,
    /// or the CANNOT KEEP line.
    #[test]
    fn red_team_a_vault_that_cannot_be_written_is_never_drawn_green() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let now = 1_000_000;
        std::fs::write(
            home.join(SNAPSHOT),
            format!(
                "snapshot {} every 300 version 0.1.13 copies 2 of 2 unwritable\n",
                now - 60
            ),
        )
        .expect("snapshot");
        assert_eq!(
            brief_status(&home, true, now),
            "unwritable 60",
            "a node whose vault refuses every write was drawn green: it goes on promising \
             room it cannot give"
        );

        let (text, _) = super::render_status_and_copies(&node, None, Some("Read-only file system"))
            .expect("render");
        assert!(
            text.contains("  CANNOT KEEP    the vault at ")
                && text.contains(" cannot be written (Read-only file system)"),
            "`status` of a node whose vault cannot be written does not say so:\n{text}"
        );
        let (fine, _) = super::render_status_and_copies(&node, None, None).expect("render");
        assert!(!fine.contains("CANNOT KEEP"), "{fine}");
    }

    /// The snapshot is read and dated without a passphrase anywhere near it.
    ///
    /// The function takes a path and nothing else, which is the guarantee: it
    /// cannot prompt, so `status` cannot be made to ask for a key on the path
    /// whose purpose is to answer while the daemon holds the store.
    #[test]
    fn red_team_a_running_node_is_reported_with_its_age_and_no_passphrase() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path();
        let taken = itsanas_discover::now_unix().saturating_sub(4 * 3600);
        std::fs::write(
            home.join(SNAPSHOT),
            format!("snapshot {taken}\n  files          3\n"),
        )
        .expect("write snapshot");

        let out = snapshot_status(home, true).expect("a stamped snapshot is readable");
        assert!(
            out.contains("4 hours ago"),
            "the snapshot's age was not reported, so a reader cannot tell a \
             live answer from one left by a daemon that died last week: {out}"
        );
        assert!(
            out.contains("files          3"),
            "the snapshot body was dropped: {out}"
        );
    }

    /// A stopped node's report never reads as a live one.
    ///
    /// `status` prints the snapshot in two quite different situations: the
    /// daemon is holding the store, so the file is a recent report of a
    /// running node; or nothing is running and it is the last thing a stopped
    /// node said, possibly last week. Printing one sentence for both would
    /// make "this node is running" a claim the command cannot support -- and
    /// that sentence is the one a person uses to decide whether to trust the
    /// numbers under it.
    #[test]
    fn red_team_a_stopped_node_is_never_reported_as_a_running_one() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path();
        let taken = itsanas_discover::now_unix().saturating_sub(6 * 86_400);
        std::fs::write(
            home.join(SNAPSHOT),
            format!("snapshot {taken}\n  files          3\n"),
        )
        .expect("write snapshot");

        let stopped = snapshot_status(home, false).expect("readable");
        assert!(
            !stopped.contains("this node is running"),
            "a node nothing is running was reported as running: {stopped}"
        );
        assert!(
            stopped.contains("6 days ago"),
            "a six-day-old snapshot did not say so: {stopped}"
        );

        let running = snapshot_status(home, true).expect("readable");
        assert!(
            running.contains("this node is running"),
            "a held store should say the node is running: {running}"
        );
    }

    /// An undated snapshot is printed, and never given an age it never had.
    #[test]
    fn a_snapshot_without_a_stamp_is_printed_but_not_dated() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path();
        std::fs::write(home.join(SNAPSHOT), "  files          3\n").expect("write snapshot");

        let out = snapshot_status(home, false).expect("an unstamped snapshot is still readable");
        assert!(
            out.contains("no time on it"),
            "an undated snapshot was passed off as current: {out}"
        );
        assert!(
            !out.contains(" ago"),
            "an age was invented for a snapshot that carried no stamp: {out}"
        );
    }

    /// A node with no snapshot yet says so, rather than reporting nothing.
    #[test]
    fn a_node_that_has_never_synced_says_so_rather_than_printing_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(
            snapshot_status(dir.path(), true).is_err(),
            "a missing snapshot produced a successful, empty status, which \
             reads as a healthy node that has simply nothing to report"
        );
    }

    #[test]
    fn an_age_never_reads_as_fresher_than_it_is() {
        // The number this describes decides whether somebody trusts what they
        // are looking at, so every boundary rounds *down* -- towards admitting
        // the snapshot is older -- and nothing below a minute is allowed to
        // call itself "just now" except the few seconds where it is true.
        assert_eq!(describe_age(0), "just now");
        assert_eq!(describe_age(5), "just now");
        assert_eq!(describe_age(6), "6 seconds ago");
        assert_eq!(describe_age(89), "89 seconds ago");
        assert_eq!(describe_age(90), "1 minutes ago");
        assert_eq!(describe_age(3599), "59 minutes ago");
        assert_eq!(describe_age(5399), "89 minutes ago");
        assert_eq!(describe_age(5400), "1 hours ago");
        assert_eq!(describe_age(86_399), "23 hours ago");
        assert_eq!(describe_age(86_400), "1 days ago");

        // The case that matters most: a daemon that died three days ago must
        // not leave a snapshot that reads as if it were current.
        assert_eq!(describe_age(3 * 86_400 + 7), "3 days ago");
    }
}
