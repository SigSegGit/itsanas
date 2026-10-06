//! The last step: does it work, seen from outside, within a deadline.
//!
//! Four questions, each answered by looking rather than by trusting the steps
//! before it -- a setup that reports success on a machine that does not sync
//! is the failure this whole wizard exists to end (the live test of
//! 2026-10-06 found machines "installed" that had never synced a file):
//!
//! 1. **Is the daemon healthy?** `status --brief`'s rule ([`crate::brief_status`]):
//!    it holds the store and wrote a snapshot within two intervals.
//! 2. **Does the coordinator list this device?** What the account's other
//!    machines will be told when they look for it.
//! 3. **Can the outside reach it?** The coordinator dials back -- `doctor`'s
//!    network half ([`crate::network_check`]), called, not copied.
//! 4. **Does the folder sync?** A small file written into it must show up in
//!    the snapshot's file count; then it is removed.
//!
//! Each answer is a [`Finding`] with a verdict, what was seen, and the one
//! thing to do when it failed. The [`Report`] is data, so the web page of
//! 0w (4) shows the same findings this terminal prints.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use std::fmt::Write as _;

use itsanas_node::node::Identity;

use crate::{coordinator::Reachability, node::Node};

/// What one check concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Passed,
    Failed,
    /// Not asked, or not answerable here; says why in the detail.
    Skipped,
}

/// One check: its verdict, what was seen, and what to do if it failed.
#[derive(Clone, Debug)]
pub(crate) struct Finding {
    pub(crate) what: &'static str,
    pub(crate) verdict: Verdict,
    pub(crate) detail: String,
    /// Empty unless it failed.
    pub(crate) remedy: String,
}

impl Finding {
    fn passed(what: &'static str, detail: impl Into<String>) -> Self {
        Self {
            what,
            verdict: Verdict::Passed,
            detail: detail.into(),
            remedy: String::new(),
        }
    }

    fn skipped(what: &'static str, detail: impl Into<String>) -> Self {
        Self {
            what,
            verdict: Verdict::Skipped,
            detail: detail.into(),
            remedy: String::new(),
        }
    }

    fn failed(what: &'static str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            what,
            verdict: Verdict::Failed,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }

    /// One line for a terminal.
    pub(crate) fn line(&self) -> String {
        let mark = match self.verdict {
            Verdict::Passed => "ok  ",
            Verdict::Failed => "FAIL",
            Verdict::Skipped => "skip",
        };
        if self.remedy.is_empty() {
            format!("{mark} {}: {}", self.what, self.detail)
        } else {
            format!(
                "{mark} {}: {}\n       what to do: {}",
                self.what, self.detail, self.remedy
            )
        }
    }
}

/// Every finding, in the order they were checked.
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    pub(crate) findings: Vec<Finding>,
}

impl Report {
    pub(crate) fn failures(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.verdict == Verdict::Failed)
            .collect()
    }

    pub(crate) fn summary(&self) -> String {
        let count = |verdict| {
            self.findings
                .iter()
                .filter(|finding| finding.verdict == verdict)
                .count()
        };
        format!(
            "{} passed, {} failed, {} skipped",
            count(Verdict::Passed),
            count(Verdict::Failed),
            count(Verdict::Skipped)
        )
    }
}

/// What the checks need.
#[derive(Debug)]
pub(crate) struct Inputs<'a> {
    pub(crate) home: &'a Path,
    /// The keys, when a coordinator is set (checks 2 and 3 sign with them).
    pub(crate) identity: Option<&'a Identity>,
    /// Whether a background service was meant to be running.
    pub(crate) daemon_expected: bool,
    pub(crate) folder: Option<&'a Path>,
    /// How long the daemon and the folder have to show themselves.
    pub(crate) deadline: Duration,
    /// Where the daemon's log is, for the remedies.
    pub(crate) log_hint: String,
}

/// Run every check.
pub(crate) fn run(inputs: &Inputs<'_>) -> Report {
    let started = Instant::now();
    let mut findings = vec![daemon_health(inputs, started)];
    let healthy = findings[0].verdict == Verdict::Passed;
    match inputs.identity {
        None => {
            findings.push(Finding::skipped(
                "listed by the coordinator",
                "no coordinator is set",
            ));
            findings.push(Finding::skipped(
                "reachable from outside",
                "no coordinator is set to dial back",
            ));
        }
        Some(identity) => findings.extend(network_findings(identity, healthy)),
    }
    findings.push(if healthy {
        canary(inputs, started)
    } else {
        Finding::skipped(
            "the folder syncs",
            "no healthy daemon to take a file in; start it, then run setup again",
        )
    });
    Report { findings }
}

