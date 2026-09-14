//! Node configuration.
//!
//! A deliberately small `key = value` file rather than TOML or JSON. There is a
//! handful of settings; pulling in a configuration-language parser to read a
//! handful of settings adds a dependency tree to a security-sensitive binary in
//! exchange for nothing. The format is a strict subset of TOML's simplest form,
//! so if it ever grows enough to justify a real parser, existing files keep
//! working.
//!
//! Everything here is public, non-secret configuration. Secrets live in the
//! passphrase-sealed keystore and never appear in this file.

use std::{
    fmt::Write as _,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use itsanas_coord::accounting::Split;
use itsanas_policy::keeping::{Keeping, Order};

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

/// Default pledge when a node has not chosen one: nothing.
///
/// Hosting for other people is opt-in. A node that has not said how much space
/// it is offering has not offered any, and quietly assuming otherwise would
/// fill someone's disk on their behalf.
pub const DEFAULT_PLEDGE_BYTES: u64 = 0;

/// Default listen address: every interface.
///
/// Listening publicly by default is safe now that every connection is TLS with
/// both ends proving which device they are, and it is what a node in a network
/// has to do to be reachable. Before that it was not, and the default was
/// loopback with an explicit override.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:9797";

/// A node's non-secret settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Account name, as it would be registered with a coordinator.
    pub username: String,
    /// How much space this node offers to other users, in bytes.
    pub pledge_bytes: u64,
    /// How this machine's commitment divides between its owner and the network.
    ///
    /// Local, and only ever stricter than [`Split::DEFAULT`]. It decides what
    /// this node refuses to its own owner -- `itsanas keep` and `itsanas space`
    /// both ask it -- and nothing about what the network grants, which comes
    /// from the split the coordinator assesses with. A more generous split is
    /// refused when the file is read: `keep` is the only live enforcement of the
    /// bargain today, and it must not be one edited line from off.
    pub split: Split,
    /// Where to listen when serving.
    pub listen: String,
    /// Peers to sync with, as `host:port`.
    pub peers: Vec<String>,
    /// A coordinator to register with, announce to, and look peers up on.
    ///
    /// Optional, and a node without one is fully working: machines on the same
    /// network find each other with no server at all. What it adds is reaching
    /// a machine on a *different* network, and recovery from a passphrase.
    pub coordinator: Option<String>,

    /// The device the coordinator must prove itself to be, if known.
    ///
    /// An address is configuration, not a promise about who answers there. Pin
    /// this and a redirected address is refused rather than trusted.
    pub coordinator_device: Option<String>,

    /// The directory kept in step with the store, if one is configured.
    ///
    /// Optional on purpose: a node can be a pure host, offering space and
    /// holding other people's sealed data without syncing a folder of its own.
    pub folder: Option<PathBuf>,
    /// The most of this account's own data to keep on this device.
    ///
    /// `None` is "all of it", which is what every machine wanted before devices
    /// smaller than the account existed -- which is to say, before phones.
    ///
    /// This is not a cache eviction policy. Content that does not fit is simply
    /// never brought down: `itsanas_store::catalogue` lists every file the
    /// account has, marking the ones this device does not hold, and a client
    /// shows them and fetches on demand. Nothing is downloaded and then thrown
    /// away, which on a mobile connection is the difference between a setting
    /// and an insult.
    ///
    /// Separate from `pledge_bytes` on purpose, and the two are not
    /// interchangeable: this is room for *your* data, that is room you offer
    /// *others*. Having a terabyte free is not agreeing to lend a terabyte, and
    /// wanting to hold two gigabytes of your own says nothing about either.
    pub keep_bytes: Option<u64>,
    /// Which files matter most when the budget cannot hold them all.
    ///
    /// Without this the budget bounds the quantity and says nothing about the
    /// choice, so what a device ends up with is whatever the log replayed
    /// first -- the order operations were written, possibly by another machine,
    /// years ago. See `itsanas_policy::keeping`.
    pub keep_order: Order,
    /// Path prefixes this device restricts itself to. Empty means everything.
    pub keep_only: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            username: String::new(),
            pledge_bytes: DEFAULT_PLEDGE_BYTES,
            split: Split::DEFAULT,
            listen: DEFAULT_LISTEN.to_owned(),
            peers: Vec::new(),
            coordinator: None,
            coordinator_device: None,
            folder: None,
            keep_bytes: None,
            keep_order: Order::default(),
            keep_only: Vec::new(),
        }
    }
}

impl Config {
    /// What this device has been told to hold of its own account.
    ///
    /// The three settings that answer one question, gathered so that no caller
    /// has to remember they belong together — the budget without the order is
    /// how "keep two gigabytes" came to mean "keep the first two gigabytes the
    /// log mentions".
    #[must_use]
    pub fn keeping(&self) -> Keeping {
        Keeping {
            budget: self.keep_bytes,
            order: self.keep_order,
            only: self.keep_only.clone(),
        }
    }

