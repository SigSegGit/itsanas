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

use std::path::PathBuf;

use itsanas_coord::accounting::Split;
use itsanas_policy::keeping::{Keeping, Order};

mod file;
mod homes;
mod values;

pub use file::{PORTABLE_MARKER, parse_order};
pub use homes::{
    default_home, instance_home, instance_home_in, node_homes, other_user_home, stranded_node,
    unnamed_home, user_home,
};
pub use values::{format_size, parse_announce, parse_listen, parse_size, size_argument};

/// What the daemon does about a newer signed release (HANDOVER §8 0w (6)).
///
/// `Notify` by default: a member is told, and nothing on their machine changes
/// without them, until they choose `Auto`. A setting rather than a flag so the
/// daemon reads it again before every daily check and a change needs no
/// restart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Updates {
    /// Install it and restart, once a day.
    Auto,
    /// Log it and show "update available" in `itsanas status`.
    #[default]
    Notify,
    /// Never look.
    Off,
}

impl Updates {
    /// The word written in the configuration and in setup's answers.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Notify => "notify",
            Self::Off => "off",
        }
    }

    /// Parse `auto`, `notify` or `off`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "notify" => Some(Self::Notify),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// How this node chooses where its account's chunks go (placement spec §0.4).
///
/// `Everyone` by default, and it must stay exactly today's behaviour: every
/// reachable host is offered everything. `Measured` places by the availability
/// this device measured itself. Measurement and the `status` lines run in both,
/// so a member can read the figures before choosing to act on them -- turning
/// on a placement whose inputs nobody had seen would be trusting it blind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlacementMode {
    /// Offer every chunk to every host reached, as before placement existed.
    #[default]
    Everyone,
    /// Offer a host only the chunks whose target wants it.
    Measured,
}

impl PlacementMode {
    /// The word written in the configuration.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::Measured => "measured",
        }
    }

    /// Parse `everyone` or `measured`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "everyone" => Some(Self::Everyone),
            "measured" => Some(Self::Measured),
            _ => None,
        }
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

    /// Offer this machine to the network. Off, it declares nothing and takes in
    /// nothing, and `pledge_bytes` is kept for when it comes back on.
    pub contribute: bool,

    /// Battery percentage at or below which a phone works only when watched.
    pub battery_floor_percent: u8,

    /// Minutes between rounds while charging on a free connection.
    pub every_charging_minutes: u32,

    /// Bytes hosting may move over a metered connection per `metered_period`.
    pub metered_allowance_bytes: u64,

    /// What `metered_allowance_bytes` is counted over.
    pub metered_period: itsanas_policy::Period,
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
    /// The address to publish to the coordinator, when it is not where this
    /// node listens.
    ///
    /// A node behind a router publishes, by default, the address it reached the
    /// coordinator from -- which is its address *on the LAN it is on*, and
    /// means nothing to a member in another house. This is what to publish
    /// instead: the name or public address that reaches this machine from
    /// outside, `nas.example.org:9801` for a forwarded port or a global IPv6 address
    /// for a machine that needs no forward at all.
    ///
    /// The port is published as written and is **not** replaced by the listen
    /// port, because a forward exists precisely to map one to the other.
    ///
    /// Absent is right for a machine that moves -- a laptop, a phone. Those are
    /// not dialled from elsewhere and do not pretend to be; they dial out, and
    /// one reachable side per pair is enough (ROADMAP, mutual hosting over one
    /// outbound connection).
    pub announce: Option<String>,
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
    /// Where the vault (what this node keeps for others) lives, if not
    /// `<home>/vault`: another disk, a NAS (HANDOVER §8 0s). `None` keeps
    /// every existing node where it is.
    pub vault: Option<PathBuf>,
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
    /// What the daemon does about a newer release.
    pub updates: Updates,
    /// How the daemon places this account's chunks on other people's hosts.
    pub placement: PlacementMode,
    /// How available the hosts of this account's copies should be, as this
    /// machine measured them: tried first, never the only ones
    /// (`host_availability = 99`). Any level but `cheapest` places by measure.
    pub host_tier: itsanas_placement::HostTier,
    /// Hold the pledge on disk: a file the size of what is promised and not
    /// yet held, shrunk as chunks arrive ([`crate::pledges::reserve`]), so
    /// another program cannot take the space this machine promised. Off by
    /// default: it costs the space on day one (`reserve = on`).
    pub reserve: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            username: String::new(),
            pledge_bytes: DEFAULT_PLEDGE_BYTES,
            contribute: DEFAULT_HOSTING.contribute,
            battery_floor_percent: DEFAULT_HOSTING.battery_floor_percent,
            every_charging_minutes: DEFAULT_CHARGING_MINUTES,
            metered_allowance_bytes: DEFAULT_HOSTING.metered_allowance,
            metered_period: DEFAULT_HOSTING.metered_period,
            split: Split::DEFAULT,
            listen: DEFAULT_LISTEN.to_owned(),
            announce: None,
            peers: Vec::new(),
            coordinator: None,
            coordinator_device: None,
            folder: None,
            vault: None,
            keep_bytes: None,
            keep_order: Order::default(),
            keep_only: Vec::new(),
            updates: Updates::default(),
            placement: PlacementMode::default(),
            host_tier: itsanas_placement::HostTier::Cheapest,
            reserve: false,
        }
    }
}

