//! The closed list of capabilities a manifest may declare and an install may grant, by the exact
//! names of design 2.2 and R4.1, and the import [`Group`] each one is reached through.

use crate::ops::Group;
use serde::{Deserialize, Serialize};

macro_rules! capabilities {
    ($($variant:ident = $name:literal => $group:ident,)*) => {
        /// One capability. Serialized as its manifest name (`"nodes.read"`).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        pub enum Capability {
            $(#[serde(rename = $name)] $variant,)*
        }

        impl Capability {
            pub const ALL: &'static [Capability] = &[$(Capability::$variant,)*];

            /// `[self]`, borrowed from [`Self::ALL`] (declaration order is discriminant order).
            pub const fn alone(self) -> &'static [Capability] {
                core::slice::from_ref(&Self::ALL[self as usize])
            }

            /// The manifest name.
            pub const fn name(self) -> &'static str {
                match self { $(Capability::$variant => $name,)* }
            }

            /// The import group whose calls this capability can make.
            pub const fn group(self) -> Group {
                match self { $(Capability::$variant => Group::$group,)* }
            }

            pub fn from_name(name: &str) -> Option<Capability> {
                match name { $($name => Some(Capability::$variant),)* _ => None }
            }
        }
    };
}

capabilities! {
    Canvas = "canvas" => Canvas,
    Ui = "ui" => Ui,
    Events = "events" => Events,
    Navigate = "navigate" => Navigate,
    Log = "log" => Log,
    NodesRead = "nodes.read" => Nodes,
    NodesWrite = "nodes.write" => Nodes,
    MdEdit = "md.edit" => Md,
    FilesRead = "files.read" => Files,
    FilesWrite = "files.write" => Files,
    ChatRead = "chat.read" => Chat,
    ChatSend = "chat.send" => Chat,
    MembersList = "members.list" => Members,
    StorageUser = "storage.user" => Storage,
    StorageScope = "storage.scope" => Storage,
    StorageNode = "storage.node" => Storage,
    StorageBlob = "storage.blob" => Storage,
    StorageFiles = "storage.files" => Storage,
    StorageLocal = "storage.local" => Storage,
    NetHost = "net.host" => Net,
    NetHostPrivate = "net.host.private" => Net,
    NetServer = "net.server" => Net,
    IhttpServe = "ihttp.serve" => Ihttp,
    IhttpCall = "ihttp.call" => Ihttp,
    IhttpP2pServe = "ihttp.p2p.serve" => Ihttp,
    IhttpP2pCall = "ihttp.p2p.call" => Ihttp,
    ClipboardRead = "clipboard.read" => Clipboard,
    ClipboardWrite = "clipboard.write" => Clipboard,
    Notifications = "notifications" => Notify,
    Documents = "documents" => Documents,
    GroupTopicSubscribe = "group.topic.subscribe" => Topic,
    GroupTopicPublish = "group.topic.publish" => Topic,
}

impl Capability {
    /// Held by every install and declarable by every manifest without listing them: they reach
    /// nothing outside the app's own surface.
    pub const INTRINSIC: [Capability; 5] = [
        Capability::Canvas,
        Capability::Ui,
        Capability::Events,
        Capability::Navigate,
        Capability::Log,
    ];

    pub fn is_intrinsic(self) -> bool {
        Self::INTRINSIC.contains(&self)
    }

    /// The capabilities that open `group`: a module may import `group` only if its manifest
    /// declares at least one of them.
    pub fn opening(group: Group) -> impl Iterator<Item = Capability> {
        Self::ALL
            .iter()
            .copied()
            .filter(move |c| c.group() == group)
    }
}
