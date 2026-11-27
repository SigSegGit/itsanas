//! The on-disk `key = value` form of a [`Config`](super::Config): the words
//! each setting is written as, rendering, parsing, and where a portable home
//! keeps its folder.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use itsanas_coord::accounting::Split;
use itsanas_policy::keeping::Order;

use super::{
    Config, DEFAULT_CHARGING_MINUTES, DEFAULT_HOSTING, PlacementMode, Updates, parse_announce,
    parse_listen,
};
use crate::error::{NodeError, Result};

/// The on-disk name of an ordering, and its parser.
///
/// Written out rather than derived so that renaming a variant cannot silently
/// change the meaning of every configuration file already on a disk somewhere.
const fn order_name(order: Order) -> &'static str {
    match order {
        Order::Newest => "newest",
        Order::Oldest => "oldest",
        Order::Smallest => "smallest",
    }
}

/// Parse an ordering, or `None` if it is not one.
#[must_use]
pub fn parse_order(value: &str) -> Option<Order> {
    match value {
        "newest" => Some(Order::Newest),
        "oldest" => Some(Order::Oldest),
        "smallest" => Some(Order::Smallest),
        _ => None,
    }
}

/// The `updates` line, or why it is wrong.
fn parse_updates(value: &str, line: usize) -> Result<Updates> {
    Updates::parse(value).ok_or_else(|| {
        NodeError::Config(format!(
            "line {line}: updates must be auto, notify or off, found {value:?}"
        ))
    })
}

/// The `reserve` line, or why it is wrong.
fn parse_reserve(value: &str, line: usize) -> Result<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(NodeError::Config(format!(
            "line {line}: reserve must be on or off, found {value:?}"
        ))),
    }
}

/// The `placement` line, or why it is wrong.
fn parse_placement(value: &str, line: usize) -> Result<PlacementMode> {
    PlacementMode::parse(value).ok_or_else(|| {
        NodeError::Config(format!(
            "line {line}: placement must be everyone or measured, found {value:?}"
        ))
    })
}

