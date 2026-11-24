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
        /// A tab of the node's pane; `tab_id` names it among the node's tabs.
        NodeTab { tab_id: String },
        /// The whole page of a node. Empty, so a later field is additive.
        NodePage {},
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
        /// How long `app_close` and the replies it waits on may take before the host ends the
        /// instance: the manifest's `limits.close_grace_ms`, at most `abi::MAX_CLOSE_GRACE_MS`.
        pub close_grace_ms: u32,
        /// The host's ABI minor (`abi::ABI_MINOR` of the host's build). Absent from a host that
        /// predates minors, which is minor 0: that is what `default` decodes it as.
        #[serde(default)]
        pub sdk_abi_minor: u32,
    }
}

impl AppContext {
    pub fn is_granted(&self, cap: Capability) -> bool {
        self.granted.contains(&cap)
    }

    /// Whether the host paints the CW-5 canvas commands and delivers `AppEvent::Activate`.
    pub fn has_canvas_extension(&self) -> bool {
        self.sdk_abi_minor >= crate::abi::ABI_MINOR_CANVAS_EXT
    }
}
