//! The small groups.

macro_rules! wrappers {
    ($($module:ident { $($(#[$doc:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $op:expr, $map:expr;)* })*) => {$(
        pub mod $module {
            #[allow(unused_imports)]
            use citadel_app_sdk_types::calls::*;
            #[allow(unused_imports)]
            use citadel_app_sdk_types::Bytes;
            use crate::Pending;

            $(
                $(#[$doc])*
                pub fn $name($($arg: $ty),*) -> Pending<$ret> {
                    crate::call(&$op).map($map)
                }
            )*
        }
    )*};
}

wrappers! {
    events {
        /// Which input events to receive; none until asked.
        fn subscribe(kinds: Vec<EventKind>) -> () = Subscribe { kinds }, |r| r;
        fn request_frames(on: bool) -> () = RequestFrames { on }, |r| r;
    }
    navigate {
        /// Internal links only; the host refuses anything else.
        fn open(href: &str) -> () = NavigateOpen { href: href.into() }, |r| r;
    }
    files {
        fn list(path: &str) -> Vec<FileEntry> = FilesList { path: path.into() }, |l: FileList| l.entries;
        fn read(path: &str, offset: u64, len: u32) -> Vec<u8> = FilesRead { path: path.into(), offset, len }, |b: BytesValue| b.bytes.0;
        fn write(path: &str, bytes: &[u8]) -> () = FilesWrite { path: path.into(), bytes: Bytes(bytes.to_vec()) }, |r| r;
        fn delete(path: &str) -> () = FilesDelete { path: path.into() }, |r| r;
    }
    chat {
        fn recent(limit: u32) -> Vec<ChatMessage> = ChatRecent { limit }, |m: ChatMessages| m.messages;
        fn send(text: &str) -> () = ChatSend { text: text.into() }, |r| r;
    }
    members {
        fn list() -> Vec<Member> = MembersList {}, |l: MemberList| l.members;
    }
    clipboard {
        fn read_text() -> String = ClipboardReadText {}, |t: Text| t.text;
        fn write_text(text: &str) -> () = ClipboardWriteText { text: text.into() }, |r| r;
    }
    notify {
        fn notify(title: &str, body: &str) -> () = Notify { title: title.into(), body: body.into() }, |r| r;
    }
    documents {
        fn open(node_id: &str) -> Vec<u8> = DocOpen { node_id: node_id.into() }, |s: DocState| s.state.0;
        fn apply(node_id: &str, update: &[u8]) -> () = DocApply { node_id: node_id.into(), update: Bytes(update.to_vec()) }, |r| r;
        fn pull(node_id: &str, since: &[u8]) -> Vec<Vec<u8>> = DocPull { node_id: node_id.into(), since: Bytes(since.to_vec()) }, |u: DocUpdates| u.updates.into_iter().map(|b| b.0).collect();
    }
}
