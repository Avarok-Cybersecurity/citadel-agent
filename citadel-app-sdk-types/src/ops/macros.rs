//! The macro that turns the op table into the group enum, the per-group op enums, the
//! [`HostOp`](super::HostOp) impls, [`HostCall`](super::HostCall) and [`OPS`](super::OPS).

macro_rules! cap_of {
    ($s:ident, args) => {
        $s.required_capability()
    };
    ($s:ident, $c:ident) => {
        Capability::$c
    };
}

macro_rules! fixed_cap {
    (args) => {
        None
    };
    ($c:ident) => {
        Some(Capability::$c)
    };
}

macro_rules! groups {
    ($($group:ident = $gnum:literal, $import:literal, $ops:ident {
        $($op:ident = $onum:literal -> $reply:ty, $cap:ident;)*
    })*) => {
        /// An import group: one `(op, ptr, len) -> handle` function in the ABI namespace.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        pub enum Group { $($group,)* }

        impl Group {
            pub const ALL: &'static [Group] = &[$(Group::$group,)*];

            pub const fn index(self) -> i32 {
                match self { $(Group::$group => $gnum,)* }
            }

            /// The import's field name in the ABI namespace.
            pub const fn import_name(self) -> &'static str {
                match self { $(Group::$group => $import,)* }
            }

            pub fn from_import_name(name: &str) -> Option<Group> {
                match name { $($import => Some(Group::$group),)* _ => None }
            }

            pub fn from_index(index: i32) -> Option<Group> {
                match index { $($gnum => Some(Group::$group),)* _ => None }
            }
        }

        $(
            #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
            #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
            pub enum $ops { $($op($op),)* }

            impl $ops {
                pub fn op(&self) -> i32 {
                    match self { $($ops::$op(_) => $onum,)* }
                }

                pub fn capability(&self) -> Capability {
                    match self { $($ops::$op(a) => a.capability(),)* }
                }

                fn encode_args(&self) -> Vec<u8> {
                    match self { $($ops::$op(a) => encode(a),)* }
                }

                fn decode_op(op: i32, bytes: &[u8]) -> Result<Self, CallDecodeError> {
                    match op {
                        $($onum => decode::<$op>(bytes)
                            .map($ops::$op)
                            .map_err(CallDecodeError::Malformed),)*
                        _ => Err(CallDecodeError::UnknownOp),
                    }
                }
            }

            $(
                impl HostOp for $op {
                    const GROUP: Group = Group::$group;
                    const OP: i32 = $onum;
                    const NAME: &'static str = stringify!($op);
                    type Reply = $reply;

                    fn capability(&self) -> Capability {
                        cap_of!(self, $cap)
                    }
                }
            )*
        )*

        /// Any call, as the kernel's server re-check carries it (`AppsRequest::Call`).
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        pub enum HostCall { $($group($ops),)* }

        impl HostCall {
            pub fn group(&self) -> Group {
                match self { $(HostCall::$group(_) => Group::$group,)* }
            }

            pub fn op(&self) -> i32 {
                match self { $(HostCall::$group(c) => c.op(),)* }
            }

            pub fn capability(&self) -> Capability {
                match self { $(HostCall::$group(c) => c.capability(),)* }
            }

            /// The request payload: CBOR of the op's argument struct.
            pub fn encode_args(&self) -> Vec<u8> {
                match self { $(HostCall::$group(c) => c.encode_args(),)* }
            }

            /// What a group import received, as a call.
            pub fn decode(group: Group, op: i32, bytes: &[u8]) -> Result<HostCall, CallDecodeError> {
                match group { $(Group::$group => $ops::decode_op(op, bytes).map(HostCall::$group),)* }
            }
        }

        pub const OPS: &[OpInfo] = &[$($(OpInfo {
            group: Group::$group,
            op: $onum,
            name: stringify!($op),
            capability: fixed_cap!($cap),
        },)*)*];

        /// `(op name, TypeScript name of its reply)`, for `AbiConstants.ts`.
        #[cfg(feature = "ts")]
        pub(crate) fn ts_reply_names() -> Vec<(&'static str, alloc::string::String)> {
            alloc::vec![$($((stringify!($op), <$reply as ts_rs::TS>::name()),)*)*]
        }
    };
}
