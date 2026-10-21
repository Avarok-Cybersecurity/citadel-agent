//! `nodes`: the tree under the install's scope.

use crate::{call, Pending};
use citadel_app_sdk_types::calls::*;

pub fn get(node_id: &str) -> Pending<NodeInfo> {
    call(&NodeGet {
        node_id: node_id.into(),
    })
}

pub fn children(node_id: &str) -> Pending<Vec<NodeInfo>> {
    call(&NodeChildren {
        node_id: node_id.into(),
    })
    .map(|l| l.nodes)
}

pub fn read_mdx(node_id: &str) -> Pending<String> {
    call(&NodeReadMdx {
        node_id: node_id.into(),
    })
    .map(|t| t.text)
}

pub fn create(parent_id: &str, name: &str) -> Pending<NodeInfo> {
    call(&NodeCreate {
        parent_id: parent_id.into(),
        name: name.into(),
    })
}

pub fn rename(node_id: &str, name: &str) -> Pending<()> {
    call(&NodeRename {
        node_id: node_id.into(),
        name: name.into(),
    })
}

pub fn move_to(node_id: &str, new_parent_id: &str) -> Pending<()> {
    call(&NodeMove {
        node_id: node_id.into(),
        new_parent_id: new_parent_id.into(),
    })
}

/// `md`: the hosting zone's own region of its document.
pub mod md {
    use super::*;

    pub fn read_zone() -> Pending<String> {
        call(&MdReadZone {}).map(|t| t.text)
    }

    pub fn replace_zone(text: &str) -> Pending<()> {
        call(&MdReplaceZone { text: text.into() })
    }

    pub fn append(text: &str) -> Pending<()> {
        call(&MdAppend { text: text.into() })
    }
}