/// One line of the hosting settings, or `false` if it is not one of them.
fn hosting_line(config: &mut Config, key: &str, value: &str, number: usize) -> Result<bool> {
    match key {
        "contribute" => {
            config.contribute = value.parse().map_err(|_| {
                NodeError::Config(format!(
                    "line {}: contribute must be true or false, found {value:?}",
                    number + 1
                ))
            })?;
        }
        "battery_floor_percent" => {
            config.battery_floor_percent = value
                .parse()
                .ok()
                .filter(|percent| *percent <= 100)
                .ok_or_else(|| {
                    NodeError::Config(format!(
                        "line {}: battery_floor_percent must be 0 to 100, found {value:?}",
                        number + 1
                    ))
                })?;
        }
        "every_charging_minutes" => {
            config.every_charging_minutes = value
                .parse()
                .ok()
                .filter(|minutes| *minutes >= 1)
                .ok_or_else(|| {
                    NodeError::Config(format!(
                        "line {}: every_charging_minutes must be 1 or more, found {value:?}",
                        number + 1
                    ))
                })?;
        }
        "metered_allowance_bytes" => {
            config.metered_allowance_bytes = value.parse().map_err(|_| {
                NodeError::Config(format!(
                    "line {}: metered_allowance_bytes must be a whole number of bytes, \
                     found {value:?}",
                    number + 1
                ))
            })?;
        }
        "metered_period" => {
            config.metered_period = itsanas_policy::Period::parse(value).ok_or_else(|| {
                NodeError::Config(format!(
                    "line {}: metered_period must be hour, day, week or month, \
                         found {value:?}",
                    number + 1
                ))
            })?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

impl Config {
    /// Render to the on-disk form.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# ITSaNAS node configuration.\n");
        out.push_str("# Secrets are not stored here; they live in the sealed keystore.\n\n");

        let _ = writeln!(out, "username = {}", self.username);
        let _ = writeln!(out, "pledge_bytes = {}", self.pledge_bytes);
        // Only what differs: a file with no such line still opens in an older
        // version, and a machine that never touched these writes none.
        if self.contribute != DEFAULT_HOSTING.contribute {
            let _ = writeln!(out, "contribute = {}", self.contribute);
        }
        if self.battery_floor_percent != DEFAULT_HOSTING.battery_floor_percent {
            let _ = writeln!(
                out,
                "battery_floor_percent = {}",
                self.battery_floor_percent
            );
        }
        if self.every_charging_minutes != DEFAULT_CHARGING_MINUTES {
            let _ = writeln!(
                out,
                "every_charging_minutes = {}",
                self.every_charging_minutes
            );
        }
        if self.metered_allowance_bytes != DEFAULT_HOSTING.metered_allowance {
            let _ = writeln!(
                out,
                "metered_allowance_bytes = {}",
                self.metered_allowance_bytes
            );
        }
        if self.metered_period != DEFAULT_HOSTING.metered_period {
            let _ = writeln!(out, "metered_period = {}", self.metered_period.name());
        }
        let _ = writeln!(out, "split = {}", self.split);
        let _ = writeln!(out, "listen = {}", self.listen);
        if let Some(announce) = &self.announce {
            let _ = writeln!(out, "announce = {announce}");
        }
        if let Some(keep) = self.keep_bytes {
            let _ = writeln!(out, "keep_bytes = {keep}");
        }
        if self.keep_bytes.is_some() || !self.keep_only.is_empty() {
            let _ = writeln!(out, "keep_order = {}", order_name(self.keep_order));
        }
        for only in &self.keep_only {
            let _ = writeln!(out, "keep_only = {only}");
        }
        if let Some(vault) = &self.vault {
            let _ = writeln!(out, "vault = {}", vault.display());
        }
        if let Some(folder) = &self.folder {
            let _ = writeln!(out, "folder = {}", folder.display());
        }
        if let Some(coordinator) = &self.coordinator {
            let _ = writeln!(out, "coordinator = {coordinator}");
        }
        if let Some(device) = &self.coordinator_device {
            let _ = writeln!(out, "coordinator_device = {device}");
        }
        for peer in &self.peers {
            let _ = writeln!(out, "peer = {peer}");
        }
        // Only when chosen: a file without it reads as the default, and an
        // older binary (which refuses unknown keys) still reads the file of a
        // node that never changed it.
        if self.updates != Updates::default() {
            let _ = writeln!(out, "updates = {}", self.updates.name());
        }
        // Same rule as `updates`: an older binary refuses the key, so a node
        // that never chose measured placement must not write it.
        if self.placement != PlacementMode::default() {
            let _ = writeln!(out, "placement = {}", self.placement.name());
        }
        // Same rule again: written only when chosen.
        if self.reserve {
            let _ = writeln!(out, "reserve = on");
        }

        out
    }

    /// Parse the on-disk form.
    ///
    /// Unknown keys are an error rather than being ignored. A typo in a config
    /// file that is silently discarded is how a node ends up pledging nothing
    /// while its operator believes it pledged a terabyte.
    pub fn parse(text: &str) -> Result<Self> {
        let mut config = Self::default();
        let mut peers = Vec::new();

        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(NodeError::Config(format!(
                    "line {}: expected `key = value`, found {line:?}",
                    number + 1
                )));
            };

            let key = key.trim();
            let value = value.trim();

            match key {
                "username" => value.clone_into(&mut config.username),
                "listen" => {
                    parse_listen(value).map_err(|error| {
                        NodeError::Config(format!("line {}: {error}", number + 1))
                    })?;
                    value.clone_into(&mut config.listen);
                }
                "pledge_bytes" => {
                    config.pledge_bytes = value.parse().map_err(|_| {
                        NodeError::Config(format!(
                            "line {}: pledge_bytes must be a whole number of bytes, found {value:?}",
                            number + 1
                        ))
                    })?;
                }
                "split" => {
                    let split = Split::parse(value).ok_or_else(|| {
                        NodeError::Config(format!(
                            "line {}: split must be `own/network` with both above \
                             zero, such as `30/70`, found {value:?}",
                            number + 1
                        ))
                    })?;
                    // Stricter than the network, never more generous. Until a
                    // host bounds what an owner stores, `itsanas keep` is the
                    // only thing applying the bargain, and a generous split here
                    // would switch it off from a text editor.
                    if !split.grants_no_more_than(Split::DEFAULT) {
                        return Err(NodeError::Config(format!(
                            "line {}: split {split} keeps more for this machine's owner \
                             than the network's {}; a node may be stricter than that, \
                             never more generous",
                            number + 1,
                            Split::DEFAULT
                        )));
                    }
                    config.split = split;
                }
                "keep_order" => {
                    config.keep_order = parse_order(value).ok_or_else(|| {
                        NodeError::Config(format!(
                            "line {}: keep_order must be newest, oldest or smallest, found {value:?}",
                            number + 1
                        ))
                    })?;
                }
                "announce" => {
                    config.announce = Some(parse_announce(value).map_err(|error| {
                        NodeError::Config(format!("line {}: {error}", number + 1))
                    })?);
                }
                "keep_only" => config.keep_only.push(value.to_owned()),
                "keep_bytes" => {
                    config.keep_bytes = Some(value.parse().map_err(|_| {
                        NodeError::Config(format!(
                            "line {}: keep_bytes must be a whole number of bytes, found {value:?}",
                            number + 1
                        ))
                    })?);
                }
                "folder" => config.folder = Some(PathBuf::from(value)),
                "vault" => config.vault = Some(PathBuf::from(value)),
                "coordinator" => config.coordinator = Some(value.to_owned()),
                "coordinator_device" => config.coordinator_device = Some(value.to_owned()),
                "peer" => peers.push(value.to_owned()),
                "updates" => config.updates = parse_updates(value, number + 1)?,
                "placement" => config.placement = parse_placement(value, number + 1)?,
                "reserve" => config.reserve = parse_reserve(value, number + 1)?,
                other if hosting_line(&mut config, other, value, number)? => {}
                other => {
                    // A backslash continuation here reached the repository with
                    // its second continuation eaten, so this line printed
                    // "peer," and twenty-six spaces before "coordinator".
                    // `concat!` cannot capture `other` implicitly, hence the
                    // explicit argument.
                    return Err(NodeError::Config(format!(
                        concat!(
                            "line {}: unknown setting {:?}. Known settings: ",
                            "username, pledge_bytes, split, keep_bytes, keep_order, ",
                            "contribute, battery_floor_percent, every_charging_minutes, ",
                            "metered_allowance_bytes, metered_period, ",
                            "keep_only, ",
                            "listen, announce, folder, vault, peer, ",
                            "coordinator, coordinator_device, updates, placement, reserve"
                        ),
                        number + 1,
                        other
                    )));
                }
            }
        }

        config.peers = peers;
        Ok(config)
    }

    /// Read from `path`, or return defaults if it does not exist.
    ///
    /// A relative `folder` is relative to the home holding the file: what a
    /// portable home writes (see [`Self::save`]).
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut config = Self::parse(&text)?;
                if let (Some(folder), Some(home)) = (&config.folder, path.parent())
                    && folder.is_relative()
                {
                    config.folder = Some(beside(home, folder));
                }
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(NodeError::Io {
                path: path.to_owned(),
                source: error,
            }),
        }
    }

    /// Write to `path`.
    ///
    /// In a portable home (one holding [`PORTABLE_MARKER`]) a folder next to
    /// the home is written relative to it, with `/`: a key is `E:` on one
    /// computer, `/Volumes/KEY` on a Mac and `/media/me/KEY` on Linux, and an
    /// absolute folder would point at the first computer's drive everywhere
    /// else.
    pub fn save(&self, path: &Path) -> Result<()> {
        let relative = path
            .parent()
            .filter(|home| home.join(PORTABLE_MARKER).is_file())
            .zip(self.folder.as_deref())
            .and_then(|(home, folder)| relative_to(home, folder));
        let text = match relative {
            Some(folder) => Self {
                folder: Some(PathBuf::from(folder)),
                ..self.clone()
            }
            .render(),
            None => self.render(),
        };
        // Written beside and renamed over, so a crash leaves the old config or
        // the new one, never a truncated file that loads as the defaults (no
        // vault, no folder) and starts the node on an empty vault.
        let pending = path.with_extension("new");
        let io = |at: &Path| {
            let at = at.to_owned();
            move |source| NodeError::Io { path: at, source }
        };
        let _ = std::fs::remove_file(&pending);
        std::fs::File::create_new(&pending)
            .and_then(|mut file| {
                std::io::Write::write_all(&mut file, text.as_bytes())?;
                file.sync_all()
            })
            .map_err(io(&pending))?;
        std::fs::rename(&pending, path).map_err(io(path))
    }
}

