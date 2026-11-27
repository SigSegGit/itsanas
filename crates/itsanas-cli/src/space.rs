//! What this machine gives and keeps: `space`, `keep` and `pledge`.

use std::path::Path;

use crate::opening::open;
use crate::status::free_for_pledge;
use crate::{
    config::{format_size, parse_size},
    error::{CliError, Result},
    node::Node,
};

pub(crate) fn keep(
    home: &Path,
    size: Option<&str>,
    order: Option<&str>,
    only: &[String],
) -> Result<()> {
    let mut node = open(home)?;

    if size.is_none() && order.is_none() && only.is_empty() {
        report_keeping_settings(&node);
        return Ok(());
    }

    if let Some(order) = order {
        node.config.keep_order = crate::config::parse_order(order).ok_or_else(|| {
            CliError::Usage(format!(
                "unknown order {order:?}. Try newest, oldest or smallest."
            ))
        })?;
    }

    // Given at all, `--only` replaces the whole list rather than adding to it.
    // Appending would make the setting impossible to narrow without editing the
    // file by hand, and a filter that can only ever grow is one that quietly
    // stops filtering.
    if !only.is_empty() {
        node.config.keep_only = if only.iter().any(|prefix| prefix == "all") {
            Vec::new()
        } else {
            only.to_vec()
        };
    }

    if let Some(size) = size {
        if size.eq_ignore_ascii_case("all") || size.eq_ignore_ascii_case("none") {
            node.config.keep_bytes = None;
        } else {
            let bytes = parse_size(size)?;

            // The bargain, enforced where the number is typed.
            //
            // This network gives storage in proportion to storage provided, and
            // the ratio lives in `itsanas-coord`. Checking it only there would
            // mean somebody sets a limit, fills it over a fortnight, and is
            // then told it was never theirs to set -- with the data already on
            // the machine. Refused here, with the number that would make it
            // legal.
            Node::check_split(&node.config, node.config.pledge_bytes, Some(bytes))
                .map_err(|refusal| CliError::Usage(refusal.to_string()))?;

            node.config.keep_bytes = Some(bytes);
        }
    }

    node.save_config()?;
    report_keeping_settings(&node);

    // The consequence nobody would guess from the command they just typed.
    //
    // On a machine with a synced folder, letting go of content removes the file
    // from that folder: the store loses the entry, and the folder layer's next
    // pass sees content in its ledger and none in the store and deletes it from
    // disk. That is what a limit smaller than the account has to mean without a
    // placeholder filesystem, and it is what every selective-sync product did
    // before placeholders existed -- but it is a file disappearing from
    // somebody's Explorer window, and the fact that `itsanas get` brings it
    // back is knowledge only the author of this system has.
    //
    // `keep` and `folder` arm independently, so the destructive combination can
    // be reached without either command mentioning it. This is the mention.
    if node.config.folder.is_some() && node.config.keep_bytes.is_some() {
        println!();
        println!("  This machine syncs a folder, so a limit smaller than the account");
        println!("  will REMOVE files from it. They stay in the account and");
        println!("  `itsanas get <path>` brings one back, but they leave the folder.");
        println!("  `itsanas keep all` undoes the limit.");
    }

    // Said plainly rather than dressed up. A device over its limit comes back
    // down on the next sync, by letting go of what the order ranks lowest --
    // and only of content two other live machines are known to hold, at least
    // one of which has said so about that very chunk. Until then, the device
    // stays over its limit and says so.
    let held = node.store.stats()?.bytes_on_disk;
    if let Some(limit) = node.config.keep_bytes
        && held > limit
    {
        println!(
            "  {} is stored now, over that limit. The next sync lets go of what",
            format_size(held)
        );
        println!("  the order ranks lowest, keeping anything no other machine holds.");
    }

    Ok(())
}

/// Print what this device has been told to hold, in one place.
///
/// One function rather than a line at each call site, because the three
/// settings answer one question and a device that printed only the one just
/// changed would keep leaving out the one that explains the result.
pub(crate) fn report_keeping_settings(node: &Node) {
    match node.config.keep_bytes {
        Some(bytes) => println!(
            "keeping at most {} of your own data here",
            format_size(bytes)
        ),
        None => println!("keeping all of your own data here (`itsanas keep 2G` to limit it)"),
    }

    if node.config.keep_bytes.is_some() {
        println!(
            "  when it does not all fit: {} first",
            match node.config.keep_order {
                itsanas_policy::keeping::Order::Newest => "most recently changed",
                itsanas_policy::keeping::Order::Oldest => "least recently changed",
                itsanas_policy::keeping::Order::Smallest => "smallest",
            }
        );
    }

    if node.config.keep_only.is_empty() {
        return;
    }
    println!("  only these paths:");
    for prefix in &node.config.keep_only {
        println!("    {prefix}");
    }
}