    /// Render to the on-disk form.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# ITSaNAS node configuration.\n");
        out.push_str("# Secrets are not stored here; they live in the sealed keystore.\n\n");

        let _ = writeln!(out, "username = {}", self.username);
        let _ = writeln!(out, "pledge_bytes = {}", self.pledge_bytes);
        let _ = writeln!(out, "split = {}", self.split);
        let _ = writeln!(out, "listen = {}", self.listen);
        if let Some(keep) = self.keep_bytes {
            let _ = writeln!(out, "keep_bytes = {keep}");
        }
        if self.keep_bytes.is_some() || !self.keep_only.is_empty() {
            let _ = writeln!(out, "keep_order = {}", order_name(self.keep_order));
        }
        for only in &self.keep_only {
            let _ = writeln!(out, "keep_only = {only}");
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
                "coordinator" => config.coordinator = Some(value.to_owned()),
                "coordinator_device" => config.coordinator_device = Some(value.to_owned()),
                "peer" => peers.push(value.to_owned()),
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
                            "keep_only, ",
                            "listen, folder, peer, ",
                            "coordinator, coordinator_device"
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
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(NodeError::Io {
                path: path.to_owned(),
                source: error,
            }),
        }
    }

    /// Write to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.render()).map_err(|error| NodeError::Io {
            path: path.to_owned(),
            source: error,
        })
    }
}

/// Parse a human size such as `500M`, `2G`, `1TB`, or a plain byte count.
///
/// Nobody wants to type `10737418240`, and a config file full of raw byte counts
/// is a config file whose numbers nobody checks.
/// Check that a listen address is one a node could actually bind.
///
/// This is checked when the configuration is *read*, not when the socket is
/// opened. The difference matters on the machines this runs on: the daemon is
/// a systemd unit with `Restart=on-failure`, so a `listen` line that cannot be
/// parsed produces a unit that dies and restarts every thirty seconds forever,
/// with the reason in a journal the owner has no reason to open. Refusing at
/// load turns that into one message at the moment the file was edited.
///
/// A hostname is not accepted, and that is not an oversight: you bind an
/// address, not a name. `listen = localhost:9797` used to be stored happily
/// and then failed at `serve` with a parse error naming a line the reader had
/// not seen since.
///
/// # Errors
///
/// If `text` is not `host:port` with a literal IP address.
pub fn parse_listen(text: &str) -> Result<SocketAddr> {
    text.parse().map_err(|_| {
        NodeError::Config(format!(
            "listen must be an address and port such as 0.0.0.0:9797, found {text:?}"
        ))
    })
}

pub fn parse_size(text: &str) -> Result<u64> {
    let trimmed = text.trim();
    let digits_end = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());

    let (number, suffix) = trimmed.split_at(digits_end);
    if number.is_empty() {
        return Err(NodeError::Config(format!(
            "{text:?} does not start with a number"
        )));
    }

    let number: u64 = number
        .parse()
        .map_err(|_| NodeError::Config(format!("{text:?} is not a valid size")))?;

    let multiplier: u64 = match suffix.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1024,
        "M" | "MB" | "MIB" => 1024 * 1024,
        "G" | "GB" | "GIB" => 1024 * 1024 * 1024,
        "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
        other => {
            return Err(NodeError::Config(format!(
                "unknown size suffix {other:?}; use K, M, G or T"
            )));
        }
    };

    number
        .checked_mul(multiplier)
        .ok_or_else(|| NodeError::Config(format!("{text:?} overflows a 64-bit byte count")))
}

/// Render a byte count the way a person would read it.
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1024 * 1024 * 1024 * 1024, "TiB"),
        (1024 * 1024 * 1024, "GiB"),
        (1024 * 1024, "MiB"),
        (1024, "KiB"),
    ];

    for (threshold, unit) in UNITS {
        if bytes >= threshold {
            // One decimal place: enough to distinguish 1.2 GiB from 1.9 GiB,
            // few enough that the number stays readable in a status table.
            let whole = bytes / threshold;
            let tenths = (bytes % threshold) * 10 / threshold;
            return format!("{whole}.{tenths} {unit}");
        }
    }

    format!("{bytes} B")
}

