// NOTE: CitadelClient is Node.js-only (uses 'ws' package) and should NOT be exported
// for browser environments. Use InternalServiceWasmClient instead.
// To use CitadelClient in Node.js, import directly: import { CitadelClient } from './CitadelClient'

// Export the WASM-based client for browser environments
export { InternalServiceWasmClient } from './InternalServiceWasmClient';
export type {
    WasmClientConfig,
    WasmModule,
    ConnectOptions as WasmConnectOptions,
    RegisterOptions as WasmRegisterOptions,
    MessageOptions as WasmMessageOptions
} from './InternalServiceWasmClient';

// Export all types
export * from './types';

// Export type-safe discriminated union narrowing helpers
export { isResponseType, isRequestType, isVariant } from './type-guards';
export type { DiscriminatorOf, ResponseType, RequestType } from './type-guards';

// Re-export as default for convenience
export { InternalServiceWasmClient as default } from './InternalServiceWasmClient'; 