//! What a phone shows of its space, and the checks on what it offers and keeps.
//!
//! The figures and the refusals are the phone's own wording (it has no
//! `itsanas space`); the rule under them is the core's.

use itsanas_node::{Config, Node};

use crate::failure::{Answer, Failure, KEEP_SECTION, PLEDGE_SECTION};

/// The figures the Space step and Settings show, in bytes, the disk given.
pub fn space_of(node: &Node, disk: itsanas_node::pledges::DiskSpace) -> Answer {
    use itsanas_node::pledges;
    let asked = itsanas_node::coordinator::other_pledges(node)
        .ok()
        .flatten();
    let others = asked.map_or_else(|| Node::others_pledged(&node.home), |told| told.total);
    let largest = asked.map_or(0, |told| told.largest);

    let (free, total) = (disk.free.unwrap_or(0), disk.total.unwrap_or(0));
    let local = node.store.local_bytes()?;
    let held = node.held_for_others()?;
    let account: u64 = itsanas_store::catalogue(&node.store, &node.vault)?
        .files
        .iter()
        .fold(0, |sum, file| sum.saturating_add(file.size));
    let pledged = others.saturating_add(node.config.pledge_bytes);
    let split = node.config.split;
    let usable = pledges::phone_usable(total, free, local);
    let suggested = pledges::suggest(&node.config, total, free, local, others, largest);

    Ok(serde_json::json!({
        "freeBytes": free,
        "totalBytes": total,
        "localBytes": local,
        "heldForOthers": held,
        "pledgeBytes": node.config.pledge_bytes,
        "keepBytes": node.config.keep_bytes,
        "othersPledgedBytes": others,
        "largestOtherPledgeBytes": largest,
        "accountBytes": account,
        "earnedBytes": split.room_earned(pledged),
        "allowedBytes": Node::allowed_for(&node.config, pledged),
        // What leaves the phone its headroom, and what the disk can promise at
        // all. Sliders end here; `setLimits` is what refuses.
        "maxPledgeBytes": usable.min(free.saturating_add(held)),
        "maxKeepBytes": usable,
        "suggested": {
            "pledgeBytes": suggested.pledge,
            "keepBytes": suggested.keep,
            "sameAsOtherMachines": suggested.same_as_other_machines,
        },
        "split": { "own": split.own, "network": split.network },
    })
    .to_string())
}

/// Say how much of this account's own data to hold here, and which files: the split is checked before anything is changed, so a refused limit
/// leaves order and filter as they were too.
pub fn set_keep(
    node: &mut Node,
    keep: Option<u64>,
    order: itsanas_policy::keeping::Order,
    only: &str,
) -> Result<String, Failure> {
    check_phone_split(node, node.config.pledge_bytes, keep)?;

    node.config.keep_bytes = keep;
    node.config.keep_order = order;
    node.config.keep_only = only
        .split('\n')
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    node.save_config()?;

    Ok(describe_keeping(&node.config))
}

/// Say how much room this device offers other people. A pledge under what the phone's `keep` needs is refused, as
/// the CLI's `pledge` refuses it.
pub fn set_pledge(node: &mut Node, bytes: u64) -> Result<String, Failure> {
    check_phone_split(node, bytes, node.config.keep_bytes)?;

    node.config.pledge_bytes = bytes;
    node.save_config()?;
    Ok(serde_json::json!({ "pledgeBytes": node.config.pledge_bytes }).to_string())
}

/// Set the pledge and the keep together, with what the disk has free given, so a test can ask about a
/// disk the size of its choosing. `None` skips the disk: nothing is known of it.
///
/// Takes the JVM's signed longs so the sign is checked where a test reaches it:
/// a negative pledge read as its absolute value would offer space nobody asked
/// to give (found by `itsanas-redteam`).
pub fn set_limits_on(
    node: &mut Node,
    pledge: i64,
    keep: i64,
    free: Option<u64>,
) -> Result<String, Failure> {
    let pledge = u64::try_from(pledge)
        .map_err(|_| Failure::Usage(format!("a pledge cannot be negative ({pledge} bytes)")))?;
    let keep = u64::try_from(keep).ok();
    check_phone_split(node, pledge, keep)?;
    check_phone_disk(node, pledge, keep, free)?;

    node.config.pledge_bytes = pledge;
    node.config.keep_bytes = keep;
    node.save_config()?;
    Ok(serde_json::json!({
        "pledgeBytes": node.config.pledge_bytes,
        "keepBytes": node.config.keep_bytes,
    })
    .to_string())
}

