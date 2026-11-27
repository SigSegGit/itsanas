//! A round against real machines: who it dials, and what it enrols and reads.

use super::*;

/// `get` dials who `sync` dials: a file reached through a machine known only
/// from the coordinator, with nothing typed in. The same bug the round had, on
/// the path that opens a file. Sabotage: `fetch` reads `config.peers` alone
/// (it says "no machine configured" beside a running machine).
#[test]
fn red_team_a_file_is_fetched_from_a_machine_known_only_from_the_coordinator() {
    with_directory(|address, _| {
        let dir = tempfile::tempdir().unwrap();
        let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
        let source = dir.path().join("source");
        std::fs::write(&source, b"kept on the NAS").unwrap();
        put_file(&nas, "Photos/two.jpg", source.to_str().unwrap()).unwrap();
        let (me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));

        itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
        let server = itsanas_net::PeerServer::bind("127.0.0.1:0").unwrap();
        let at = server.local_addr().unwrap().to_string();
        nas.config.announce = Some(at.clone());
        itsanas_node::coordinator::announce(&nas, &at, itsanas_discover::now_unix()).unwrap();

        serving_on(&server, &nas, |_| {
            assert!(
                me.config.peers.is_empty(),
                "the fixture typed an address in"
            );
            let round: serde_json::Value =
                serde_json::from_str(&sync_node(&me, true, itsanas_discover::now_unix()).unwrap())
                    .unwrap();
            assert_eq!(round["reached"], 1, "{round}");
            assert_eq!(
                me.store.read_file("Photos/two.jpg").unwrap(),
                None,
                "a metadata round fetched the content, so the fetch proves nothing"
            );

            let out = dir.path().join("out");
            get_file(&me, "Photos/two.jpg", out.to_str().unwrap())
                .unwrap_or_else(|failure| panic!("not fetched: {}", failure.message()));
            assert_eq!(std::fs::read(&out).unwrap(), b"kept on the NAS");
        });
    });
}

/// A phone with no machine yet says so, and says nothing of an invitation it
/// was never asked for; a round still enrols it first, since a person has no
/// step for that, and reads what the account's machines pledge. Sabotage: drop
/// `ensure_enrolled` from `sync_node` (no slot, no name, nothing to read
/// pledges with); drop `refresh_others_pledged`.
#[test]
fn red_team_a_round_enrols_the_phone_and_reads_what_its_machines_pledge() {
    with_directory(|address, directory| {
        let dir = tempfile::tempdir().unwrap();
        let (mut nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
        nas.config.pledge_bytes = 100 * GIB;
        itsanas_node::coordinator::register_with(&nas, None, NOW).unwrap();
        let (me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));

        let said = sync_node(&me, true, NOW + 1)
            .expect_err("no machine has announced itself")
            .message();
        assert!(said.contains("no other machine"), "{said}");
        assert!(!said.contains("invitation"), "{said}");

        assert!(
            live(directory, me.store.device_id()),
            "a phone that ran a round never enrolled: its name is unclaimed"
        );
        assert_eq!(
            itsanas_node::coordinator::registered_with(&me.home),
            Some(address.to_string())
        );
        assert_eq!(
            Node::others_pledged(&me.home),
            100 * GIB,
            "the phone holds the account to 10 GiB however much the NAS lends"
        );
        let status: serde_json::Value = serde_json::from_str(&status_of(&me).unwrap()).unwrap();
        assert_eq!(status["enrolled"], true, "{status}");
        assert_eq!(status["heldForOthers"], 0, "{status}");
    });
}
