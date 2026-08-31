pub mod kernel;

// Re-export the browser-transfer startup sweep so the binary entrypoint
// can call it before the runtime spins up, without exposing the full
// internal `requests::file` module.
pub use kernel::requests::file::upload::sweep_stale_browser_transfers;

/// Re-exported so the binaries that build a WebSocket service can name the
/// policy without depending on the connector crate directly.
#[cfg(feature = "websockets")]
pub use citadel_internal_service_connector::io_interface::origin_policy::OriginPolicy;
