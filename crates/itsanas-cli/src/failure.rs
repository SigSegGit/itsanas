//! What a call into the library can fail at, in the words a person sees.
//!
//! Every shell asks the same questions of the same code (the page, the phone's
//! few remaining native calls), so the failure and its sentences are written
//! once, here.

use itsanas_node::NodeError;

/// Anything an entry point can fail at, in the words the caller will see.
#[derive(Debug)]
pub enum Failure {
    /// The node is not open. Distinct from every other failure because it is
    /// the one a shell can fix by itself.
    Closed,
    /// Something the core refused, phrased for a person.
    Node(NodeError),
    /// The shell asked for something impossible.
    Usage(String),
}

impl From<NodeError> for Failure {
    fn from(error: NodeError) -> Self {
        Self::Node(error)
    }
}

impl From<itsanas_store::StoreError> for Failure {
    fn from(error: itsanas_store::StoreError) -> Self {
        Self::Node(NodeError::Store(error))
    }
}

impl From<itsanas_net::NetError> for Failure {
    fn from(error: itsanas_net::NetError) -> Self {
        Self::Node(NodeError::Net(error))
    }
}

/// The Settings section where this phone says how much it offers, and the one
/// where it says how much it keeps. A refusal names the place to act, since the
/// commands the core's own sentences name do not exist here.
pub const PLEDGE_SECTION: &str = "Space you offer other people";

pub const KEEP_SECTION: &str = "How much of your data to keep here";

impl Failure {
    #[must_use]
    pub fn message(&self) -> String {
        use itsanas_node::config::format_size;
        use itsanas_store::StoreError;
        match self {
            Self::Closed => "no account is open on this device".to_owned(),
            Self::Node(NodeError::Store(StoreError::DiskFull {
                path,
                incoming,
                room,
            })) => format!(
                "{path} needs at least {} on this phone, and only {} are free once the space it \
                 offers other people is set aside. Nothing was stored. Free some room, or lower \
                 \"{PLEDGE_SECTION}\" in Settings.",
                format_size(*incoming),
                format_size(*room),
            ),
            Self::Node(NodeError::Store(StoreError::OverBudget {
                path,
                total,
                allowed,
                ..
            })) => format!(
                "{path} would bring this account to at least {}, over the {} it may hold. \
                 Nothing was stored. Raising \"{PLEDGE_SECTION}\" in Settings, here or on \
                 another of your machines, earns it more.",
                format_size(*total),
                format_size(*allowed),
            ),
            Self::Node(error) => error.to_string(),
            Self::Usage(what) => what.clone(),
        }
    }
}

pub type Answer = Result<String, Failure>;
