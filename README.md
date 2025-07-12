# Citadel Internal Service

This repository contains the internal service implementation for the Citadel Protocol, providing a standardized interface for client applications to interact with Citadel's secure communication features.

## Components

- **citadel-internal-service**: Core service implementation
- **citadel-internal-service-types**: Shared type definitions with TypeScript generation
- **citadel-internal-service-connector**: Connection handling and IO interfaces
- **typescript-client**: TypeScript client with WebSocket support

## Automated TypeScript Type Generation

The project features **fully automated TypeScript type generation** from Rust types using [ts-rs](https://github.com/Aleph-Alpha/ts-rs). All types are properly annotated and automatically exported with dependency resolution.

### Quick Start

Generate TypeScript types with a single command:

```bash
./generate_types.sh
```

This script automatically:
- ✅ Builds the Rust crate with TypeScript features
- ✅ Generates all TypeScript types (85+ files) with proper imports  
- ✅ Creates convenient `index.ts` for easy imports
- ✅ Fixes any missing import statements
- ✅ Verifies TypeScript compilation

### Features

- **Zero Configuration**: Works out of the box
- **Complete Type Coverage**: All request/response types included
- **Automatic Imports**: Dependencies resolved automatically
- **Build Integration**: Types generated directly to `typescript-client/src/types/`
- **Validation**: Automatically verifies TypeScript compilation

### Implementation Details

The automated generation uses:
- `ts-rs` library with `#[ts(export)]` annotations on all types
- Build script (`build.rs`) that sets proper export directories
- Test-based export triggering for complete type coverage
- Post-processing for missing import statements
- Index file generation for convenient imports

### Architecture

```
citadel-internal-service-types/
├── src/lib.rs                    # All types with #[ts(export)]
├── build.rs                      # Sets TS_RS_EXPORT_DIR
├── examples/generate_ts_types.rs  # Export trigger
└── tests/                        # Export tests for all types

typescript-client/src/types/
├── index.ts                      # Convenient re-exports
├── InternalServiceRequest.ts     # Main request enum
├── InternalServiceResponse.ts    # Main response enum
└── *.ts                         # Individual type files (85+)
```

All types use proper TypeScript conventions and include automatic import resolution for dependencies.

## Development

### Building

```bash
# Build with TypeScript features
cd citadel-internal-service-types
cargo build --features typescript

# Build TypeScript client
cd typescript-client
npm run build
```

### Testing

```bash
# Run Rust tests
cargo test

# Test TypeScript client
cd typescript-client
npm test
```

### Regenerating Types

Simply run the generation script whenever Rust types change:

```bash
./generate_types.sh
```

The script is idempotent and safe to run multiple times.