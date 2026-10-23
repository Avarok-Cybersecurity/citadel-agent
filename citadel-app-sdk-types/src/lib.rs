//! The Citadel app ABI, `citadel_app_abi_1`: the one definition of every message that crosses
//! between a sandboxed app (a core wasm module, wasm32-unknown-unknown, no WASI, no glue) and
//! its host.
//!
//! # The boundary
//!
//! The module imports its linear memory as `citadel_app_abi_1.memory` (so the host sets the hard
//! maximum) and one function per capability [`Group`], each `(op, ptr, len) -> handle`, plus
//! [`abi::REPLY_LEN`] and [`abi::REPLY_READ`]. It exports [`abi::REQUIRED_EXPORTS`] (`app_close`
//! among them) and, optionally, [`abi::EXPORT_FRAME`]. Its life is the one state machine in
//! [`lifecycle`]. Every payload is CBOR: the request is the op's argument
//! struct ([`HostOp`]), the reply a [`HostReply`] of the op's `Reply`. A group the install was not
//! granted is still importable, and every call to it answers `{refused: "ungranted"}`.
//!
//! # SSOT
//!
//! The group and op numbering, the capability names and the capability each op needs are
//! declared once, in [`ops`] and [`capability`], and exported to TypeScript (`bindings/`,
//! `bindings/AbiConstants.ts`) for the host. The workspace types crate's `Capability` must be a
//! re-export of [`Capability`], not a second enum.
#![cfg_attr(not(any(feature = "ts", test)), no_std)]

extern crate alloc;

/// Every ABI message: CBOR through serde, TypeScript through ts-rs (feature `ts`).
macro_rules! abi_types {
    ($($item:item)*) => {
        $(
            #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
            #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
            $item
        )*
    };
}

pub mod abi;
mod bytes;
pub mod calls;
pub mod capability;
mod codec;
mod context;
mod event;
pub mod lifecycle;
pub mod ops;
mod refused;
#[cfg(feature = "ts")]
mod ts_constants;
#[cfg(feature = "ts")]
pub use ts_constants::render as render_ts_constants;

pub use bytes::Bytes;
pub use capability::Capability;
pub use codec::{decode, encode, DecodeError};
pub use context::{AppContext, AppScope, Size, Surface};
pub use event::*;
pub use lifecycle::{CloseReason, LifecycleState, LifecycleStep, SuspendReason};
pub use ops::{Group, HostCall, HostOp, OpInfo, OPS};
pub use refused::{HostReply, Refused};
