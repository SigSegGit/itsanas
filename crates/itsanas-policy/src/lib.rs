//! When to sync, and how much of it.
//!
//! # Why this is a crate and not a corner of the Android app
//!
//! A laptop tethered to a phone should not upload forty gigabytes either. The
//! question "is this connection expensive, and is this a good moment" is not an
//! Android question; Android is simply the platform that forces you to answer
//! it. Answering it once, in testable Rust, means every shell gets the same
//! behaviour and the reasoning sits somewhere it can be argued with.
//!
//! It depends on nothing — not on the store, not on the network layer, not on a
//! platform crate. It is a decision table with an argument attached to every
//! row.
//!
//! # The distinction that matters is metered, not Wi-Fi
//!
//! The obvious rule is "Wi-Fi means free, mobile data means expensive". It is
//! wrong in both directions and both are common: a phone's own hotspot is
//! Wi-Fi and is charged by the gigabyte, while plenty of mobile plans are
//! unlimited. Android exposes the right question directly
//! (`NET_CAPABILITY_NOT_METERED`), macOS and Windows both expose it too, and
//! guessing from the interface type instead is how a sync tool ends up costing
//! somebody fifty euros.
//!
//! # Metadata and content are separate purchases
//!
//! The insight that makes a phone pleasant rather than frightening. Learning
//! *what* files exist means fetching signed log segments: kilobytes. Fetching
//! the files themselves is megabytes or gigabytes.
//!
//! So on an expensive connection this does the cheap half automatically and
//! leaves the expensive half to a deliberate act. That is the shape of the
//! behaviour people already expect from Drive and Dropbox, arrived at from the
//! cost rather than copied.
//!
//! **The other half of that behaviour is not built.** Fetching the segments
//! makes the *next* content round instant and lets this device relay work
//! onwards; it does not yet make the files appear in a listing, because a
//! deferred operation writes no index entry. "Everything is listed, tap one to
//! download it" needs a catalogue derived from the vault, and that does not
//! exist. See `docs/ROADMAP.md`.

#![forbid(unsafe_code)]

use core::time::Duration;

/// How much a connection costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    /// Nothing reachable. Local discovery may still find machines on the same
    /// network, but there is no route off it.
    None,
    /// Charged by the gigabyte, or capped. Mobile data, and a tethered hotspot.
    Metered,
    /// Home or office Wi-Fi, or ethernet.
    Unmetered,
}

/// What the battery allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// Below the point where the system starts refusing background work.
    Low,
    /// Running on battery, with room.
    OnBattery,
    /// Plugged in. The moment to do anything expensive.
    Charging,
}

/// Whether a person is looking at the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    /// Open and on screen. Somebody is waiting, and no background limits apply.
    Foreground,
    /// Not on screen. Every platform restricts what may happen here, and the
    /// restrictions tighten with each release.
    Background,
    /// Nobody is watching, and nothing is restricting either.
    ///
    /// A service: the Pi in the cupboard, the VM on the Freebox, `itsanas
    /// daemon` under systemd. It looks like [`Attention::Background`] and is
    /// not, and conflating the two is how this crate would have given a
    /// permanently-powered machine on ethernet a two-hour sync interval.
    ///
    /// That interval is not a considered choice about ethernet. It is the
    /// smallest number that survives Android's Doze and iOS's background
    /// budget. A machine subject to neither should not inherit it: in a
    /// household where the Pi is the always-on node, it is the difference
    /// between an edit reaching the other laptop in five minutes and in two
    /// hours.
    ///
    /// What this does **not** buy is permission to spend money. On a metered
    /// connection an unattended service behaves exactly like a background app,
    /// because the constraint there is the data plan and a data plan does not
    /// care what kind of process you are.
    Unattended,
}

impl Network {
    /// Every variant.
    ///
    /// The totality test walks this rather than a list written out at the call
    /// site. A list at the call site is one somebody forgets: `Unattended` was
    /// added to [`Attention`] and the test that claims to check "every
    /// combination" went on checking the two it already knew, silently. Adding
    /// a variant now fails to compile here, which is the only kind of reminder
    /// that works.
    pub const ALL: [Self; 3] = [Self::None, Self::Metered, Self::Unmetered];
}

