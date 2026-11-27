//! What the placement code may depend on: no coordinator crate.

use super::*;

/// The workspace crates that `start` pulls in through its normal (not dev,
/// not build) dependencies, itself included.
///
/// Read by dependency *name*, not by `path`, so every way of spelling one is
/// caught: `x = { path = .. }`, `x.workspace = true`, a `[dependencies.x]`
/// table, a `package = "x"` rename. A name with a directory under `crates/` is
/// a workspace crate and is walked in turn.
fn workspace_closure(crates: &std::path::Path, start: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![start.to_owned()];
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let manifest = std::fs::read_to_string(crates.join(&name).join("Cargo.toml"))
            .unwrap_or_else(|error| panic!("{name}/Cargo.toml: {error}"));
        let mut normal = false;
        for raw in manifest.lines() {
            let line: String = raw
                .split('#')
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            let mut names = Vec::new();
            if let Some(header) = line.strip_prefix('[') {
                let header = header.trim_matches(|c| c == '[' || c == ']');
                let parts: Vec<&str> = header.split('.').collect();
                let at = parts.iter().position(|part| *part == "dependencies");
                normal = at.is_some();
                // `[dependencies.x]`, `[target.'..'.dependencies.x]`.
                if let Some(at) = at
                    && let Some(table) = parts.get(at + 1)
                {
                    names.push((*table).to_owned());
                }
            } else if normal && let Some((key, value)) = line.split_once('=') {
                names.push(key.split('.').next().unwrap_or_default().to_owned());
                if let Some(rest) = value.split("package=").nth(1) {
                    names.push(
                        rest.trim_matches(|c| c == '"' || c == '\'')
                            .split(['"', '\''])
                            .next()
                            .unwrap_or_default()
                            .to_owned(),
                    );
                }
            }
            for found in names {
                let found = found.trim_matches(|c| c == '"' || c == '\'').to_owned();
                if crates.join(&found).join("Cargo.toml").is_file() {
                    todo.push(found);
                }
            }
        }
    }
    seen
}

#[test]
fn red_team_no_coordinator_crate_can_reach_the_roster_builder() {
    // §6: availability anyone else reports never reaches placement. The roster
    // is built in `Placement::build` from this node's store alone; that holds
    // only while no crate that builds or feeds it can name a coordinator type.
    // A dependency on `itsanas-coord` anywhere below `itsanas-net` would let a
    // coordinator's presence figures be turned into `Uptime` and ranked on.
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf();
    for root in ["itsanas-net", "itsanas-placement"] {
        let closure = workspace_closure(&crates, root);
        assert!(
            closure.contains("itsanas-placement") && closure.contains("itsanas-crypto"),
            "the manifest walk found nothing under {root} ({closure:?}): this test would pass blind"
        );
        for coordinator in ["itsanas-coord", "itsanas-coordinator"] {
            assert!(
                !closure.contains(coordinator),
                "{root} now depends on {coordinator} ({closure:?}): a coordinator's view of who is \
                 online could reach the roster and decide where copies go"
            );
        }
    }
}
