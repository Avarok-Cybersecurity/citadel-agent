# WASM Support for citadel-internal-service-connector

## Current Status

The citadel-internal-service-connector library itself can be built for wasm32-wasip2 target with the following limitations:

### ✅ What Works
- The library code compiles successfully for WASM when excluding TCP networking
- The in-memory interface (`InMemoryInterface`) is fully compatible with WASM
- The connector abstraction works with WASM-compatible transports

### ❌ Current Limitations
- Tests cannot be run on WASM due to transitive dependencies:
  - `citadel_types` (via `citadel-internal-service-types`) pulls in `citadel_crypt`
  - `citadel_crypt` depends on `pqcrypto-*-wasi` crates that don't build for wasm32-wasip2
  - The pqcrypto crates have build scripts that fail with clang errors for WASM targets

## Building for WASM

To build the library for WASM:

```bash
WASI_SDK_PATH=./wasi-sdk-25.0-arm64-macos cargo build \
  --package citadel-internal-service-connector \
  --target=wasm32-wasip2
```

## Test Strategy

The library includes WASM-compatible tests in `src/lib.rs` that use only the in-memory interface. However, these cannot currently be run due to the dependency issues mentioned above.

## Future Improvements

To fully support WASM testing, one of the following approaches would be needed:

1. **Update citadel dependencies**: Make the citadel cryptographic libraries WASM-compatible
2. **Feature flags**: Add feature flags to citadel-internal-service-types to make citadel_types optional
3. **Separate test crate**: Create a minimal test crate that doesn't depend on the problematic dependencies
4. **Mock types**: Create mock versions of the internal service types for testing that don't pull in citadel dependencies

## Code Changes Made

The following changes were made to support WASM builds:

1. **Cargo.toml**: Added conditional compilation for tokio's `net` feature (only included for non-WASM targets)
2. **src/io_interface/mod.rs**: Conditionally excluded the `tcp` module for WASM builds
3. **src/connector.rs**: Added conditional compilation for TCP-related imports and functions
4. **tests/messenger.rs**: Excluded the entire test module for WASM builds
5. **src/lib.rs**: Added WASM-specific tests that use only the in-memory interface