impl Power {
    /// Every variant. See [`Network::ALL`].
    pub const ALL: [Self; 3] = [Self::Low, Self::OnBattery, Self::Charging];
}

impl Attention {
    /// Every variant. See [`Network::ALL`].
    pub const ALL: [Self; 3] = [Self::Foreground, Self::Background, Self::Unattended];
}

/// Everything the decision depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conditions {
    /// What the connection costs.
    pub network: Network,
    /// What the battery allows.
    pub power: Power,
    /// Whether somebody is looking.
    pub attention: Attention,
}

/// What a member has asked for, overriding the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Fetch and send file *contents* over a metered connection.
    ///
    /// Off by default, and that default is the whole point: a tool that
    /// silently spent somebody's data allowance would be uninstalled once and
    /// remembered for years. Listing files still happens — it costs kilobytes.
    pub content_on_metered: bool,

    /// Do anything at all while the application is not on screen.
    ///
    /// On by default, because a sync tool that only works while you watch it is
    /// not a sync tool. Off is for somebody who wants it strictly manual.
    pub background: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            content_on_metered: false,
            background: true,
        }
    }
}

/// How much of a sync to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Do not connect.
    Nothing,
    /// Exchange log segments and heads. Kilobytes.
    ///
    /// Enough to know that work is waiting, to relay it onwards, and to make
    /// the next content round resume rather than restart. Not yet enough to
    /// *show* the files: that needs a catalogue derived from the vault, which
    /// is not built.
    Metadata,
    /// Metadata and file contents. Megabytes to gigabytes.
    Everything,
}

impl Scope {
    /// Whether this scope moves file contents.
    #[must_use]
    pub const fn moves_content(self) -> bool {
        matches!(self, Self::Everything)
    }

    /// Whether this scope connects at all.
    #[must_use]
    pub const fn connects(self) -> bool {
        !matches!(self, Self::Nothing)
    }
}

/// What to do now, how often, and a sentence explaining it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// How much to move.
    pub scope: Scope,
    /// How often to repeat, or `None` for only when asked.
    pub every: Option<Duration>,
    /// Why, in words a person can read.
    ///
    /// Shown in the application. "Waiting for Wi-Fi" is the difference between
    /// a tool that seems broken and one that is plainly waiting for something,
    /// and it costs one field.
    pub because: &'static str,
}

/// How often to sync while somebody is watching, on a connection that is free.
///
/// Short enough to feel immediate without being a busy loop. No background
/// restriction applies here — the application is on screen — so the only limit
/// is politeness towards the battery and the peers.
pub const WATCHING: Duration = Duration::from_secs(30);

/// How often to sync in the background on a connection that is free.
///
/// Two hours. Comfortably above the fifteen-minute floor every mobile platform
/// imposes on periodic background work, and frequent enough that a laptop
/// picking up a phone's edits finds them within a working morning.
pub const BACKGROUND_FREE: Duration = Duration::from_secs(2 * 60 * 60);

/// How often to check the file list in the background on an expensive
/// connection.
///
/// Once a day, metadata only. Enough that the file list is never badly stale;
/// small enough that a month of it is not measurable on any data plan.
pub const BACKGROUND_METERED: Duration = Duration::from_secs(24 * 60 * 60);

/// How often an unattended service syncs on a connection that is free.
///
/// Five minutes. A machine that is plugged in and unrestricted has no reason to
/// wait two hours, and the number is chosen so that somebody who edits a file
/// on the laptop finds it on the Pi before they have finished wondering whether
/// it worked.
///
/// It is not shorter because each round is a TLS handshake and a have/missing
/// exchange per peer, and a household of four machines polling every thirty
/// seconds would spend more time greeting each other than syncing.
pub const SERVICE: Duration = Duration::from_secs(5 * 60);

