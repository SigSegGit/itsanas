//! The values a person types: a listen address, an announced address, a size
//! in bytes, and a size written back the way it was read.

use std::net::SocketAddr;

use crate::error::{NodeError, Result};

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

/// Check an address a node wants published, and return it unchanged.
///
/// Unlike [`parse_listen`] a **name is the point**: a home's public IPv4
/// address changes when the router reconnects, and a name is the only thing
/// that survives it. So this validates the shape and cannot resolve anything --
/// a name that does not resolve yet is a name whose DNS is not set up yet, and
/// refusing it here would be refusing to prepare a machine before its record
/// exists.
///
/// What it does refuse is an address that is a mistake in every network:
///
/// * **Unspecified** (`0.0.0.0`, `[::]`). Every node's `listen` is `0.0.0.0` by
///   default, so copying it into `announce` is the obvious slip, and it
///   publishes "dial nothing" to the whole account.
/// * **Loopback**. It resolves on every machine, so a member who received it
///   dials *itself*, authenticates against its own device id, and reports the
///   peer as unreachable-for-another-reason for ever.
/// * **No port**, or port 0. The coordinator hands the string to a dialler
///   verbatim; a missing port fails at each of them instead of here.
///
/// # Errors
///
/// If `text` is not a plausible `host:port` reachable from another network.
pub fn parse_announce(text: &str) -> Result<String> {
    let text = text.trim();

    let bad = |why: &str| {
        // `concat!` rather than a continuation: a backslash continuation in
        // this repository has twice reached the tree with the indentation of
        // the next line inside the message. It cannot capture `why` and `text`
        // implicitly, hence the explicit arguments.
        NodeError::Config(format!(
            concat!(
                "announce must be the host:port that reaches this machine from ",
                "another network, such as nas.example.org:9801 or [2001:db8::1]:9797; ",
                "{}, found {:?}"
            ),
            why, text
        ))
    };

    if text.len() > itsanas_coord::claim::MAX_ADDRESS_LEN {
        return Err(bad("that is longer than an address may be"));
    }

    // An IP literal parses whole -- an IPv6 one bracketed, which is the same
    // spelling a dialler needs. Anything else is a name, and of a name only the
    // port can be checked here.
    let port = if let Ok(address) = text.parse::<SocketAddr>() {
        if address.ip().is_unspecified() {
            return Err(bad("an unspecified address is not somewhere to dial"));
        }
        if address.ip().is_loopback() {
            return Err(bad(
                "loopback resolves on every machine, so every member would dial itself",
            ));
        }
        address.port()
    } else {
        let Some((host, port)) = text.rsplit_once(':') else {
            return Err(bad("there is no port"));
        };
        if host.is_empty() {
            return Err(bad("there is no host"));
        }
        // A colon or a bracket left in the host means an IPv6 literal that
        // failed to parse above: unbracketed, or malformed.
        if host.contains([':', '[', ']', '/', ' ']) {
            return Err(bad("that is not a name or a bracketed IPv6 address"));
        }
        if host.eq_ignore_ascii_case("localhost") {
            return Err(bad(
                "localhost resolves on every machine, so every member would dial itself",
            ));
        }
        port.parse::<u16>()
            .map_err(|_| bad("the port is not a number"))?
    };

    if port == 0 {
        return Err(bad("port 0 is not a port anybody can dial"));
    }

    Ok(text.to_owned())
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

#[cfg(test)]
mod tests {
    use itsanas_coord::accounting::Split;

    use super::*;

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
        assert_ne!(format_size(u64::MAX), "");
        assert_ne!(format_size(1), "");
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
