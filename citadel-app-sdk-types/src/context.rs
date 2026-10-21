//! What `app_init` receives: who, where and with what the app is running.

use crate::Capability;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    /// Where the install runs (design 2.5).
    #[serde(rename_all = "snake_case")]
    pub enum AppScope {
        Workspace,
        Node { node_id: String },
        NodeTree { node_id: String },
        Users { usernames: Vec<String> },
        SelfOnly { username: String },
    }

    #[serde(rename_all = "snake_case")]
    pub enum Surface {
        CanvasPane,
        MdZone { zone_id: String },
    }

    pub struct Size {
        pub width: u32,
        pub height: u32,
    }

    pub struct AppContext {
        pub app_id: String,
        pub install_id: String,
        pub scope: AppScope,
        /// The install's grant, sorted. A call outside it answers `ungranted`.
        pub granted: Vec<Capability>,
        pub surface: Surface,
        pub size: Size,
        pub dpr: f32,
        /// Theme token name -> CSS colour/value.
        pub theme: BTreeMap<String, String>,
        pub locale: String,
        pub acting_username: String,
        /// The node whose pane or zone hosts this instance.
        pub node_id: String,
    }
}

impl AppContext {
    pub fn is_granted(&self, cap: Capability) -> bool {
        self.granted.contains(&cap)
    }
}
