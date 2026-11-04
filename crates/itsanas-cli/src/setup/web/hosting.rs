//! The switch and the settings behind "Contribute to the network", in one form
//! for every platform. What a value means is `itsanas_policy`'s.

use super::json::field;
use itsanas_node::config::Config;
use itsanas_policy::Period;

const GB: u64 = 1 << 30;

/// The values as the page shows them.
pub(super) fn to_json(config: &Config) -> String {
    serde_json::json!({
        "contribute": config.contribute,
        "batteryFloor": config.battery_floor_percent,
        "chargingMinutes": config.every_charging_minutes,
        "meteredGb": config.metered_allowance_bytes / GB,
        "meteredPeriod": config.metered_period.name(),
    })
    .to_string()
}

/// Apply what the form says to `config`, all of it or none of it. A field left
/// out keeps its value.
pub(super) fn apply(config: &mut Config, form: &[(String, String)]) -> Result<(), String> {
    let mut next = config.clone();
    if let Some(value) = field(form, "contribute") {
        next.contribute = value == "on";
    }
    if let Some(value) = field(form, "batteryFloor") {
        next.battery_floor_percent = value
            .parse()
            .ok()
            .filter(|percent| *percent <= 100)
            .ok_or("Type a battery level from 0 to 100.")?;
    }
    if let Some(value) = field(form, "chargingMinutes") {
        next.every_charging_minutes = value
            .parse()
            .ok()
            .filter(|minutes| *minutes >= 1)
            .ok_or("Type a whole number of minutes, at least 1.")?;
    }
    if let Some(value) = field(form, "meteredGb") {
        next.metered_allowance_bytes = value
            .parse::<u64>()
            .ok()
            .and_then(|gb| gb.checked_mul(GB))
            .ok_or("Type a whole number of GB.")?;
    }
    if let Some(value) = field(form, "meteredPeriod") {
        next.metered_period = Period::parse(value).ok_or("Choose hour, day, week or month.")?;
    }
    *config = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// Switching off through the page keeps the space offered, and a form with
    /// one bad value changes nothing. Sabotage: assign to `config` as it goes.
    #[test]
    fn red_team_a_refused_form_changes_nothing_and_the_switch_keeps_the_space() {
        let mut config = Config {
            pledge_bytes: 20 * GB,
            ..Config::default()
        };
        apply(&mut config, &form(&[("contribute", "off")])).expect("applies");
        assert!(!config.contribute);
        assert_eq!(
            config.pledge_bytes,
            20 * GB,
            "the switch forgot the space offered"
        );

        let before = config.clone();
        let refused = apply(
            &mut config,
            &form(&[("contribute", "on"), ("batteryFloor", "140")]),
        );
        assert!(refused.is_err());
        assert_eq!(
            to_json(&config),
            to_json(&before),
            "half of a refused form was kept: the person was told nothing changed"
        );
    }

    #[test]
    fn the_form_reaches_every_setting() {
        let mut config = Config::default();
        apply(
            &mut config,
            &form(&[
                ("batteryFloor", "30"),
                ("chargingMinutes", "10"),
                ("meteredGb", "2"),
                ("meteredPeriod", "week"),
            ]),
        )
        .expect("applies");
        assert_eq!(config.battery_floor_percent, 30);
        assert_eq!(config.every_charging_minutes, 10);
        assert_eq!(config.metered_allowance_bytes, 2 * GB);
        assert_eq!(config.metered_period, Period::Week);
    }
}
