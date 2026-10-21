//! `nodes` (tree, names, mdx of nodes in the install's scope) and `md` (the zone's own region).

use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    pub struct NodeInfo {
        pub node_id: String,
        pub name: String,
        pub parent_id: Option<String>,
        pub has_children: bool,
    }

    pub struct NodeList {
        pub nodes: Vec<NodeInfo>,
    }

    pub struct NodeGet {
        pub node_id: String,
    }

    pub struct NodeChildren {
        pub node_id: String,
    }

    /// The node's mdx content; reply `Text`.
    pub struct NodeReadMdx {
        pub node_id: String,
    }

    pub struct NodeCreate {
        pub parent_id: String,
        pub name: String,
    }

    pub struct NodeRename {
        pub node_id: String,
        pub name: String,
    }

    pub struct NodeMove {
        pub node_id: String,
        pub new_parent_id: String,
    }

    /// The zone's own region of the hosting document; reply `Text`.
    pub struct MdReadZone {}

    /// Replace the zone's own region. Never the required-apps header.
    pub struct MdReplaceZone {
        pub text: String,
    }

    pub struct MdAppend {
        pub text: String,
    }
}