/// What this machine can offer, what that earns, and which limit is binding.
///
/// # Why this is one command and not three questions in an installer
///
/// Two limits decide how much of your own data a device may hold, and they come
/// from different places: **the disk**, which is a fact about the machine, and
/// **what you have offered other people**, which is the bargain this network
/// runs on. A person choosing numbers needs both, together, before anything is
/// installed — otherwise the first they hear of the second is a coordinator
/// disagreeing with them a fortnight later.
///
/// Every installer asks this program rather than reimplementing the arithmetic
/// in shell, PowerShell and again in the Android settings screen. Three copies
/// of a rule is three answers to one question.
pub(crate) fn space(
    home: &Path,
    pledge: Option<&str>,
    keep: Option<&str>,
    apply: bool,
) -> Result<()> {
    space_on(&mut open(home)?, pledge, keep, apply)
}

/// [`space`] on a node already open, so a test reaches it without a
/// passphrase prompt.
pub(crate) fn space_on(
    node: &mut Node,
    pledge: Option<&str>,
    keep: Option<&str>,
    apply: bool,
) -> Result<()> {
    let split = node.config.split;

    let wanted_pledge = match pledge {
        Some(size) => parse_size(size)?,
        None => node.config.pledge_bytes,
    };
    let wanted_keep = match keep {
        Some(size) if size.eq_ignore_ascii_case("all") => None,
        Some(size) => Some(parse_size(size)?),
        None => node.config.keep_bytes,
    };

    // Free space where this node actually lives, not on some default drive: a
    // laptop with a small C: and a large D: is the ordinary case, and telling
    // somebody they have room they do not have is the one answer this must
    // never give.
    let free = free_for_pledge(node).unwrap_or(0);
    let held = node.store.stats()?.bytes_on_disk;
    let hosted = node.vault.stats()?.bytes;

    println!("this machine");
    println!("  node at         {}", node.home.display());
    if free == 0 {
        println!("  free space      unknown (the filesystem would not say)");
    } else {
        println!("  free space      {}", format_size(free));
    }
    println!("  your data here  {}", format_size(held));
    println!("  held for others {}", format_size(hosted));

    let earned = split.room_earned(wanted_pledge);
    println!();
    println!("the bargain");
    println!("  you offer       {}", format_size(wanted_pledge));
    println!(
        "  that earns you  {} (a {split} split: {} of your own for every {} you lend)",
        format_size(earned),
        split.own,
        split.network
    );
    println!(
        "  first {} days    at least {}, whatever you pledge",
        itsanas_coord::accounting::JOINING_PERIOD_SECONDS / 86_400,
        format_size(itsanas_coord::accounting::JOINING_ALLOWANCE)
    );

    // Everything this machine would be committing to, together. The pledge is
    // room for other people's data and `keep` is room for yours; a disk has to
    // hold both, and neither setting knows about the other.
    let mut refusals: Vec<String> = Vec::new();

    if let Some(committed) = itsanas_node::pledges::overcommitted(
        wanted_pledge,
        wanted_keep,
        Some(free).filter(|free| *free > 0),
        held,
        hosted,
    ) {
        refusals.push(format!(
            "offering {} and keeping {} needs {} on a disk with {} free",
            format_size(wanted_pledge),
            wanted_keep.map_or_else(|| "everything".to_owned(), format_size),
            format_size(committed),
            format_size(free)
        ));
    }

    if let Err(refusal) = Node::check_split(&node.config, wanted_pledge, wanted_keep) {
        refusals.push(format!(
            "keeping {} needs {} pledged; you are offering {}",
            format_size(refusal.keep),
            itsanas_node::config::size_argument(refusal.needed),
            format_size(refusal.pledge)
        ));
    }

    // And the other nodes on this disk: the installers pledge through here
    // (`provision.sh`), and until 2026-10-29 this compared the pledge with the
    // free space alone, which `pledge` had stopped doing on 2026-10-12.
    let held_for_others = node.held_for_others()?;
    if let Some(refusal) = disk_refusal(node, wanted_pledge, held_for_others) {
        refusals.push(refusal);
    }

    println!();
    if refusals.is_empty() {
        match wanted_keep {
            Some(keep) => println!(
                "keeping {} of your own here is within both limits",
                format_size(keep)
            ),
            None => println!("keeping all of your own data here is within both limits"),
        }
    } else {
        println!("that does not fit:");
        for refusal in &refusals {
            println!("  {refusal}");
        }
    }

    if !apply {
        if pledge.is_some() || keep.is_some() {
            println!();
            println!("Nothing was changed. Add `--apply` to set these.");
        }
        return Ok(());
    }

    if !refusals.is_empty() {
        return Err(CliError::Usage(
            "refusing to set numbers this machine cannot honour".to_owned(),
        ));
    }

    node.config.pledge_bytes = wanted_pledge;
    node.config.keep_bytes = wanted_keep;
    node.save_config()?;
    publish_pledge(node, wanted_pledge, held_for_others);
    println!();
    println!("set.");
    Ok(())
}

