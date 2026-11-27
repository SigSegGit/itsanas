//! `device`: the account's machines as the coordinator lists them, and withdrawing one.

use std::path::Path;

use itsanas_crypto::DeviceId;

use crate::cli::DeviceCommand;
use crate::opening::open;
use crate::status::describe_age;
use crate::{config, coordinator};
use crate::{
    error::{CliError, Result},
    node::Node,
};

pub(crate) fn device(home: &Path, what: &DeviceCommand) -> Result<()> {
    let node = open(home)?;
    let mine = node.store.device_id();

    // A full id needs no listing, and a machine that is not enrolled is
    // refused one. Asking first locked an account out for good: five machines
    // lost or reinstalled, the five slots held by their old ids, and the new
    // machine -- the only one left -- refused `register` for want of a slot
    // and refused `forget` for want of being enrolled (found by
    // `itsanas-redteam`). The coordinator takes a withdrawal signed by the
    // account from any connection, so nothing here needs to be enrolled.
    if let DeviceCommand::Forget { device } = what
        && let Ok(wanted) = device.parse::<DeviceId>()
    {
        return withdraw(&node, wanted);
    }

    // Every enrolled device where the coordinator can say so, and the
    // reachable ones where it is too old to. The reachable list leaves out a
    // machine silent for a week, which is the lost laptop somebody came here
    // to find, so falling back is said out loud rather than done quietly.
    let enrolled = coordinator::enrolled(&node)?;
    let listed: Vec<(DeviceId, String)> = if let Some(list) = &enrolled {
        list.iter()
            .map(|entry| (entry.device, entry.address.clone().unwrap_or_default()))
            .collect()
    } else {
        println!(
            "this coordinator cannot list devices that have gone quiet (it is older than this \
             client, or the connection dropped); showing only those seen in the last week."
        );
        coordinator::devices(&node, node.store.owner())?
    };

    match what {
        DeviceCommand::List => {
            if listed.is_empty() {
                println!("the coordinator lists no devices for this account");
                return Ok(());
            }
            match &enrolled {
                Some(list) => {
                    for entry in by_silence(list) {
                        println!("{}", describe_enrolled(entry, entry.device == mine));
                    }
                    // The coordinator stops at this many. Saying so is the
                    // difference between "the machine is not enrolled" and
                    // "the machine is past the end of the list".
                    if list.len() >= itsanas_coord::protocol::MAX_PEERS_RETURNED {
                        println!(
                            "(the coordinator lists at most {} devices; there may be more, silent longest)",
                            itsanas_coord::protocol::MAX_PEERS_RETURNED
                        );
                    }
                }
                None => {
                    for (device, address) in &listed {
                        let here = if *device == mine {
                            "  (this machine)"
                        } else {
                            ""
                        };
                        println!("{device}  {address}{here}");
                    }
                }
            }
            Ok(())
        }

        DeviceCommand::Forget { device } => withdraw(&node, resolve_device(device, &listed)?),
    }
}

/// `itsanas device forget`, once the device is known.
pub(crate) fn withdraw(node: &Node, wanted: DeviceId) -> Result<()> {
    // The rule against withdrawing this machine is in `withdraw_device`, so
    // the Android app applies the same one.
    coordinator::withdraw_device(node, wanted, itsanas_discover::now_unix())?;
    println!("withdrew {wanted}");
    println!("  Nothing will dial it through the coordinator again, and the");
    println!("  withdrawal is final for that device id. To use that machine");
    println!("  again, remove its node directory and `itsanas login` on it:");
    println!("  it comes back as a new device.");
    println!("  A thief who also has its passphrase holds the account's master");
    println!("  key and can read what it stores; withdrawing cannot undo that.");
    Ok(())
}

/// What `status` says this node publishes.
pub(crate) fn describe_announce(node: &Node) -> String {
    node.config
        .announce
        .clone()
        .unwrap_or_else(|| "this machine's address on the network it is on".to_owned())
}