/// Refuse a keep the account's pledges do not earn.
///
/// The account's, where a desktop's `keep` asks of its own machine's alone: a
/// phone that lends little is the ordinary case, and it may keep what the owner's
/// other machines earn. Phone setters only; `check_split` is unchanged.
fn check_phone_split(node: &Node, pledge: u64, keep: Option<u64>) -> Result<(), Failure> {
    let others = Node::others_pledged(&node.home);
    Node::check_split(&node.config, pledge.saturating_add(others), keep)
        .map_err(|refusal| phone_split(&refusal, others))
}

/// A split refusal in the phone's words.
///
/// `SplitRefusal`'s own `Display` ends with `itsanas space --pledge ... --apply`,
/// a command the phone does not have; this names the field on the settings
/// screen instead, with the same figures: this phone's share of them, since
/// the refusal counts what the account's other machines lend.
fn phone_split(refusal: &itsanas_node::node::SplitRefusal, others: u64) -> Failure {
    use itsanas_node::config::format_size;
    Failure::Usage(format!(
        "keeping {} on this phone needs at least {} in \"{PLEDGE_SECTION}\", and this phone \
         offers {}. Raise that, or keep less here.",
        format_size(refusal.keep),
        format_size(refusal.needed.saturating_sub(others)),
        format_size(refusal.pledge.saturating_sub(others)),
    ))
}

/// Refuse a pledge and a keep this phone's disk cannot hold, in its words: a
/// phone pledging more than its disk has is refused every later write and every
/// pull, with nothing that says why.
fn check_phone_disk(
    node: &Node,
    pledge: u64,
    keep: Option<u64>,
    free: Option<u64>,
) -> Result<(), Failure> {
    use itsanas_node::config::format_size;
    let held = node.held_for_others()?;
    if let Some(refused) = node.disk_refusal_given(free, pledge, held) {
        return Err(Failure::Usage(format!(
            "this phone has {} free, so at most {} can go in \"{PLEDGE_SECTION}\"; {} is more \
             than it could keep. Lower it.",
            format_size(refused.free),
            format_size(refused.room),
            format_size(refused.bytes),
        )));
    }
    let own = node.store.local_bytes()?;
    if let Some(committed) = itsanas_node::pledges::overcommitted(pledge, keep, free, own, held) {
        return Err(Failure::Usage(format!(
            "offering {} and keeping {} needs {}, and this phone has room for {}. Lower \
             \"{PLEDGE_SECTION}\" or \"{KEEP_SECTION}\".",
            format_size(pledge),
            format_size(keep.unwrap_or(0)),
            format_size(committed),
            format_size(free.unwrap_or(0).saturating_add(own).saturating_add(held)),
        )));
    }
    Ok(())
}

