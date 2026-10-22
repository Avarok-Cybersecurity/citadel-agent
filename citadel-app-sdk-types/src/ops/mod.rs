//! THE op table: every import group with its number, and every op with its number, argument
//! struct, reply type and the capabilities that admit it. Group numbers are stable indexes (the
//! TypeScript host reads them from `bindings/AbiConstants.ts`); op 0 is never assigned.

use crate::calls::*;
use crate::codec::{decode, encode, DecodeError};
use crate::Capability;
use alloc::vec::Vec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// One op: its argument struct is the request payload, `Reply` the `HostReply` payload.
pub trait HostOp: Serialize + DeserializeOwned {
    const GROUP: Group;
    const OP: i32;
    const NAME: &'static str;
    type Reply: Serialize + DeserializeOwned;
    /// The capabilities any ONE of which admits this call; never empty. Fixed per op, except the
    /// KV ops, whose backend decides. Only `IhttpConsumed` has two: credit flows back from
    /// whichever end is reading a body, a server reading a request or a client a response.
    fn granted_by(&self) -> &'static [Capability];
}

/// A row of [`OPS`]. `granted_by` is any-of, and `None` where the arguments decide it (the KV
/// ops).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpInfo {
    pub group: Group,
    pub op: i32,
    pub name: &'static str,
    pub granted_by: Option<&'static [Capability]>,
}

/// Why `(group, op, bytes)` is not a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallDecodeError {
    UnknownOp,
    Malformed(DecodeError),
}

#[macro_use]
mod macros;

groups! {
    Canvas = 0, "canvas", CanvasOps {
        CanvasMeasure = 1 -> CanvasSize, Canvas;
        CanvasDraw = 2 -> (), Canvas;
        MeasureText = 3 -> TextMetrics, Canvas;
    }
    Ui = 1, "ui", UiOps {
        SetToolbar = 1 -> (), Ui;
        OpenMenu = 2 -> (), Ui;
        ShowDialog = 3 -> (), Ui;
        ShowToast = 4 -> (), Ui;
        RequestHeight = 5 -> HeightGranted, Ui;
    }
    Events = 2, "events", EventsOps {
        Subscribe = 1 -> (), Events;
        RequestFrames = 2 -> (), Events;
    }
    Navigate = 3, "navigate", NavigateOps {
        NavigateOpen = 1 -> (), Navigate;
    }
    Nodes = 4, "nodes", NodesOps {
        NodeGet = 1 -> NodeInfo, NodesRead;
        NodeChildren = 2 -> NodeList, NodesRead;
        NodeReadMdx = 3 -> Text, NodesRead;
        NodeCreate = 4 -> NodeInfo, NodesWrite;
        NodeRename = 5 -> (), NodesWrite;
        NodeMove = 6 -> (), NodesWrite;
    }
    Md = 5, "md", MdOps {
        MdReadZone = 1 -> Text, MdEdit;
        MdReplaceZone = 2 -> (), MdEdit;
        MdAppend = 3 -> (), MdEdit;
    }
    Files = 6, "files", FilesOps {
        FilesList = 1 -> FileList, FilesRead;
        FilesRead = 2 -> BytesValue, FilesRead;
        FilesWrite = 3 -> (), FilesWrite;
        FilesDelete = 4 -> (), FilesWrite;
    }
    Chat = 7, "chat", ChatOps {
        ChatRecent = 1 -> ChatMessages, ChatRead;
        ChatSend = 2 -> (), ChatSend;
    }
    Members = 8, "members", MembersOps {
        MembersList = 1 -> MemberList, MembersList;
    }
    Storage = 9, "storage", StorageOps {
        KvGet = 1 -> KvValue, args;
        KvSet = 2 -> (), args;
        KvSetMany = 3 -> (), args;
        KvDelete = 4 -> (), args;
        KvList = 5 -> KvPage, args;
        BlobPut = 6 -> BlobRef, StorageBlob;
        BlobGet = 7 -> BytesValue, StorageBlob;
        BlobDelete = 8 -> (), StorageBlob;
        StoreFileWrite = 9 -> (), StorageFiles;
        StoreFileRead = 10 -> BytesValue, StorageFiles;
        StoreFileList = 11 -> FileList, StorageFiles;
    }
    Net = 10, "net", NetOps {
        HostFetch = 1 -> HttpResponse, NetHost;
        ServerFetch = 2 -> HttpResponse, NetServer;
        HostFetchOpen = 3 -> StreamHead, NetHost;
        NetStreamRead = 4 -> StreamChunk, NetHost;
        NetStreamClose = 5 -> (), NetHost;
    }
    Ihttp = 11, "ihttp", IhttpOps {
        IhttpServe = 1 -> (), IhttpServe;
        IhttpRespond = 2 -> (), IhttpServe;
        IhttpCall = 3 -> (), IhttpCall;
        IhttpConsumed = 4 -> (), [IhttpServe, IhttpCall];
    }
    Clipboard = 12, "clipboard", ClipboardOps {
        ClipboardReadText = 1 -> Text, ClipboardRead;
        ClipboardWriteText = 2 -> (), ClipboardWrite;
    }
    Notify = 13, "notify", NotifyOps {
        Notify = 1 -> (), Notifications;
    }
    Documents = 14, "documents", DocumentsOps {
        DocOpen = 1 -> DocState, Documents;
        DocApply = 2 -> (), Documents;
        DocPull = 3 -> DocUpdates, Documents;
    }
    Log = 15, "log", LogOps {
        LogWrite = 1 -> (), Log;
    }
}
