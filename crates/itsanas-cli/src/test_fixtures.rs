//! What the unit tests of several command modules share.

use itsanas_crypto::DeviceId;

pub(crate) fn device(byte: u8) -> DeviceId {
    DeviceId::from_bytes([byte; 32])
}

/// A node home as `instances` sees one: a keystore and a config, no keys.
pub(crate) fn fake_node(home: &std::path::Path, username: &str, folder: Option<&std::path::Path>) {
    std::fs::create_dir_all(home).expect("home");
    std::fs::write(home.join("keystore.bin"), b"sealed").expect("keystore");
    let config = itsanas_node::config::Config {
        username: username.to_owned(),
        folder: folder.map(std::path::Path::to_path_buf),
        ..itsanas_node::config::Config::default()
    };
    config
        .save(&itsanas_node::Node::config_path(home))
        .expect("config");
}
