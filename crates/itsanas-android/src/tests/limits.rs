//! The limits a phone sets: what keep means, and what its disk can hold.

use super::*;

/// A negative `keep` is no limit and 0 is the listing alone: Kotlin always
/// sends bytes, so "nothing" and "everything" must not read the same. 0
/// survives the file, and a round then lists a file without fetching it.
/// Sabotage: `(keep > 0).then(..)`, as it was (0 reads as no limit, and a
/// person who asked for nothing here downloads everything).
#[test]
fn red_team_a_negative_keep_is_no_limit_and_zero_is_the_listing_alone() {
    with_directory(|address, _| {
        let dir = tempfile::tempdir().unwrap();
        let (nas, phrase) = phone(&dir.path().join("nas"), address, "camille", None);
        let source = dir.path().join("source");
        std::fs::write(&source, b"kept on the NAS").unwrap();
        put_file(&nas, "one.txt", source.to_str().unwrap()).unwrap();

        let (mut me, _) = phone(&dir.path().join("me"), address, "camille", Some(&phrase));
        set_limits_on(&mut me, 0, -1, None).unwrap();
        assert_eq!(
            me.config.keep_bytes, None,
            "a negative keep is not no limit"
        );

        set_limits_on(&mut me, 0, 0, None).unwrap();
        assert_eq!(me.config.keep_bytes, Some(0), "keep 0 read as no limit");
        let saved = itsanas_node::Config::load(&Node::config_path(&me.home)).unwrap();
        assert_eq!(saved.keep_bytes, Some(0), "keep 0 did not survive the file");
        assert!(!saved.keeping().is_everything());

        serving(&nas, |at| {
            me.config.peers.push(at.to_string());
            let said: serde_json::Value =
                serde_json::from_str(&sync_node(&me, false, NOW).unwrap()).unwrap();
            assert_eq!(said["reached"], 1, "{said}");
        });
        let contact = std::fs::read_to_string(me.home.join("last-contact")).unwrap();
        assert_eq!(
            contact,
            format!(" reached {NOW}"),
            "a round that reached a machine was not recorded: this phone goes on to say \
             it reached nobody"
        );
        let status: serde_json::Value = serde_json::from_str(&status_of(&me).unwrap()).unwrap();
        assert_eq!(status["notHere"], 1, "{status}");
        assert_eq!(
            me.store.read_file("one.txt").unwrap(),
            None,
            "a budget of nothing downloaded the file"
        );
    });
}

#[test]
fn red_team_the_phone_refuses_limits_its_disk_cannot_hold() {
    let dir = tempfile::tempdir().unwrap();
    let (mut node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
    let path = Node::config_path(&node.home);
    let before = std::fs::read(&path).unwrap();
    let free = Some(30 * GIB);

    let said = set_limits_on(&mut node, gib(31), -1, free)
        .expect_err("31 GiB promised on 30 free")
        .message();
    assert!(
        said.contains("Space you offer") && !said.contains("itsanas "),
        "{said}"
    );
    let said = set_limits_on(&mut node, gib(25), gib(10), free)
        .expect_err("35 GiB promised on 30 free")
        .message();
    assert!(
        said.contains("Space you offer")
            && said.contains("How much of your data")
            && !said.contains("itsanas "),
        "the phone was told something it cannot act on: {said}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused pair rewrote the node file"
    );
    assert_eq!(
        node.config.pledge_bytes, 0,
        "a refused pair changed the pledge"
    );

    // Promised beyond what is free, though the pair would fit: the files here
    // can be released to make room for a keep, not for somebody else's data.
    let source = dir.path().join("source");
    std::fs::write(&source, vec![7u8; 4096]).unwrap();
    put_file(&node, "one.bin", source.to_str().unwrap()).unwrap();
    let said = set_limits_on(&mut node, gib(30) + 1, -1, free)
        .expect_err("a byte more than is free was promised")
        .message();
    assert!(said.contains("at most"), "{said}");

    set_limits_on(&mut node, gib(20), gib(10), free).expect("exactly what the disk holds");
    set_limits_on(&mut node, gib(31), gib(10), None).expect("a disk that will not say");
}