pub(crate) fn pledge(home: &Path, size: &str) -> Result<()> {
    let bytes = parse_size(size)?;
    let mut node = open(home)?;
    if let Some(warning) = set_pledge(&mut node, bytes)? {
        println!("warning: {warning}");
    }
    println!("pledged {} to the network", format_size(bytes));
    Ok(())
}

/// Set this node's pledge to `bytes`, or refuse and leave the file as it was.
/// The warning, when there is one, is for the caller to say.
pub(crate) fn set_pledge(node: &mut Node, bytes: u64) -> Result<Option<String>> {
    set_pledge_counting(node, bytes, 0)
}

/// [`set_pledge`], judging what this machine may keep against `bytes` plus
/// `others`, the pledges of the account's other machines: a phone earns on the
/// whole account, a computer on its own pledge (`others` is 0).
pub(crate) fn set_pledge_counting(
    node: &mut Node,
    bytes: u64,
    others: u64,
) -> Result<Option<String>> {
    // The split first: lowering the pledge under what `keep` holds would leave
    // this machine keeping more than it earns, which `keep` itself refuses.
    Node::check_split(
        &node.config,
        bytes.saturating_add(others),
        node.config.keep_bytes,
    )
    .map_err(|refusal| CliError::Usage(refusal.to_string()))?;

    // Only other accounts' bytes: our own devices push here too, and counting
    // those as hosted said the pledge was already paid by our own backlog.
    let held = node.held_for_others()?;
    // Lowering below what is already stored is allowed — the operator may
    // be reclaiming a disk — but it must be said out loud, because the node
    // will keep serving what it already took rather than silently dropping
    // a peer's data.
    let warning = (bytes < held).then(|| {
        format!(
            "{} is already held for other people, which is more than \
             the new pledge of {}. Nothing will be deleted, and what is already \
             stored will still be served; this node simply will not accept more.",
            format_size(held),
            format_size(bytes)
        )
    });

    if let Some(refusal) = disk_refusal(node, bytes, held) {
        return Err(CliError::Usage(refusal));
    }

    node.config.pledge_bytes = bytes;
    node.save_config()?;
    publish_pledge(node, bytes, held);
    Ok(warning)
}

/// [`Node::disk_refusal`] in the command line's words.
pub(crate) fn disk_refusal(node: &Node, bytes: u64, held: u64) -> Option<String> {
    node.disk_refusal(bytes, held).map(|refused| {
        format!(
            "offering {} on a disk with {} free, already holding {} for others; \
             at most {} can be offered here{}",
            format_size(refused.bytes),
            format_size(refused.free),
            format_size(refused.held),
            format_size(refused.room),
            if refused.others.is_empty() && refused.stale.is_empty() {
                String::new()
            } else {
                format!(
                    ", because other nodes on this disk promised space too:{}",
                    itsanas_node::pledges::describe(&refused.others, &refused.stale, format_size)
                )
            }
        )
    })
}

/// Tell the other nodes on this disk about a pledge just set, now and not at
/// the next round: a second setup a minute later must already see it.
pub(crate) fn publish_pledge(node: &Node, bytes: u64, held: u64) {
    let _ = itsanas_node::pledges::publish(
        &node.home,
        itsanas_node::pledges::shared_dir().as_deref(),
        &node.store.device_id().to_hex(),
        bytes,
        held,
        itsanas_discover::now_unix(),
    );
}

