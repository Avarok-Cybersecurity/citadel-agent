// NOTE: CitadelClient is Node.js-only (uses 'ws' package) and should NOT be exported
// for browser environments. Use InternalServiceWasmClient instead.
// To use CitadelClient in Node.js, import directly: import { CitadelClient } from './CitadelClient'

// Export the WASM-based client for browser environments
export { InternalServiceWasmClient } from './InternalServiceWasmClient';
export type {
    WasmClientConfig,
    ConnectOptions as WasmConnectOptions,
    RegisterOptions as WasmRegisterOptions,
    MessageOptions as WasmMessageOptions
} from './InternalServiceWasmClient';

// Export all types
export * from './types';

// Re-export as default for convenience
export { InternalServiceWasmClient as default } from './InternalServiceWasmClient'; 