/// The synced folder's line in `status`, and the warning when it is not there.
///
/// A folder whose marker is gone is either a disk that is not mounted or a
/// directory somebody emptied. Both stop syncing, and neither says so anywhere
/// else, which is how a disk stays unmounted for a week.
pub(crate) fn describe_folder(folder: &std::path::Path) -> String {
    let mut out = format!("  synced folder   {}", folder.display());
    if !folder.join(itsanas_folder::scan::MARKER).exists() {
        out.push_str(
            "
                  STORAGE UNREACHABLE: no marker there.",
        );
        out.push_str(
            "
                  A disk or share that is not mounted leaves an empty",
        );
        out.push_str(
            "
                  directory behind. Nothing has been deleted.",
        );
    }
    out
}

/// Whether this machine may be unplugged or switched off without losing a
/// file, from the complete copies of everything written here that exist on
/// other machines.
pub(crate) fn unplug_words(complete_elsewhere: usize) -> String {
    if complete_elsewhere == 0 {
        "  unplugging     NOT yet: some of what is here is on no other machine; keep it connected until a round copies it".to_owned()
    } else {
        format!(
            "  unplugging     loses nothing: everything here has {complete_elsewhere} complete copy(ies) on other machines"
        )
    }
}

/// The enrolled devices, the most silent first: never announced, then the
/// longest silence. The machine somebody opens the list to find is the one
/// that has gone quiet, and it should not be at the bottom of twenty.
pub(crate) fn by_silence(
    list: &[itsanas_coord::protocol::EnrolledDevice],
) -> Vec<&itsanas_coord::protocol::EnrolledDevice> {
    let mut sorted: Vec<_> = list.iter().collect();
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.silent_for.unwrap_or(u64::MAX)));
    sorted
}

/// Whether a device silent for `silent_for` is resting past a week: said in
/// words, never in colour, because a key in a drawer is not a fault.
pub(crate) const fn dormant(silent_for: Option<u64>) -> bool {
    match silent_for {
        Some(seconds) => seconds > itsanas_store::holders::LIVE_FOR,
        None => true,
    }
}

/// One line of `itsanas device list`.
///
/// The silence is the coordinator's own measurement, so it is what decides
/// whether a device is "lost"; this machine's clock never enters it.
pub(crate) fn describe_enrolled(
    entry: &itsanas_coord::protocol::EnrolledDevice,
    here: bool,
) -> String {
    let heard = match entry.silent_for {
        Some(seconds) => format!("heard from {}", describe_age(seconds)),
        None => "never announced".to_owned(),
    };
    let address = entry.address.as_deref().unwrap_or("no address");
    let this = if here { "  (this machine)" } else { "" };
    let resting = if dormant(entry.silent_for) && !here {
        "  resting"
    } else {
        ""
    };
    format!(
        "{}  {address}  pledges {}  {heard}{resting}{this}",
        entry.device,
        config::format_size(entry.pledged_bytes)
    )
}