/// Decide what to do.
///
/// Total: every combination of conditions produces a plan, and the plan always
/// carries a reason. There is no "unknown" state and nothing falls through to a
/// default, because a sync tool that silently does nothing is the failure this
/// whole crate exists to make impossible.
#[must_use]
pub fn plan(conditions: Conditions, settings: Settings) -> Plan {
    // Nothing reachable. Local discovery may still be finding machines on this
    // network, and that costs nothing and is handled elsewhere.
    if conditions.network == Network::None {
        return Plan {
            scope: Scope::Nothing,
            every: None,
            because: "no network",
        };
    }

    let watching = conditions.attention == Attention::Foreground;
    let service = conditions.attention == Attention::Unattended;

    // A person who opened the application is waiting, and they can see the
    // battery indicator themselves. Refusing to work while somebody watches is
    // how a tool gets a reputation for being broken.
    //
    // A service is not exempt: a laptop running the daemon on three per cent
    // ought to stop, and the machines this is aimed at are plugged in anyway.
    if !watching && conditions.power == Power::Low {
        return Plan {
            scope: Scope::Nothing,
            every: None,
            because: "battery is low",
        };
    }

    // The background switch is the mobile application's "don't work unless I'm
    // looking". Starting a daemon *is* the deliberate act that switch exists to
    // require, so it does not also have to be granted.
    if !watching && !service && !settings.background {
        return Plan {
            scope: Scope::Nothing,
            every: None,
            because: "background syncing is switched off",
        };
    }

    let metered = conditions.network == Network::Metered;
    let content_allowed = !metered || settings.content_on_metered;

    match (watching, metered) {
        // Watching, connection is free: as close to live as it gets.
        (true, false) => Plan {
            scope: Scope::Everything,
            every: Some(WATCHING),
            because: "open, on an unmetered connection",
        },

        // Watching, connection costs money. The list stays current for
        // kilobytes; a file downloads when it is asked for. Unless the member
        // has said they do not mind, in which case they do not mind.
        (true, true) if !content_allowed => Plan {
            scope: Scope::Metadata,
            every: Some(WATCHING),
            because: "open, but this connection is metered — tap a file to download it",
        },
        (true, true) => Plan {
            scope: Scope::Everything,
            every: Some(WATCHING),
            because: "open, and you have allowed downloads over metered connections",
        },

        // A service on a free connection: the fast beat. No platform is
        // restricting it and nobody is paying by the gigabyte, so the only
        // reason to wait is politeness towards the peers.
        (false, false) if service => Plan {
            scope: Scope::Everything,
            every: Some(SERVICE),
            because: "running as a service on an unmetered connection",
        },

        // Background, connection is free. Charging is the moment to do the
        // expensive thing, but not doing it on battery would mean a phone that
        // never syncs unless plugged in, which is not what anybody wants.
        (false, false) => Plan {
            scope: Scope::Everything,
            every: Some(BACKGROUND_FREE),
            because: if conditions.power == Power::Charging {
                "charging, on an unmetered connection"
            } else {
                "on an unmetered connection"
            },
        },

        // Background, connection costs money. Once a day, list only. This is
        // the row that decides whether somebody trusts the application on their
        // phone, so it is the most conservative one here.
        (false, true) if !content_allowed => Plan {
            scope: Scope::Metadata,
            every: Some(BACKGROUND_METERED),
            because: "metered connection — checking for changes only, once a day",
        },
        (false, true) => Plan {
            scope: Scope::Everything,
            every: Some(BACKGROUND_METERED),
            because: "metered connection, and you have allowed downloads over it",
        },
    }
}

