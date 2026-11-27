//! Where a node's vault lives, and refusing to open one that is not this
//! node's -- or a stand-in while its disk is away.

use std::path::{Path, PathBuf};

use itsanas_store::Vault;

use super::{AWAY_VAULT, Node, VAULT_MARKER, same_volume};
use crate::{
    config::Config,
    error::{NodeError, Result},
};

impl Node {
    /// A fresh, empty vault under the home for a node whose own is away.
    pub(super) fn stand_in_vault(home: &Path, device: &itsanas_crypto::DeviceId) -> Result<Vault> {
        let path = home.join(AWAY_VAULT);
        let io = |source| NodeError::Io {
            path: path.clone(),
            source,
        };
        match std::fs::remove_dir_all(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(io(error)),
            _ => {}
        }
        std::fs::create_dir_all(&path).map_err(io)?;
        std::fs::write(path.join(VAULT_MARKER), device.to_hex()).map_err(io)?;
        Ok(Vault::open(&path)?)
    }

    /// Where this node's vault lives: `config.vault`, or `<home>/vault`.
    #[must_use]
    pub fn vault_path(home: &Path, config: &Config) -> PathBuf {
        config.vault.clone().unwrap_or_else(|| home.join("vault"))
    }

    /// Whether `a` and `b` are on one disk; `None` when that cannot be told.
    #[must_use]
    pub fn same_disk(a: &Path, b: &Path) -> Option<bool> {
        same_volume(a, b)
    }

    /// This node's vault directory (see [`Self::vault_path`]).
    #[must_use]
    pub fn vault_dir(&self) -> PathBuf {
        Self::vault_path(&self.home, &self.config)
    }

    /// Open the vault, refusing one that is not this node's.
    ///
    /// `Vault::open` creates what is missing, so a vault on a disk that is not
    /// mounted would come back empty on the local disk: every chunk this node
    /// hosts would look lost, audits would fail and the owners would
    /// re-replicate everything (HANDOVER §8 0s). So a *configured* vault must
    /// already exist and carry the marker [`VAULT_MARKER`] naming this device;
    /// the default `<home>/vault` of a node that predates the marker gets one.
    ///
    /// # Errors
    ///
    /// A configured vault missing, unmarked or another device's; the store's.
    pub fn open_vault(
        home: &Path,
        config: &Config,
        device: &itsanas_crypto::DeviceId,
    ) -> Result<Vault> {
        let path = Self::vault_path(home, config);
        let marker = path.join(VAULT_MARKER);
        let mine = device.to_hex();
        match std::fs::read_to_string(&marker) {
            Ok(text) if text.trim() == mine => {
                // A marker without its database is a partial copy or restore:
                // opening it would start an empty index over whatever blobs
                // survived, and every hosted chunk would look lost.
                if config.vault.is_some() && !path.join("vault.redb").is_file() {
                    return Err(NodeError::Config(format!(
                        "the vault at {} has its marker but no vault.redb: a partial copy or \
                         restore? Refusing to start an empty index over it",
                        path.display()
                    )));
                }
            }
            Ok(_) => {
                return Err(NodeError::Config(format!(
                    "the vault at {} belongs to another device; refusing to use it",
                    path.display()
                )));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(NodeError::Io {
                    path: marker.clone(),
                    source: error,
                });
            }
            Err(_) if config.vault.is_some() => {
                return Err(NodeError::VaultAbsent(path));
            }
            Err(_) => {
                std::fs::create_dir_all(&path).map_err(|source| NodeError::Io {
                    path: path.clone(),
                    source,
                })?;
                std::fs::write(&marker, &mine).map_err(|source| NodeError::Io {
                    path: marker.clone(),
                    source,
                })?;
            }
        }
        Ok(Vault::open(&path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::tests::PASSPHRASE;

    #[test]
    fn red_team_a_vault_on_a_dropped_disk_is_refused_never_recreated_empty() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let mount = dir.path().join("nas");
        let (node, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        let mut config = node.config.clone();
        let device = node.store.device_id().to_hex();
        drop(node);
        config.vault = Some(mount.join("vault"));
        config.save(&Node::config_path(&home)).unwrap();

        // The mount dropped: the directory is missing, then empty.
        assert!(
            Node::open(&home, PASSPHRASE).is_err(),
            "a configured vault that is not there was accepted"
        );
        assert!(
            !mount.join("vault").exists(),
            "an empty vault was created where the disk should have been: every hosted chunk \
             would look lost and the owners would re-replicate all of it"
        );
        std::fs::create_dir_all(mount.join("vault")).unwrap();
        assert!(
            Node::open(&home, PASSPHRASE).is_err(),
            "an empty, unmarked directory was taken"
        );
        assert!(
            !mount.join("vault").join("vault.redb").exists(),
            "a fresh vault was written into the empty mount point"
        );

        // A partial copy: this device's marker, no index.
        std::fs::write(mount.join("vault").join(VAULT_MARKER), &device).unwrap();
        assert!(
            Node::open(&home, PASSPHRASE).is_err(),
            "a marker without vault.redb was opened: an empty index over surviving blobs"
        );
        std::fs::write(mount.join("vault").join(VAULT_MARKER), "not-this-device").unwrap();
        assert!(
            Node::open(&home, PASSPHRASE).is_err(),
            "another device's vault was opened as this node's"
        );
    }

    #[test]
    fn an_existing_node_keeps_its_vault_where_it_is_and_gains_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("node");
        let (node, _phrase) = Node::create(&home, PASSPHRASE, "nicolas").unwrap();
        let device = node.store.device_id().to_hex();
        drop(node);
        // Every node built before the marker existed.
        std::fs::remove_file(home.join("vault").join(VAULT_MARKER)).unwrap();
        let reopened = Node::open(&home, PASSPHRASE).expect("an old node must still open");
        assert_eq!(reopened.vault_dir(), home.join("vault"));
        assert_eq!(
            std::fs::read_to_string(home.join("vault").join(VAULT_MARKER)).unwrap(),
            device,
            "the default vault did not gain its marker"
        );
    }
}
