//! Probes: never at a private address, never for a device with no account,
//! and one an hour.

use super::*;

/// THE ATTACK: a coordinator that dials what a caller names is a port
/// scanner with somebody else's address on it. This one dials only what the
/// caller *announced*, which is signed by the caller's own device key --
/// and even then, only if it is an address the wider internet could reach.
/// Aiming it at a private range would scan the coordinator's own network,
/// which is the one network a member has no business reaching.
#[test]
fn red_team_a_probe_cannot_be_aimed_at_the_coordinators_own_network() {
    let directory = directory();
    let owner = user(7);
    let laptop = device(7);
    enrol(&directory, &owner, &laptop);

    let service = CoordService::new(&directory);

    for address in [
        "192.168.1.1:80",
        "10.0.0.5:22",
        "127.0.0.1:9797",
        "[::1]:9797",
        "169.254.169.254:80",
        "172.16.0.1:443",
        // A list with a private entry behind a public one (HANDOVER 0u).
        "82.67.35.234:9801,10.0.0.5:22",
        "82.67.35.234:9801,a,b,c,d",
    ] {
        announce_address(&directory, &owner, &laptop, address);
        let decided = service.probe_target(laptop.device_id(), NOW).unwrap();
        assert!(
            matches!(decided, Probe::Refuse(_)),
            "{address} would have been dialled by the coordinator: a member \
             can point that at the machine it runs on"
        );
    }

    // And the addresses the feature exists for.
    for address in [
        "ngas.fr:9801",
        "82.67.35.234:9801",
        "[2001:db8::1]:9797",
        "82.67.35.234:9801,[2001:db8::1]:9797",
    ] {
        announce_address(&directory, &owner, &laptop, address);
        assert_eq!(
            service.probe_target(laptop.device_id(), NOW).unwrap(),
            Probe::Address(address.to_owned()),
            "{address} is the case this exists for"
        );
    }
}

/// A probe is work the coordinator does for a member. A stranger who
/// completed a handshake is not a member: device keys are free keypairs,
/// and answering them would let anybody with a socket spend this
/// coordinator's outbound connections.
#[test]
fn red_team_a_device_with_no_account_cannot_make_the_coordinator_dial_anything() {
    let directory = directory();
    let service = CoordService::new(&directory);
    let stranger = device(99);

    let Probe::Refuse(why) = service.probe_target(stranger.device_id(), NOW).unwrap() else {
        panic!("an unenrolled device got the coordinator to dial on its behalf");
    };
    // The reason, not merely the refusal: an unenrolled device also has no
    // presence, so "there is nothing to probe" refuses it by accident. That
    // accident disappears the day anything else writes a presence, and the
    // rule this test is about would be gone with no test failing.
    assert!(
        why.contains("not enrolled"),
        concat!(
            "the refusal must be about enrolment rather than about a ",
            "missing address; it said {:?}"
        ),
        why
    );
}

/// A withdrawn machine keeps its key: withdrawal is the account saying it
/// no longer speaks for them. It must stop buying the coordinator's work
/// too, or a stolen laptop is a probe budget for as long as it runs.
#[test]
fn a_withdrawn_device_stops_being_probed_for() {
    let directory = directory();
    let owner = user(8);
    let laptop = device(8);
    enrol(&directory, &owner, &laptop);
    announce_address(&directory, &owner, &laptop, "ngas.fr:9801");

    let service = CoordService::new(&directory);
    assert!(matches!(
        service.probe_target(laptop.device_id(), NOW).unwrap(),
        Probe::Address(_)
    ));

    directory
        .claim(
            &NodeClaim {
                owner: owner.user_id(),
                device: laptop.device_id(),
                pledged_bytes: 0,
                issued_unix: NOW + 1,
                revoked: true,
            }
            .sign(&owner),
            NOW + 1,
        )
        .expect("withdraw");

    assert!(
        matches!(
            service.probe_target(laptop.device_id(), NOW + 2).unwrap(),
            Probe::Refuse(_)
        ),
        "a withdrawn device still spends this coordinator's connections"
    );
}

/// A device that has never announced has nothing to probe, and the
/// alternative -- letting it name an address -- is the scanner.
#[test]
fn a_device_that_never_announced_is_told_so_rather_than_probed() {
    let directory = directory();
    let owner = user(9);
    let laptop = device(9);
    enrol(&directory, &owner, &laptop);

    let service = CoordService::new(&directory);
    let Probe::Refuse(why) = service.probe_target(laptop.device_id(), NOW).unwrap() else {
        panic!("a device with no address was given a probe");
    };
    assert!(
        why.contains("announced"),
        "the refusal must say what is missing; it said {why:?}"
    );
}

/// THE BUDGET: one probe per device per hour. Without it a member's daemon
/// -- or a member's script -- turns every round into an outbound connection
/// the coordinator pays for, which is the load this whole design exists to
/// keep off it.
#[test]
fn red_team_asking_to_be_probed_again_and_again_buys_one_probe_an_hour() {
    let mut limiter = EscrowLimiter::with(1, PROBE_WINDOW, PROBE_TRACKED);
    let device = device(11).device_id().to_hex();
    let start = Instant::now();

    assert!(limiter.allow(&device, start), "the first ask is answered");
    for attempt in 0..50 {
        assert!(
            !limiter.allow(&device, start + Duration::from_secs(attempt)),
            "asking again within the hour bought another outbound connection"
        );
    }
    assert!(
        limiter.allow(&device, start + PROBE_WINDOW + Duration::from_secs(1)),
        "the window must reopen, or a machine that really did move can never \
         find out that it is reachable again"
    );
}