/// Whether a member's explicit "sync now" should be honoured.
///
/// Almost always yes. A button that does nothing is worse than no button: it
/// teaches people the application is broken. The single exception is having no
/// network, where there is nothing to honour.
///
/// Note what is *not* an exception. A low battery does not block a deliberate
/// act, and neither does a metered connection: somebody who presses the button
/// on mobile data has decided, and second-guessing them is not this crate's
/// job.
#[must_use]
pub fn asked_for_it(conditions: Conditions) -> Plan {
    if conditions.network == Network::None {
        return Plan {
            scope: Scope::Nothing,
            every: None,
            because: "no network",
        };
    }
    Plan {
        scope: Scope::Everything,
        every: None,
        because: "you asked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(network: Network, power: Power, attention: Attention) -> Conditions {
        Conditions {
            network,
            power,
            attention,
        }
    }

    #[test]
    fn nothing_is_moved_over_a_metered_connection_unless_it_was_asked_for() {
        // The rule people actually care about. A tool that quietly spent a
        // data allowance is uninstalled once and remembered for years, and no
        // amount of correctness elsewhere buys that back.
        for power in [Power::Low, Power::OnBattery, Power::Charging] {
            for attention in [Attention::Foreground, Attention::Background] {
                let plan = plan(at(Network::Metered, power, attention), Settings::default());
                assert!(
                    !plan.scope.moves_content(),
                    "content moved over a metered connection: {power:?} {attention:?} -> {plan:?}"
                );
            }
        }
    }

    #[test]
    fn the_file_list_still_arrives_on_a_metered_connection() {
        // Refusing everything would be safe and useless: the phone would show
        // a stale list, which is indistinguishable from a broken application.
        // Log segments are kilobytes.
        let plan = plan(
            at(Network::Metered, Power::OnBattery, Attention::Foreground),
            Settings::default(),
        );
        assert_eq!(plan.scope, Scope::Metadata);
        assert!(plan.every.is_some());
    }

    #[test]
    fn allowing_metered_downloads_actually_allows_them() {
        // A setting that does nothing is worse than no setting.
        let settings = Settings {
            content_on_metered: true,
            ..Settings::default()
        };
        for attention in [Attention::Foreground, Attention::Background] {
            let plan = plan(at(Network::Metered, Power::OnBattery, attention), settings);
            assert_eq!(plan.scope, Scope::Everything, "{attention:?}");
        }
    }

    #[test]
    fn an_open_application_on_free_wifi_syncs_almost_live() {
        let plan = plan(
            at(Network::Unmetered, Power::OnBattery, Attention::Foreground),
            Settings::default(),
        );
        assert_eq!(plan.scope, Scope::Everything);
        assert_eq!(plan.every, Some(WATCHING));
    }

    #[test]
    fn a_low_battery_stops_background_work_but_never_stops_a_person() {
        // Somebody who opened the application is waiting and can see their own
        // battery indicator. Refusing to work while they watch is how a tool
        // gets a reputation for being broken.
        let background = plan(
            at(Network::Unmetered, Power::Low, Attention::Background),
            Settings::default(),
        );
        assert_eq!(background.scope, Scope::Nothing);

        let foreground = plan(
            at(Network::Unmetered, Power::Low, Attention::Foreground),
            Settings::default(),
        );
        assert!(foreground.scope.connects());
    }

    #[test]
    fn a_service_on_ethernet_does_not_inherit_a_phone_s_interval() {
        // The Pi in the cupboard and the VM on the Freebox are the always-on
        // machines a household syncs through. Two hours is not a considered
        // choice about ethernet; it is the smallest number that survives
        // Android Doze. Applying it to a permanently-powered machine would make
        // an edit on the laptop take up to two hours to reach the other laptop,
        // through a node that was awake the whole time.
        let service = plan(
            at(Network::Unmetered, Power::Charging, Attention::Unattended),
            Settings::default(),
        );
        assert_eq!(service.scope, Scope::Everything);
        assert_eq!(service.every, Some(SERVICE));
        assert!(
            SERVICE < BACKGROUND_FREE,
            "a service waits as long as a backgrounded phone, which is the \
             restriction it does not have"
        );
    }

    #[test]
    fn a_service_on_a_metered_link_is_no_less_careful_than_a_phone() {
        // Being a service buys freedom from the *platform*, not from the data
        // plan. A laptop tethered to a phone, running the daemon, must not
        // start uploading forty gigabytes because it is technically a service.
        let tethered = plan(
            at(Network::Metered, Power::Charging, Attention::Unattended),
            Settings::default(),
        );
        assert_eq!(tethered.scope, Scope::Metadata);
        assert_eq!(tethered.every, Some(BACKGROUND_METERED));
    }

    #[test]
    fn a_service_still_stops_when_the_battery_is_nearly_gone() {
        // A laptop running the daemon on three per cent should stop, exactly as
        // a backgrounded app would. Nothing about being a service makes the
        // battery bigger.
        let dying = plan(
            at(Network::Unmetered, Power::Low, Attention::Unattended),
            Settings::default(),
        );
        assert_eq!(dying.scope, Scope::Nothing);
    }

    #[test]
    fn switching_background_syncing_off_does_not_stop_a_daemon_somebody_started() {
        // That switch means "do not work unless I am looking at the app".
        // Running `itsanas daemon` is the deliberate act it exists to require,
        // so it does not also have to be granted. A daemon that silently did
        // nothing because of a phone setting would be a support case nobody
        // could diagnose.
        let started = plan(
            at(Network::Unmetered, Power::Charging, Attention::Unattended),
            Settings {
                background: false,
                ..Settings::default()
            },
        );
        assert_eq!(started.scope, Scope::Everything);

        // The same setting still silences a backgrounded application.
        let app = plan(
            at(Network::Unmetered, Power::Charging, Attention::Background),
            Settings {
                background: false,
                ..Settings::default()
            },
        );
        assert_eq!(app.scope, Scope::Nothing);
    }

    #[test]
    fn no_network_means_no_plan_and_no_button() {
        for attention in [Attention::Foreground, Attention::Background] {
            assert_eq!(
                plan(
                    at(Network::None, Power::Charging, attention),
                    Settings::default()
                )
                .scope,
                Scope::Nothing
            );
        }
        assert_eq!(
            asked_for_it(at(Network::None, Power::Charging, Attention::Foreground)).scope,
            Scope::Nothing
        );
    }

    #[test]
    fn the_button_works_even_when_the_schedule_would_not() {
        // A button that does nothing teaches people the application is broken.
        // Somebody pressing it on mobile data with a flat battery has decided,
        // and second-guessing them is not this crate's job.
        let hostile = at(Network::Metered, Power::Low, Attention::Foreground);
        assert_eq!(plan(hostile, Settings::default()).scope, Scope::Metadata);
        assert_eq!(asked_for_it(hostile).scope, Scope::Everything);
    }

    #[test]
    fn switching_background_syncing_off_leaves_the_foreground_alone() {
        let settings = Settings {
            background: false,
            ..Settings::default()
        };
        assert_eq!(
            plan(
                at(Network::Unmetered, Power::Charging, Attention::Background),
                settings
            )
            .scope,
            Scope::Nothing
        );
        assert!(
            plan(
                at(Network::Unmetered, Power::Charging, Attention::Foreground),
                settings
            )
            .scope
            .connects()
        );
    }

    #[test]
    fn every_combination_produces_a_plan_with_a_reason() {
        // Totality, asserted. A sync tool that silently does nothing in some
        // unconsidered corner is the failure this crate exists to prevent, and
        // "silently" is the operative word: every state has to be explainable
        // to the person looking at it.
        for network in Network::ALL {
            for power in Power::ALL {
                for attention in Attention::ALL {
                    for content_on_metered in [false, true] {
                        for background in [false, true] {
                            let plan = plan(
                                at(network, power, attention),
                                Settings {
                                    content_on_metered,
                                    background,
                                },
                            );
                            assert!(
                                !plan.because.is_empty(),
                                "{network:?} {power:?} {attention:?} had no reason"
                            );
                            assert_eq!(
                                plan.scope.connects(),
                                plan.every.is_some(),
                                "{network:?} {power:?} {attention:?}: a plan that connects needs \
                                 an interval, and one that does not must not have one"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn background_intervals_sit_above_every_platform_floor() {
        // Android, iOS and every desktop scheduler refuse periodic background
        // work more often than about a quarter of an hour. An interval below
        // that is not honoured — it is silently stretched, and the application
        // then behaves differently from what this crate says.
        let floor = Duration::from_secs(15 * 60);
        assert!(BACKGROUND_FREE >= floor);
        assert!(BACKGROUND_METERED >= floor);
        assert!(
            WATCHING < floor,
            "the foreground interval is not subject to that floor and should not be padded to it"
        );
    }

    #[test]
    fn a_day_of_metered_checking_is_not_measurable_on_a_data_plan() {
        // The claim made in this module's documentation, checked rather than
        // asserted in prose. One metadata round is a handful of signed log
        // segments; call it 64 KiB to be pessimistic.
        let rounds_per_month = 30 * 24 * 60 * 60 / BACKGROUND_METERED.as_secs();
        let bytes = rounds_per_month * 64 * 1024;
        assert!(
            bytes < 4 * 1024 * 1024,
            "{bytes} bytes a month of background checking on mobile data"
        );
    }
}
