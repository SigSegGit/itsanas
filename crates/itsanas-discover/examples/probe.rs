//! A standalone beacon, for checking that discovery actually works on a real
//! network rather than only on a loopback socket in a test.
//!
//! Two ITSaNAS nodes cannot share the discovery port on one machine, so a
//! daemon and a listener cannot be run side by side to watch each other. This
//! sends instead: it announces a throwaway device on the real broadcast
//! address, which a daemon on this machine or any other on the same network
//! should report finding.
//!
//! ```text
//! cargo run -p itsanas-discover --example probe
//! cargo run -p itsanas-discover --example probe -- listen
//! ```
//!
//! `listen` binds the discovery port and prints what arrives, for use on a
//! machine where no daemon is running. It is a diagnostic, not a product
//! surface: the identity it announces is generated fresh each run and belongs
//! to nobody.

use std::time::Duration;

use itsanas_crypto::{DeviceKeys, ID_LEN, UserId};
use itsanas_discover::{DEFAULT_PORT, Lan};

/// First six bytes of a tag, for a diagnostic line.
fn hex_short(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(12);
    for byte in &bytes[..6] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn main() {
    let listen = std::env::args().nth(1).as_deref() == Some("listen");

    let opened = if listen {
        Lan::bind(DEFAULT_PORT)
    } else {
        // An ephemeral local port, but announcements still go to the discovery
        // port — otherwise a daemon holding it here would block the probe.
        Lan::announcer(DEFAULT_PORT)
    };

    let lan = match opened {
        Ok(lan) => lan,
        Err(error) => {
            eprintln!("could not bind the discovery port: {error}");
            eprintln!("if a daemon is running here, it already holds it — run this elsewhere");
            std::process::exit(1);
        }
    };

    if listen {
        println!("listening on udp {DEFAULT_PORT}. Ctrl-C to stop.");
        loop {
            match lan.receive(Duration::from_secs(5)) {
                Ok(Some((heard, from))) => println!(
                    "heard {} (owner {}) at {}:{}",
                    heard.device.short(),
                    hex_short(&heard.owner_tag),
                    from,
                    heard.port
                ),
                Ok(None) => {}
                Err(error) if error.is_foreign_traffic() => {}
                Err(error) => println!("refused a packet: {error}"),
            }
        }
    }

    let keys = DeviceKeys::generate().expect("a keypair");
    let owner = UserId::from_bytes([0xAB; ID_LEN]);
    println!(
        "announcing throwaway device {} as owner {} to {:?}",
        keys.device_id().short(),
        owner.short(),
        lan.targets()
    );

    for round in 1..=5 {
        match lan.announce(&keys, owner, 9797) {
            Ok(()) => println!("  sent {round}/5"),
            Err(error) => println!("  send {round}/5 failed: {error}"),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("done. A daemon on this network should have reported finding it.");
}