#[cfg(test)]
mod tests {
    use super::set_pledge;
    use super::space_on;
    use crate::test_fixtures::fake_node;

    /// `itsanas pledge` lowered under what `keep` needs is refused and the
    /// node file is left as it was. Sabotage: drop the `check_split` call in
    /// `set_pledge` -- the 1 MiB pledge is then saved beside a 20 GiB keep.
    #[test]
    fn red_team_pledge_under_what_keep_needs_is_refused_and_saves_nothing() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (mut node, _phrase) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        node.config.pledge_bytes = 50 * GIB;
        node.config.keep_bytes = Some(20 * GIB);
        node.save_config().expect("save");
        let path = itsanas_node::Node::config_path(&home);
        let before = std::fs::read(&path).expect("config");

        let refused = set_pledge(&mut node, 1024 * 1024);
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.to_string().contains("--apply")),
            "a 1 MiB pledge was accepted beside a 20 GiB keep: {refused:?}"
        );
        assert_eq!(
            std::fs::read(&path).expect("config"),
            before,
            "a refused pledge still rewrote the node file"
        );
        // Nor the node in memory: a caller that saves later would write it.
        assert_eq!(node.config.pledge_bytes, 50 * GIB, "refused, yet set");

        // Not a setter that refuses everything: with no limit to earn, the
        // same pledge goes through.
        node.config.keep_bytes = None;
        set_pledge(&mut node, 1024 * 1024).expect("a small pledge with no keep");
    }

    /// `space --pledge X --apply` is how the installers pledge
    /// (`provision.sh`), and it compared the pledge with the free space
    /// alone: a second instance on the disk could promise what the first had
    /// promised, the bug `pledge` was fixed for on 2026-10-12 (ROADMAP,
    /// "Pledges are not reserved"). It now asks the same question, and says
    /// the promise to the disk at once, as `pledge` does, so the next setup
    /// sees it. Sabotage: drop the disk refusal from `space_on`, or its
    /// `publish_pledge`.
    #[test]
    fn red_team_space_apply_cannot_promise_what_another_node_on_the_disk_promised() {
        const MIB: u64 = 1024 * 1024;
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (mut node, _phrase) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        // A second instance beside it promised more than any disk holds, and
        // stores none of it yet: all of it is still owed by the free space.
        let sibling = base.path().join(".itsanas-dominique");
        fake_node(&sibling, "dominique", None);
        let promised = sibling.join(itsanas_node::pledges::STATE_FILE);
        std::fs::write(
            &promised,
            format!(
                "pledge={}\nheld=0\nupdated={}\n",
                u64::MAX / 2,
                itsanas_discover::now_unix()
            ),
        )
        .expect("the sibling's promise");
        let path = itsanas_node::Node::config_path(&home);
        let before = std::fs::read(&path).expect("config");

        let refused = space_on(&mut node, Some("1M"), None, true);
        assert!(
            refused.is_err(),
            "space --apply promised a disk another node on it had already promised: the \
             second member to fill it finds its host short, through no fault of its own"
        );
        assert_eq!(
            std::fs::read(&path).expect("config"),
            before,
            "a refused space --apply still rewrote the node file"
        );
        assert_eq!(node.config.pledge_bytes, 0, "refused, yet set in memory");
        // One rule for both commands: neither is the way around the other.
        let pledge = set_pledge(&mut node, MIB);
        assert!(
            pledge
                .as_ref()
                .is_err_and(|error| error.to_string().contains("promised space too")),
            "{pledge:?}"
        );

        // With the sibling's promise gone the same numbers fit, and this
        // node's promise is on the disk for the next setup at once, not at
        // its daemon's next round.
        std::fs::remove_file(&promised).expect("remove");
        space_on(&mut node, Some("1M"), None, true).expect("a small pledge on a free disk");
        assert_eq!(node.config.pledge_bytes, MIB);
        let published = itsanas_node::pledges::own(&home).map(|promise| promise.pledge);
        assert_eq!(
            published,
            Some(MIB),
            "space --apply set a pledge the other nodes on this disk cannot see yet: the next \
             instance set up a minute later promises the same room"
        );
    }
}