fn describe_keeping(config: &Config) -> String {
    serde_json::json!({
        "keepBytes": config.keep_bytes,
        "keepOnly": config.keep_only,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{NOW, phone, with_directory};

    const GIB: u64 = 1 << 30;

    fn gib(n: i64) -> i64 {
        n * (1 << 30)
    }

    /// The phone's setters keep the split as `pledge` and `keep` do: a pledge
    /// lowered under what `keep` needs, or a keep the pledge does not earn,
    /// is refused and the node file is left as it was. Covers `set_pledge`
    /// and `set_keep`.
    /// Sabotage: drop the `check_split` call in either.
    #[test]
    fn red_team_the_phone_s_setters_keep_the_split() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, _phrase) = Node::create(&home, "a long passphrase", "nicolas").unwrap();
        node.config.pledge_bytes = 50 * GIB;
        node.config.keep_bytes = Some(20 * GIB);
        node.save_config().unwrap();
        let path = Node::config_path(&home);
        let before = std::fs::read(&path).unwrap();

        assert!(
            matches!(set_pledge(&mut node, 1024 * 1024), Err(Failure::Usage(_))),
            "setPledge accepted 1 MiB beside a 20 GiB keep"
        );
        let order = node.config.keep_order;
        let only = node.config.keep_only.clone();
        assert!(
            matches!(
                set_keep(&mut node, Some(40 * GIB), order, "Photos"),
                Err(Failure::Usage(_))
            ),
            "setKeep accepted 40 GiB on a 50 GiB pledge"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "a refused setter still rewrote the node file"
        );
        // Nor the node in memory: it lives in a process-wide mutex, so the
        // next setter that succeeds would save what a refused one left there.
        assert_eq!(
            node.config.pledge_bytes,
            50 * GIB,
            "refused pledge, yet set"
        );
        assert_eq!(
            node.config.keep_bytes,
            Some(20 * GIB),
            "refused keep, yet set"
        );
        assert_eq!(node.config.keep_only, only, "refused keep, filter changed");

        // Not setters that refuse everything.
        assert!(
            set_keep(&mut node, None, order, "").is_ok(),
            "no limit refused"
        );
        assert!(
            set_pledge(&mut node, 1024 * 1024).is_ok(),
            "a small pledge with no keep refused"
        );
    }

    /// The settings screen saves pledge and keep together. Raising both is
    /// accepted in one call (saved one after the other, the new keep was
    /// checked against the old pledge and refused), and a pair that does not
    /// fit is refused in words naming the screen's field, not a command the
    /// phone does not have; a negative pledge is refused. Sabotage: check the
    /// keep against the old pledge; word the refusal with `SplitRefusal`'s own
    /// `Display`; take the pledge's absolute value.
    #[test]
    fn red_team_the_phone_saves_pledge_and_keep_as_a_pair_and_says_so_in_its_words() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (mut node, _phrase) = Node::create(&home, "a long passphrase", "nicolas").unwrap();
        node.config.pledge_bytes = 50 * GIB;
        node.config.keep_bytes = Some(20 * GIB);
        node.save_config().unwrap();

        let gib = |n: i64| n * 1024 * 1024 * 1024;
        set_limits_on(&mut node, gib(500), gib(200), None)
            .expect("raising pledge and keep together was refused");
        assert_eq!(
            (node.config.pledge_bytes, node.config.keep_bytes),
            (500 * GIB, Some(200 * GIB))
        );

        assert!(
            set_limits_on(&mut node, -gib(5), 0, None).is_err(),
            "a negative pledge was taken as a positive one"
        );
        let Err(refused) = set_limits_on(&mut node, gib(1), gib(200), None) else {
            panic!("1 GiB offered earned a 200 GiB keep");
        };
        let said = refused.message();
        assert!(
            !said.contains("itsanas ") && said.contains("Space you offer"),
            "the phone was told something it cannot act on: {said}"
        );
        assert_eq!(
            node.config.pledge_bytes,
            500 * GIB,
            "a refused pair still changed the pledge"
        );
    }

    /// A phone's keep is judged against what the account pledges, not this
    /// phone alone; a desktop's rule is unchanged. Sabotage: pass the phone's
    /// pledge alone to `check_split` (a phone beside a 100 GiB NAS may keep 10);
    /// add the others to the desktop's rule.
    #[test]
    fn red_team_a_phone_keeps_what_the_account_earns_not_what_it_lends_itself() {
        let dir = tempfile::tempdir().unwrap();
        let (mut node, _) =
            Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        node.remember_others_pledged(100 * GIB).unwrap();

        set_limits_on(&mut node, 0, gib(42), None)
            .expect("the account's 100 GiB earn 42 GiB, and the phone was refused it");
        let refused = set_limits_on(&mut node, 0, gib(43), None)
            .expect_err("100 GiB earned more than 42 GiB")
            .message();
        assert!(
            refused.contains("Space you offer") && !refused.contains("itsanas "),
            "{refused}"
        );
        let order = node.config.keep_order;
        assert!(set_keep(&mut node, Some(43 * GIB), order, "").is_err());
        assert!(set_keep(&mut node, Some(42 * GIB), order, "").is_ok());
        assert!(set_pledge(&mut node, 0).is_ok());

        assert!(
            Node::check_split(&node.config, 0, Some(42 * GIB)).is_err(),
            "the desktop's rule, per machine, changed with the phone's"
        );
        assert!(
            set_limits_on(&mut node, gib(10), gib(43), None).is_ok(),
            "this phone's pledge no longer counts"
        );
    }

    /// A refusal from the store on a phone names the Settings section, in sizes a
    /// person reads, never a command it has not got. Sabotage: fall through to the
    /// store's own `Display`.
    #[test]
    fn red_team_a_full_disk_and_a_full_account_are_said_in_the_phone_s_words() {
        use itsanas_store::StoreError;
        let full = Failure::from(StoreError::DiskFull {
            path: "one.jpg".to_owned(),
            incoming: 3 * GIB,
            room: GIB,
        })
        .message();
        let over = Failure::from(StoreError::OverBudget {
            path: "one.jpg".to_owned(),
            account: 9 * GIB,
            total: 12 * GIB,
            allowed: 10 * GIB,
        })
        .message();
        for said in [&full, &over] {
            assert!(
                said.contains("Space you offer") && said.contains("Settings"),
                "the phone was not told where to act: {said}"
            );
            assert!(
                !said.contains("itsanas ") && !said.contains(" bytes"),
                "the phone was told a command it lacks, or a count of bytes: {said}"
            );
            assert!(said.contains("one.jpg"), "which file: {said}");
        }
        assert!(
            full.contains("3.0 GiB") && full.contains("1.0 GiB"),
            "{full}"
        );
        assert!(
            over.contains("12.0 GiB") && over.contains("10.0 GiB"),
            "{over}"
        );
    }

    /// The Space step's answer, in the names Kotlin reads, with a sibling's pledge
    /// suggested when the phone has the room. Sabotage: pass 0 for the largest
    /// (the same value is never offered); leave a field out.
    #[test]
    fn red_team_the_space_step_is_told_the_figures_it_shows() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().unwrap();
            let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
            nas.config.pledge_bytes = 100 * GIB;
            itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
            let (mut me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));
            itsanas_node::coordinator::register_with(&me, None, NOW).unwrap();

            let disk = itsanas_node::pledges::DiskSpace {
                free: Some(400 * GIB),
                total: Some(512 * GIB),
            };
            let said: serde_json::Value =
                serde_json::from_str(&space_of(&me, disk).unwrap()).unwrap();
            for key in [
                "freeBytes",
                "totalBytes",
                "localBytes",
                "heldForOthers",
                "pledgeBytes",
                "keepBytes",
                "othersPledgedBytes",
                "largestOtherPledgeBytes",
                "accountBytes",
                "earnedBytes",
                "allowedBytes",
                "maxPledgeBytes",
                "maxKeepBytes",
                "suggested",
                "split",
            ] {
                assert!(said.get(key).is_some(), "Kotlin reads {key}: {said}");
            }
            assert_eq!(said["freeBytes"], 400 * GIB);
            assert_eq!(said["othersPledgedBytes"], 100 * GIB);
            assert_eq!(said["largestOtherPledgeBytes"], 100 * GIB);
            assert_eq!(said["pledgeBytes"], 0);
            assert_eq!(said["suggested"]["pledgeBytes"], 100 * GIB);
            assert_eq!(said["suggested"]["keepBytes"], 85 * GIB);
            assert_eq!(said["suggested"]["sameAsOtherMachines"], true);
            assert_eq!(said["split"]["own"], 30);
            assert_eq!(said["split"]["network"], 70);
            assert_eq!(said["allowedBytes"], 100 * GIB * 30 / 70);

            // Coordinator gone: what it last said, and no "the same".
            me.config.coordinator = Some("127.0.0.1:1".to_owned());
            let said: serde_json::Value =
                serde_json::from_str(&space_of(&me, disk).unwrap()).unwrap();
            assert_eq!(said["othersPledgedBytes"], 100 * GIB, "{said}");
            assert_eq!(said["largestOtherPledgeBytes"], 0, "{said}");
            assert_eq!(said["suggested"]["sameAsOtherMachines"], false, "{said}");
        });
    }
}