/// The policy's own defaults, so a value has one home.
const DEFAULT_HOSTING: itsanas_policy::Settings = itsanas_policy::Settings::DEFAULT;

/// [`itsanas_policy::SERVICE`], in the file's unit.
const DEFAULT_CHARGING_MINUTES: u32 = 5;

impl Config {
    /// What this machine declares it offers: nothing while switched off, and
    /// `pledge_bytes` stays for when it comes back on.
    #[must_use]
    pub const fn offered_bytes(&self) -> u64 {
        if self.contribute {
            self.pledge_bytes
        } else {
            0
        }
    }

    /// These settings as the policy reads them. A machine offering no space has
    /// nothing to contribute, whatever the switch says.
    #[must_use]
    pub fn hosting_settings(&self, content_on_metered: bool) -> itsanas_policy::Settings {
        itsanas_policy::Settings {
            content_on_metered,
            contribute: self.offered_bytes() > 0,
            battery_floor_percent: self.battery_floor_percent,
            every_charging: std::time::Duration::from_secs(
                u64::from(self.every_charging_minutes) * 60,
            ),
            metered_allowance: self.metered_allowance_bytes,
            metered_period: self.metered_period,
            ..itsanas_policy::Settings::DEFAULT
        }
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Switched off and on, nothing is forgotten, and the values survive the
    /// file. Sabotage: make `offered_bytes` return `pledge_bytes`; zero the
    /// pledge when switching off; drop a line from the writer.
    #[test]
    fn red_team_switching_off_declares_nothing_and_forgets_nothing() {
        let mut config = Config {
            pledge_bytes: 20 << 30,
            contribute: false,
            battery_floor_percent: 30,
            every_charging_minutes: 10,
            metered_allowance_bytes: 1 << 30,
            metered_period: itsanas_policy::Period::Week,
            ..Config::default()
        };
        assert_eq!(
            config.offered_bytes(),
            0,
            "switched off, it still declared space"
        );
        assert!(!config.hosting_settings(false).contribute);
        let back = Config::parse(&config.render()).expect("parses");
        assert_eq!(
            back.pledge_bytes,
            20 << 30,
            "switching off forgot the space offered"
        );
        assert!(!back.contribute);
        assert_eq!(back.battery_floor_percent, 30);
        assert_eq!(back.every_charging_minutes, 10);
        assert_eq!(back.metered_allowance_bytes, 1 << 30);
        assert_eq!(back.metered_period, itsanas_policy::Period::Week);
        config.contribute = true;
        assert_eq!(
            config.offered_bytes(),
            20 << 30,
            "switched back on, the space did not return"
        );
        let settings = config.hosting_settings(true);
        assert_eq!(settings.every_charging.as_secs(), 600);
        assert_eq!(settings.metered_allowance, 1 << 30);
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
        assert_eq!(config.peers, [] as [std::string::String; 0]);
    }
}
