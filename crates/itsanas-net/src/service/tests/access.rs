//! Who a node serves at all: an away disk, an account's bound, the unscoped
//! owners, and which sessions record a network.

use super::*;

/// "Do you have these?" is answered from the index, never by reading
/// each chunk: a batch of 1024 addresses cost the host up to 256 MB of
/// disk for a 32 KB request, as often as any peer cared to ask.
/// Sabotage: answer with `self.chunk(..).is_none()` again.
#[test]
fn red_team_asking_whether_chunks_are_held_reads_none_of_them() {
    let host = node(&alice(), 1);
    let stranger = UserId::from_bytes([0x5E; 32]);
    let held: Vec<ChunkId> = (0..64u8).map(|n| ChunkId::from_bytes([n; 32])).collect();
    for address in &held {
        host.vault
            .put_chunk(stranger, address, &[7; 4096])
            .expect("hosted");
    }
    let absent = ChunkId::from_bytes([0xEE; 32]);
    let mut asked = held.clone();
    asked.push(absent);
    let before = host.vault.chunk_reads();
    let answer = service(&host)
        .handle_from_test(&Request::HaveChunks {
            owner: stranger,
            addresses: asked,
        })
        .expect("answered");
    assert_eq!(answer, Response::Missing(vec![absent]));
    assert_eq!(
        host.vault.chunk_reads(),
        before,
        "answering whether chunks are held read them whole: a cheap request a peer can repeat costs the host its disk"
    );
}

#[test]
fn red_team_only_a_direct_session_from_a_known_device_records_its_network() {
    // §8 7c. The network groups hosts behind one router; a relay's
    // address, a loopback or a stranger must group nobody.
    let host = node(&alice(), 1);
    let service = service(&host);
    let known = DeviceKeys::from_seed(&SecretBytes::new([2; 32])).device_id();
    let stranger = DeviceKeys::from_seed(&SecretBytes::new([3; 32])).device_id();
    let public: std::net::IpAddr = "203.0.113.7".parse().expect("address");
    host.store.note_seen(&known).expect("known");

    service.note_session(stranger, Some(public));
    assert_eq!(host.store.network_since(&stranger, 0).expect("read"), None);

    service.note_session(known, None);
    assert_eq!(
        host.store.network_since(&known, 0).expect("read"),
        None,
        "a relayed session recorded the relay's network as the caller's"
    );
    service.note_session(known, Some("127.0.0.1".parse().expect("address")));
    assert_eq!(host.store.network_since(&known, 0).expect("read"), None);

    service.note_session(known, Some(public));
    assert_eq!(
        host.store.network_since(&known, 0).expect("read"),
        itsanas_placement::Network::of(public)
    );
}

#[test]
fn red_team_an_away_node_serves_its_own_account_and_refuses_hosting_as_away() {
    // A USB vault unplugged: the node's own files keep syncing, and
    // every request about another account's data gets `HOSTING_AWAY`,
    // which an owner reads as "not here" -- never an audit "not held",
    // which would pause a host that only had its disk out.
    let node = node(&alice(), 10);
    let away = service(&node).hosting_away(true);
    let stranger = UserKeys::derive(&MasterSecret::from_bytes([0xB0; 32])).user_id();
    let address = ChunkId::from_bytes([3; 32]);
    let refused = Response::Refused(HOSTING_AWAY.to_owned());
    for request in [
        Request::Challenge {
            owner: stranger,
            address,
            nonce: [1; 32],
        },
        Request::Chunk {
            owner: stranger,
            address,
        },
        Request::HaveChunks {
            owner: stranger,
            addresses: vec![address],
        },
        Request::StoreChunk {
            owner: stranger,
            address,
            sealed: vec![0; 8],
        },
    ] {
        assert_eq!(
            away.handle_from_test(&request).unwrap(),
            refused,
            "{request:?}"
        );
    }
    assert_ne!(
        away.handle_from_test(&Request::Heads {
            owner: node.store.owner()
        })
        .unwrap(),
        refused,
        "an away node stopped serving its own account"
    );
    assert_ne!(
        service(&node)
            .handle_from_test(&Request::Chunk {
                owner: stranger,
                address
            })
            .unwrap(),
        refused,
        "a node with its disk in said it was away"
    );
}

#[test]
fn red_team_a_host_that_bounds_nobody_serves_no_read() {
    // The default is the safe side: a service built without an `Owners`
    // cannot tell an account's device from a stranger, so it reads to
    // nobody. Every request `read_owner` names is checked here.
    let node = node(&alice(), 10);
    let bare = PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1));
    let owner = node.store.owner();
    let address = ChunkId::from_bytes([3; 32]);
    for request in [
        Request::Heads { owner },
        Request::Segments {
            owner,
            device: node.store.device_id(),
            after: None,
            limit: 1,
        },
        Request::Chunk { owner, address },
        Request::HaveChunks {
            owner,
            addresses: vec![address],
        },
        Request::ChunkSummary { owner },
        Request::Challenge {
            owner,
            address,
            nonce: [1; 32],
        },
        Request::Claims { owner },
    ] {
        assert_eq!(
            read_owner(&request),
            Some(owner),
            "{request:?} is not classed as a read"
        );
        assert_eq!(
            bare.handle_from_test(&request).unwrap(),
            Response::Refused(NOT_YOUR_ACCOUNT.to_owned()),
            "{request:?} was served by a host that cannot tell who asks"
        );
    }
}

#[test]
fn red_team_no_node_serves_with_the_unscoped_owners() {
    // `Unscoped` exists for tests. A node built with it would serve every
    // account to anyone, so the code that runs one must never name it.
    // Each file is cut off at its `#[cfg(test)] mod tests`.
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut checked = 0;
    for krate in ["itsanas-cli", "itsanas-node", "itsanas-android"] {
        let mut stack = vec![crates.join(krate).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && !path.ends_with("tests.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    // Only the test module goes: a `#[cfg(test)]` helper
                    // higher up must not hide the code after it.
                    let code = text
                        .split("#[cfg(test)]\nmod tests")
                        .next()
                        .unwrap_or_default();
                    assert!(
                        !code.contains("Unscoped"),
                        "{} names `Unscoped` outside its tests: that node serves every \
                         account to anyone",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked > 20,
        "the scan found {checked} files: it would pass blind"
    );
}