fn daemon_health(inputs: &Inputs<'_>, started: Instant) -> Finding {
    const WHAT: &str = "the daemon is healthy";
    if !inputs.daemon_expected {
        return Finding::skipped(WHAT, "no background service was asked for");
    }
    let last = loop {
        let running = itsanas_store::Store::is_locked(Node::store_path(inputs.home));
        let state = crate::brief_status(inputs.home, running, itsanas_discover::now_unix());
        if state.starts_with("healthy") || state.starts_with("paused") {
            return Finding::passed(WHAT, state);
        }
        if started.elapsed() >= inputs.deadline {
            break state;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    Finding::failed(
        WHAT,
        format!("after {} s it is still {last:?}", inputs.deadline.as_secs()),
        format!(
            "read the daemon's log ({}); a first round can be slow, so run `itsanas status \
             --brief` again in a minute",
            inputs.log_hint
        ),
    )
}

/// The verdict on what the coordinator saw when it dialled back.
///
/// Unreachable is a failure only when this machine **announces** an address:
/// then somebody meant it to be reached there. A laptop announcing nothing
/// takes part by dialling out, and calling that a failure is what sends
/// somebody to rewire a router that works (the same rule as `doctor`).
pub(crate) fn reach_finding(inbound: &crate::Inbound, announced: Option<&str>) -> Finding {
    const WHAT: &str = "reachable from outside";
    match (inbound, announced) {
        (crate::Inbound::Answered(Reachability::Reachable(detail)), _) => {
            Finding::passed(WHAT, detail.clone())
        }
        (crate::Inbound::Answered(Reachability::Unreachable(detail)), Some(announce)) => {
            Finding::failed(
                WHAT,
                format!("this machine announces {announce}, and nothing reached it: {detail}"),
                "check the router's forward, and that it points at this machine's listening port \
                 (`itsanas listen`)",
            )
        }
        (crate::Inbound::Answered(Reachability::Unreachable(_)), None) => Finding::skipped(
            WHAT,
            "nothing dials in, which is expected: this machine announces no address and takes \
             part by dialling out",
        ),
        (crate::Inbound::Answered(Reachability::Unknown(why)), _) => {
            Finding::skipped(WHAT, format!("not checked this time: {why}"))
        }
        (crate::Inbound::TooOld, _) => {
            Finding::skipped(WHAT, "this coordinator is too old to dial back")
        }
        (crate::Inbound::Failed(why), _) => Finding::skipped(WHAT, format!("not checked: {why}")),
    }
}

fn network_findings(identity: &Identity, healthy: bool) -> Vec<Finding> {
    const LISTED: &str = "listed by the coordinator";
    let check = crate::network_check(identity);
    let address = check.coordinator.clone().unwrap_or_default();
    match &check.outbound {
        Err(why) => vec![
            Finding::failed(
                LISTED,
                format!("the coordinator at {address} could not be reached: {why}"),
                "check the address, that the coordinator runs, and the firewall between",
            ),
            Finding::skipped("reachable from outside", "the coordinator was not reached"),
        ],
        Ok(peers) => {
            let listed = if peers.me_listed {
                Finding::passed(
                    LISTED,
                    format!(
                        "{address} lists this device, and {} other machine(s) of the account",
                        peers.elsewhere
                    ),
                )
            } else {
                Finding::failed(
                    LISTED,
                    format!("{address} answered, and does not list this device"),
                    "run `itsanas register`; its message says why the coordinator refused",
                )
            };
            let reach = match (&check.inbound, healthy) {
                (_, false) => Finding::skipped(
                    "reachable from outside",
                    "nothing is listening yet, so a dial-back would prove nothing",
                ),
                (Some(inbound), true) => {
                    reach_finding(inbound, identity.config.announce.as_deref())
                }
                (None, true) => Finding::skipped("reachable from outside", "not asked"),
            };
            vec![listed, reach]
        }
    }
}

/// The snapshot's stamp and file count, if it has both.
pub(crate) fn snapshot_counts(text: &str) -> Option<(u64, u64)> {
    let mut lines = text.lines();
    let stamp = lines
        .next()?
        .strip_prefix("snapshot ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let files = lines
        .find_map(|line| line.trim_start().strip_prefix("files"))?
        .trim()
        .parse()
        .ok()?;
    Some((stamp, files))
}

fn read_counts(home: &Path) -> Option<(u64, u64)> {
    snapshot_counts(&std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).ok()?)
}

/// Ask the running daemon for a round now (`itsanas sync-now`'s request).
fn ask_for_a_round(home: &Path) {
    if let Ok(mut control) = crate::control::Control::read(home) {
        control.sync_asked = Some(itsanas_discover::now_unix());
        let _ = control.write(home);
    }
}

fn canary(inputs: &Inputs<'_>, started: Instant) -> Finding {
    const WHAT: &str = "the folder syncs";
    let Some(folder) = inputs.folder else {
        return Finding::skipped(WHAT, "no folder is set");
    };
    if crate::control::paused_on_disk(inputs.home) {
        return Finding::skipped(WHAT, "syncing is paused; `itsanas resume` resumes it");
    }
    let Some((stamp, files)) = read_counts(inputs.home) else {
        return Finding::failed(
            WHAT,
            "the daemon has written no snapshot to compare with",
            format!("read the daemon's log ({})", inputs.log_hint),
        );
    };
    let mut tag = [0u8; 6];
    let _ = getrandom::fill(&mut tag);
    let name = format!(
        "itsanas-setup-check-{}.txt",
        tag.iter().fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
    );
    let path = folder.join(&name);
    if let Err(error) = std::fs::write(
        &path,
        "Written by `itsanas setup` to check that this folder syncs. It removes itself.\n",
    ) {
        return Finding::failed(
            WHAT,
            format!("could not write into {}: {error}", folder.display()),
            "check that the folder is there and yours to write",
        );
    }
    ask_for_a_round(inputs.home);
    // The whole deadline again, not what the daemon check left: a first round
    // on a machine that just joined can be long, and that is not a failure.
    let seen = loop {
        match read_counts(inputs.home) {
            Some((now, count)) if now > stamp && count > files => break true,
            _ if started.elapsed() >= inputs.deadline.saturating_mul(2) => break false,
            _ => std::thread::sleep(Duration::from_secs(1)),
        }
    };
    let _ = std::fs::remove_file(&path);
    ask_for_a_round(inputs.home);
    if seen {
        Finding::passed(
            WHAT,
            format!(
                "a file written into {} reached the account, and was removed",
                folder.display()
            ),
        )
    } else {
        Finding::failed(
            WHAT,
            format!(
                "a file written into {} did not reach the account in time",
                folder.display()
            ),
            format!(
                "read the daemon's log ({}); `itsanas status` says whether the folder is reachable",
                inputs.log_hint
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreachable_laptop_is_not_a_failure_an_unreachable_forward_is() {
        let unreachable = crate::Inbound::Answered(Reachability::Unreachable("refused".to_owned()));
        assert_eq!(
            reach_finding(&unreachable, None).verdict,
            Verdict::Skipped,
            "a laptop that dials out was failed for not being dialled: its owner would go and \
             rewire a router that works"
        );
        let failed = reach_finding(&unreachable, Some("example.org:9801"));
        assert_eq!(
            failed.verdict,
            Verdict::Failed,
            "an announced address nothing reaches was passed: other members would dial it and \
             never sync with this machine"
        );
        assert!(
            !failed.remedy.is_empty(),
            "a failure that says nothing about what to do"
        );
        assert_eq!(
            reach_finding(
                &crate::Inbound::Answered(Reachability::Unknown("asked recently".to_owned())),
                Some("example.org:9801")
            )
            .verdict,
            Verdict::Skipped,
            "a dial-back that was not tried was reported as a verdict"
        );
    }

    #[test]
    fn the_snapshot_count_is_read_as_the_daemon_writes_it() {
        assert_eq!(
            snapshot_counts("snapshot 1759750000 every 300\n  files           12\n  other 3\n"),
            Some((1_759_750_000, 12)),
            "the canary could never be seen arriving, and every setup would fail its last check"
        );
        assert_eq!(
            snapshot_counts("  files 3\n"),
            None,
            "a snapshot with no stamp was read as a fresh one"
        );
    }
}
