pub mod kernel;

// Re-export the browser-transfer startup sweep so the binary entrypoint
// can call it before the runtime spins up, without exposing the full
// internal `requests::file` module.
pub use kernel::requests::file::upload::sweep_stale_browser_transfers;
