// Export the original WebSocket-based client
export { CitadelClient } from './CitadelClient';
export type { CitadelClientConfig, ConnectOptions as WSConnectOptions, MessageOptions as WSMessageOptions } from './CitadelClient';

// Export the new WASM-based client
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