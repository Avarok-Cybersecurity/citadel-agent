//! The macro that turns the op table into the group enum, the per-group op enums, the
//! [`HostOp`](super::HostOp) impls, [`HostCall`](super::HostCall) and [`OPS`](super::OPS).

/// The capabilities any one of which admits a call: the backend's for `args`, otherwise the
/// table's (`Cap` or `[CapA, CapB]`).
macro_rules! granted_by {
    ($s:ident, args) => {
        $s.required_capability().alone()
    };
    ($s:ident, $c:tt) => {
        fixed_caps!($c)
    };
}

macro_rules! fixed_caps {
    ([$($c:ident),+]) => {
        &[$(Capability::$c),+]
    };
    ($c:ident) => {
        &[Capability::$c]
    };
}

macro_rules! op_info_caps {
    (args) => {
        None
    };
    ($c:tt) => {
        Some(fixed_caps!($c))
    };
}

macro_rules! groups {
    ($($group:ident = $gnum:literal, $import:literal, $ops:ident {
        $($op:ident = $onum:literal -> $reply:ty, $cap:tt;)*
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

                pub fn granted_by(&self) -> &'static [Capability] {
                    match self { $($ops::$op(a) => a.granted_by(),)* }
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

                    fn granted_by(&self) -> &'static [Capability] {
                        granted_by!(self, $cap)
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

            /// Any one of these capabilities admits the call; never empty.
            pub fn granted_by(&self) -> &'static [Capability] {
                match self { $(HostCall::$group(c) => c.granted_by(),)* }
            }

            /// Whether an install holding what `holds` says may make this call.
            pub fn is_granted(&self, holds: impl Fn(Capability) -> bool) -> bool {
                self.granted_by().iter().any(|c| holds(*c))
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
            granted_by: op_info_caps!($cap),
        },)*)*];

        /// `(op name, TypeScript name of its reply)`, for `AbiConstants.ts`.
        #[cfg(feature = "ts")]
        pub(crate) fn ts_reply_names() -> Vec<(&'static str, alloc::string::String)> {
            alloc::vec![$($((stringify!($op), <$reply as ts_rs::TS>::name()),)*)*]
        }
    };
}