/// The file in a home that makes its node a portable one: a client only,
/// whose home moves from computer to computer (`itsanas portable`).
pub const PORTABLE_MARKER: &str = "portable";

/// `relative` (as [`relative_to`] writes it) resolved against `home`.
fn beside(home: &Path, relative: &Path) -> PathBuf {
    let mut out = home.to_path_buf();
    let text = relative.to_string_lossy();
    for part in text.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            name => out.push(name),
        }
    }
    out
}

/// `folder` relative to `home`, `/`-separated, when it is inside the home
/// or beside it; `None` otherwise.
fn relative_to(home: &Path, folder: &Path) -> Option<String> {
    let words = |rest: &Path| -> Option<Vec<String>> {
        rest.components()
            .map(|part| match part {
                std::path::Component::Normal(name) => name.to_str().map(str::to_owned),
                _ => None,
            })
            .collect()
    };
    if let Ok(rest) = folder.strip_prefix(home) {
        return words(rest).filter(|w| !w.is_empty()).map(|w| w.join("/"));
    }
    let rest = folder.strip_prefix(home.parent()?).ok()?;
    words(rest)
        .filter(|w| !w.is_empty())
        .map(|w| format!("../{}", w.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A power cut while the config is saved: the old config was truncated in
    /// place, then loaded as the defaults, and the node forgot its vault. A
    /// save now replaces the file by name and never writes into the old one
    /// (a hard link to it still reads the old config), and a stale pending
    /// file left by an earlier crash is no obstacle.
    #[test]
    fn red_team_a_save_never_rewrites_the_old_config_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let named = |username: &str| Config {
            username: username.to_owned(),
            ..Config::default()
        };
        named("before").save(&path).unwrap();
        let old = dir.path().join("old");
        std::fs::hard_link(&path, &old).unwrap();
        std::fs::write(path.with_extension("new"), b"username = \"half").unwrap();

        named("after").save(&path).unwrap();

        assert_eq!(
            Config::load(&old).unwrap().username,
            "before",
            "the old config was rewritten in place"
        );
        assert_eq!(Config::load(&path).unwrap().username, "after");
        assert!(
            !path.with_extension("new").exists(),
            "the pending config was left behind"
        );
    }

    #[test]
    fn a_default_file_says_nothing_of_the_hosting_settings() {
        // An older version refuses a setting it does not know.
        let written = Config::default().render();
        assert!(
            !written.contains("contribute") && !written.contains("metered"),
            "{written}"
        );
    }

    #[test]
    fn a_config_round_trips() {
        let config = Config {
            username: "nicolas".to_owned(),
            pledge_bytes: 10 * 1024 * 1024 * 1024,
            split: Split::new(20, 80).expect("both parts are above zero"),
            listen: "127.0.0.1:9797".to_owned(),
            peers: vec!["pi.local:9797".to_owned(), "vm.local:9797".to_owned()],
            coordinator: None,
            coordinator_device: None,
            keep_bytes: Some(2 * 1024 * 1024 * 1024),
            keep_order: Order::Oldest,
            keep_only: vec!["Documents".to_owned(), "Photos/2026".to_owned()],
            folder: Some(PathBuf::from("/home/nicolas/ITSaNAS")),
            vault: Some(PathBuf::from("/mnt/nas/itsanas-vault")),
            announce: Some("ngas.fr:9801".to_owned()),
            // Not the default, so the round trip covers the line being written.
            updates: Updates::Auto,
            placement: PlacementMode::Measured,
            ..Config::default()
        };

        assert_eq!(Config::parse(&config.render()).unwrap(), config);
    }

    #[test]
    fn reserve_is_off_unless_the_file_says_on() {
        assert!(!Config::default().reserve);
        assert!(!Config::default().render().contains("reserve"));
        let on = Config::parse("reserve = on").unwrap();
        assert!(on.reserve && on.render().contains("reserve = on"));
        assert!(Config::parse("reserve = maybe").is_err());
    }

    #[test]
    fn placement_is_everyone_unless_the_file_says_measured() {
        // Everyone is today's behaviour; a node upgraded in place must keep it
        // until its owner chooses otherwise, and must not write a line an
        // older binary would refuse.
        assert_eq!(Config::default().placement, PlacementMode::Everyone);
        assert_eq!(
            Config::parse("").unwrap().placement,
            PlacementMode::Everyone
        );
        assert!(!Config::default().render().contains("placement"));
        let measured = Config::parse("placement = measured").unwrap();
        assert_eq!(measured.placement, PlacementMode::Measured);
        assert_eq!(
            Config::parse(&measured.render()).unwrap().placement,
            PlacementMode::Measured
        );
    }

    #[test]
    fn red_team_an_unknown_placement_is_refused_naming_the_two_choices() {
        // Read as everyone, a typo would leave a member believing their copies
        // were placed by measurement when nothing measured had been applied.
        let error = Config::parse("placement = smart").unwrap_err().to_string();
        assert!(error.contains("everyone"), "{error}");
        assert!(error.contains("measured"), "{error}");
        assert!(error.contains("line 1"), "{error}");
    }

    #[test]
    fn an_announced_address_survives_the_round_trip_through_the_file() {
        // It is written to the file by `itsanas announce` and read back by the
        // daemon on the next start. A setting that rendered and did not parse
        // would leave a node publishing its LAN address again after a restart,
        // and nothing would say so.
        let text = Config {
            username: "nicolas".to_owned(),
            announce: Some("ngas.fr:9801".to_owned()),
            ..Config::default()
        }
        .render();

        assert_eq!(
            Config::parse(&text).unwrap().announce.as_deref(),
            Some("ngas.fr:9801")
        );
    }

    #[test]
    fn red_team_an_announce_nobody_can_dial_is_refused_where_it_enters() {
        // Every one of these publishes an address to the whole account, and
        // each fails somewhere else: the unspecified ones tell a peer to dial
        // nothing, and the loopback ones tell every member to dial *itself*,
        // which authenticates against its own device id and reports the peer as
        // unreachable for a reason that names the wrong machine. The obvious
        // slip is the first: `listen` is `0.0.0.0:9797` on every node, and
        // copying it here is what a tired person does at midnight.
        for value in [
            "announce = 0.0.0.0:9797",
            "announce = [::]:9797",
            "announce = 127.0.0.1:9797",
            "announce = [::1]:9797",
            "announce = localhost:9797",
            "announce = ngas.fr",
            "announce = ngas.fr:0",
            "announce = ngas.fr:notaport",
            "announce = :9801",
            "announce = 2001:db8::1:9797",
        ] {
            let refused = Config::parse(value);
            assert!(
                refused.is_err(),
                concat!(
                    "{} would be published to every member of the account ",
                    "and cannot be dialled by any of them"
                ),
                value
            );
        }

        // And the ones that are the point of the setting.
        for value in [
            "announce = ngas.fr:9801",
            "announce = 82.67.35.234:9801",
            "announce = [2001:db8::1]:9797",
            "announce = pi.ngas.fr:9797",
        ] {
            assert!(
                Config::parse(value).is_ok(),
                "{value} is how a member in another house reaches this one"
            );
        }
    }

    #[test]
    fn a_split_in_the_config_file_overrides_the_default() {
        // The point of the field. Without this the split is a constant wearing a
        // struct, and the network cannot be tuned for the machines actually in
        // it -- a pool of phones needs a different one from a pool of servers,
        // and the argument for each number then lives in a commit message.
        let config = Config::parse("split = 25/75").unwrap();

        assert_eq!(config.split, Split::new(25, 75).unwrap());
        assert_ne!(config.split, Split::DEFAULT);

        // Absent, it is the network's default rather than nothing.
        assert_eq!(
            Config::parse("username = nicolas").unwrap().split,
            Split::DEFAULT
        );
    }

    #[test]
    fn red_team_an_impossible_split_in_the_config_file_is_refused_where_it_enters() {
        // `split = 30/0` divides by zero the first time somebody runs
        // `itsanas keep`, and the daemon is what usually holds the store open
        // when they do. Refused when the file is *read*, in the same breath as
        // the listen address, because a value that cannot work must never reach
        // the arithmetic that assumes it does.
        //
        // What this catches: the parser accepting the two numbers and leaving
        // the check to `Split::new`'s caller, which is how a validated type ends
        // up with an unvalidated door.
        for line in [
            "split = 30/0",
            "split = 0/70",
            "split = 30",
            "split = thirty/seventy",
        ] {
            let error = Config::parse(line).expect_err(&format!("{line:?} was accepted"));
            assert!(
                error.to_string().contains("30/70"),
                "the refusal of {line:?} does not show what a split looks like: {error}"
            );
        }
    }

    #[test]
    fn red_team_a_node_cannot_grant_itself_a_more_generous_split() {
        // Nothing on the network enforces the bargain yet: `assess` runs only in
        // tests and a host bounds itself, not owners. The one live check is
        // `itsanas keep` refusing a limit the pledge has not earned, and it reads
        // this field. Accepting `split = 1000/1` would let a member switch that
        // check off with a text editor, where before it took a rebuilt client.
        //
        // What this catches: the generosity check dropped from the parser, or
        // the comparison written as a division that rounds 31/69 down to 30/70.
        for line in [
            "split = 1000/1",
            "split = 1/1",
            "split = 31/69",
            "split = 301/700",
        ] {
            let error = Config::parse(line).expect_err(&format!("{line:?} was accepted"));
            assert!(
                error.to_string().contains("30/70"),
                "the refusal of {line:?} does not name the network's split: {error}"
            );
        }

        // Stricter is the case the field exists for, and equal is no change.
        assert_eq!(
            Config::parse("split = 25/75").unwrap().split,
            Split::new(25, 75).unwrap()
        );
        assert_eq!(
            Config::parse("split = 3/7").unwrap().split,
            Split::new(3, 7).unwrap()
        );
    }

    #[test]
    fn a_listen_address_nobody_can_bind_is_refused_when_the_file_is_read() {
        // The failure this prevents is not a bad error message. It is a systemd
        // unit with Restart=on-failure looping every thirty seconds because the
        // address it was told to bind is a hostname, with the explanation in a
        // journal nobody opens. Refuse where the value enters.
        let refused = Config::parse(
            "username = nicolas
listen = localhost:9797
",
        );
        assert!(
            refused.is_err(),
            concat!(
                "a config naming an address no socket can bind was accepted; ",
                "the node would start, fail at serve, and restart forever"
            )
        );

        // The control: the same file with a bindable address must load, or the
        // check above passes for the wrong reason.
        let accepted = Config::parse(
            "username = nicolas
listen = 0.0.0.0:9797
",
        )
        .expect("a bindable address must still load");
        assert_eq!(accepted.listen, "0.0.0.0:9797");
    }

    #[test]
    fn an_address_that_loads_is_stored_exactly_as_written() {
        // Validation must not rewrite the value. IPv6 has several spellings of
        // the same address and a node that publishes one form while its owner
        // reads another in the file has two answers to one question.
        let config = Config::parse(
            "listen = [::]:9797
",
        )
        .unwrap();
        assert_eq!(config.listen, "[::]:9797");
    }

    #[test]
    fn a_windows_folder_path_survives_the_round_trip() {
        // Backslashes and a drive letter must not be mangled by a format that
        // uses `=` as its only separator.
        let config = Config {
            folder: Some(PathBuf::from(r"C:\Users\SigSeg\ITSaNAS")),
            ..Config::default()
        };

        assert_eq!(
            Config::parse(&config.render()).unwrap().folder,
            config.folder
        );
    }

    /// A key copied on Windows at `E:` is `/Volumes/KEY` on a Mac: the
    /// folder beside a portable home is written relative to it, and found
    /// again wherever the drive is mounted. An installed home keeps its
    /// absolute folder.
    #[test]
    fn a_portable_homes_folder_moves_with_the_drive() {
        let first = tempfile::tempdir().expect("first mount");
        let home = first.path().join("ITSaNAS").join("home");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::write(home.join(PORTABLE_MARKER), "").expect("marker");
        let config = Config {
            folder: Some(first.path().join("ITSaNAS").join("Fichiers")),
            ..Config::default()
        };
        config.save(&home.join("config")).expect("save");
        let text = std::fs::read_to_string(home.join("config")).expect("read");
        assert!(text.contains("folder = ../Fichiers\n"), "{text}");

        let second = tempfile::tempdir().expect("second mount");
        let moved = second.path().join("ITSaNAS").join("home");
        std::fs::create_dir_all(&moved).expect("moved home");
        std::fs::write(moved.join("config"), &text).expect("copy");
        assert_eq!(
            Config::load(&moved.join("config")).expect("load").folder,
            Some(second.path().join("ITSaNAS").join("Fichiers"))
        );

        // Without the marker, the folder is written as it is.
        std::fs::remove_file(home.join(PORTABLE_MARKER)).expect("unmark");
        config.save(&home.join("config")).expect("save");
        assert_eq!(
            Config::load(&home.join("config")).expect("load").folder,
            config.folder
        );
        let text = std::fs::read_to_string(home.join("config")).expect("read");
        assert!(!text.contains(".."), "{text}");
    }

    #[test]
    fn no_folder_is_a_valid_configuration() {
        // A pure host offers space and holds other people's sealed data
        // without syncing a folder of its own.
        assert_eq!(Config::default().folder, None);
        assert_eq!(Config::parse("username = host-only").unwrap().folder, None);
    }

    #[test]
    fn an_unknown_setting_is_an_error_rather_than_being_ignored() {
        // A silently discarded typo is how a node ends up pledging nothing
        // while its operator believes it pledged a terabyte.
        let error = Config::parse("pledge_byte = 500").unwrap_err();
        assert!(
            error.to_string().contains("unknown setting"),
            "got: {error}"
        );
    }

    #[test]
    fn a_malformed_line_names_its_line_number() {
        let error = Config::parse("username = a\nthis is not a setting\n").unwrap_err();
        assert!(error.to_string().contains("line 2"), "got: {error}");
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let config = Config::parse("# a comment\n\n  \nusername = bob\n").unwrap();
        assert_eq!(config.username, "bob");
    }

    #[test]
    fn several_peers_accumulate() {
        let config = Config::parse("peer = a:1\npeer = b:2\npeer = c:3\n").unwrap();
        assert_eq!(config.peers, vec!["a:1", "b:2", "c:3"]);
    }

    #[test]
    fn a_missing_file_reads_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("absent.conf")).unwrap();
        assert_eq!(config, Config::default());
    }
}