/// Turn what somebody typed into a device on this account.
///
/// Accepts the full identifier, and the short form the logs print. A short form
/// is matched against the account's own devices rather than parsed, so a string
/// that matches nothing says so -- instead of becoming an identifier for a
/// device that does not exist, which the coordinator would accept and file a
/// revocation against, silently, for ever.
pub(crate) fn resolve_device(typed: &str, listed: &[(DeviceId, String)]) -> Result<DeviceId> {
    if let Ok(parsed) = typed.parse::<DeviceId>() {
        return Ok(parsed);
    }

    let matches: Vec<DeviceId> = listed
        .iter()
        .map(|(device, _)| *device)
        .filter(|device| device.to_string().starts_with(typed))
        .collect();

    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!(
            "no device on this account starts with {typed:?}. `itsanas device list` shows them."
        ))),
        several => Err(CliError::Usage(format!(
            "{} devices start with {typed:?}; give more of it.",
            several.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_device;
    use itsanas_crypto::DeviceId;

    /// `itsanas device list` puts the machine somebody opened it to find --
    /// the one gone quiet -- first, never announced before any silence, and
    /// says "resting" past a week in a word, for no other machine and never
    /// for this one. Sabotage: drop the sort in `by_silence`.
    #[test]
    fn red_team_the_device_list_puts_the_quiet_machine_first_and_calls_it_resting() {
        use itsanas_coord::protocol::EnrolledDevice;
        let id = |seed: u8| {
            itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]))
                .device_id()
        };
        let day = 24 * 3600;
        let row = |seed: u8, silent_for: Option<u64>| EnrolledDevice {
            device: id(seed),
            pledged_bytes: 0,
            silent_for,
            address: None,
        };
        let listed = vec![
            row(1, Some(60)),
            row(2, Some(30 * day)),
            row(3, None),
            row(4, Some(8 * day)),
        ];
        let order: Vec<_> = super::by_silence(&listed)
            .iter()
            .map(|entry| entry.device)
            .collect();
        assert_eq!(
            order,
            vec![id(3), id(2), id(4), id(1)],
            "the quiet machines are not first"
        );
        let line = |seed: u8, here: bool| {
            let entry = listed
                .iter()
                .find(|entry| entry.device == id(seed))
                .expect("row");
            super::describe_enrolled(entry, here)
        };
        assert!(
            line(2, false).contains("resting"),
            "a month silent is not said: {}",
            line(2, false)
        );
        assert!(
            !line(1, false).contains("resting"),
            "a machine heard a minute ago is called resting"
        );
        assert!(
            !line(2, true).contains("resting"),
            "this machine calls itself resting"
        );
    }

    /// "May I unplug it?" is answered from the copies elsewhere: no while a
    /// file is here alone, which is when unplugging loses it; yes, with the
    /// count, once every file has a complete copy elsewhere. Sabotage: answer
    /// yes from zero copies.
    #[test]
    fn red_team_status_says_unplugging_loses_nothing_only_once_a_copy_is_elsewhere() {
        assert!(
            super::unplug_words(0).contains("NOT yet"),
            "a key whose files are nowhere else was told it could go: {}",
            super::unplug_words(0)
        );
        assert!(super::unplug_words(1).contains("loses nothing"));
        assert!(super::unplug_words(2).contains("2 complete"));
    }

    fn listed() -> Vec<(DeviceId, String)> {
        vec![
            (DeviceId::from_bytes([0xab; 32]), "a:1".to_owned()),
            (DeviceId::from_bytes([0xcd; 32]), "b:2".to_owned()),
        ]
    }

    #[test]
    fn a_prefix_that_names_no_device_is_refused_rather_than_invented() {
        // This is the half that can do damage. A revocation is a signed record
        // the coordinator files and honours; one written against an identifier
        // nobody holds is silent, permanent, and impossible to notice.
        let devices = listed();
        for typed in ["ffff", "0", "not-hex", "abcdef01", ""] {
            assert!(
                resolve_device(typed, &devices).is_err(),
                concat!(
                    "{:?} was resolved to a device, and no device on the ",
                    "account starts with it"
                ),
                typed
            );
        }
    }

    #[test]
    fn the_short_form_the_logs_print_is_enough_to_name_a_device() {
        // The error a person is reacting to prints twelve characters. Making
        // them go and find the other fifty-two is asking them to do work the
        // program can do -- but only when the answer is unambiguous.
        let devices = listed();
        let full = DeviceId::from_bytes([0xab; 32]);

        assert_eq!(resolve_device(&full.to_string(), &devices).unwrap(), full);
        assert_eq!(resolve_device(&full.short(), &devices).unwrap(), full);

        // "ab" is unique here; a prefix shared by both must refuse rather than
        // pick one.
        assert_eq!(resolve_device("ab", &devices).unwrap(), full);
        assert!(resolve_device("", &devices).is_err());
    }
}