/// Render a byte count as an argument `parse_size` reads back as no less.
///
/// For prices, not reports. [`format_size`] floors to a tenth, so the pledge
/// that buys 31 GiB at 30/70 -- 72.33 GiB -- reads "72.3 GiB", and somebody who
/// pledges what the refusal said is refused again with the same sentence. It
/// also writes a decimal and a space, which `parse_size` has never accepted, so
/// the command a refusal suggested could not be typed as shown. Rounded up to a
/// whole unit instead: "73G" costs at most a gigabyte more and always works.
///
/// Terabytes only when exact, because rounding a price up to the next terabyte
/// overcharges by up to a terabyte. Counts within a gigabyte of `u64::MAX` round
/// past what `parse_size` can hold; no pledge that size exists.
#[must_use]
pub fn size_argument(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if bytes >= TIB && bytes.is_multiple_of(TIB) {
        return format!("{}T", bytes / TIB);
    }
    for (unit, suffix) in [(GIB, "G"), (MIB, "M"), (KIB, "K")] {
        if bytes >= unit {
            return format!("{}{suffix}", bytes.div_ceil(unit));
        }
    }
    bytes.to_string()
}

/// Where a node keeps its state, if the user did not say.
pub fn default_home() -> PathBuf {
    // Deliberately not the OS config directory: this holds bulk data as well as
    // settings, and burying gigabytes of chunks in AppData or ~/.config would
    // surprise people and break backup tooling that treats those as small.
    std::env::var_os("ITSANAS_HOME").map_or_else(
        || {
            dirs_home()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".itsanas")
        },
        PathBuf::from,
    )
}

fn dirs_home() -> Option<PathBuf> {
    // Avoids a dependency for something this small. Both variables are set on
    // every platform this project targets.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };

        assert_eq!(Config::parse(&config.render()).unwrap(), config);
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

    #[test]
    fn no_folder_is_a_valid_configuration() {
        // A pure host offers space and holds other people's sealed data
        // without syncing a folder of its own.
        assert_eq!(Config::default().folder, None);
        assert_eq!(Config::parse("username = host-only").unwrap().folder, None);
    }

    #[test]
    fn defaults_are_safe() {
        // A node that has not said how much it offers has not offered any.
        // Assuming otherwise fills someone's disk on their behalf.
        let config = Config::default();
        assert_eq!(config.pledge_bytes, 0);
        // Listening publicly is safe because the transport authenticates both
        // ends; what must stay zero is what the node gives away.
        assert_eq!(config.pledge_bytes, 0);
        assert!(config.peers.is_empty());
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

    #[test]
    fn sizes_parse_the_way_people_write_them() {
        assert_eq!(parse_size("500").unwrap(), 500);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("2MB").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("10G").unwrap(), 10 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1TiB").unwrap(), 1024_u64.pow(4));
        assert_eq!(parse_size(" 3 g ").unwrap(), 3 * 1024 * 1024 * 1024);
    }

    #[test]
    fn a_nonsense_size_is_refused_rather_than_read_as_zero() {
        // Reading "ten gigabytes" as 0 would silently disable hosting.
        for bad in ["", "abc", "G", "10X", "-5"] {
            assert!(parse_size(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn an_overflowing_size_is_refused() {
        assert!(parse_size("999999999999T").is_err());
    }

    #[test]
    fn sizes_format_readably() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(10 * 1024 * 1024 * 1024), "10.0 GiB");
    }

    #[test]
    fn formatting_never_panics_at_the_extremes() {
        assert!(!format_size(u64::MAX).is_empty());
        assert!(!format_size(1).is_empty());
    }

    #[test]
    fn a_quoted_price_parses_back_to_no_less_than_the_price() {
        // A refusal names a pledge and a command to set it. If the figure is
        // floored, the person pledges exactly what they were told and is refused
        // again with the same sentence; if it does not parse, the command they
        // were handed fails before it reaches the bargain at all. Both happened:
        // `keep` suggested `--pledge 93.0 GiB`, which `parse_size` rejects, and
        // at 30/70 the floored figure is below the price at almost every size.
        const GIB: u64 = 1024 * 1024 * 1024;
        const TIB: u64 = 1024 * GIB;

        // The worked example the documents use, pinned so they can be checked.
        let price = Split::DEFAULT.pledge_needed_for(31 * GIB);
        assert_eq!(size_argument(price), "73G");

        for bytes in [
            0,
            1,
            1023,
            1024,
            1025,
            1536,
            GIB - 1,
            GIB,
            GIB + 1,
            price,
            TIB,
            TIB + 1,
            3 * TIB + GIB / 2,
        ] {
            let written = size_argument(bytes);
            let read = parse_size(&written)
                .unwrap_or_else(|error| panic!("{bytes} was quoted as {written:?}: {error}"));
            assert!(
                read >= bytes,
                "{bytes} was quoted as {written:?}, which pledges only {read}"
            );
        }

        // Exact sizes stay exact rather than being rounded into a larger unit.
        assert_eq!(size_argument(TIB), "1T");
        assert_eq!(size_argument(31 * GIB), "31G");
    }
}
