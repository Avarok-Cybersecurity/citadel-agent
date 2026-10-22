/**
 * The WASM module, loaded and initialised once per page (wasm-bindgen's init returns the module
 * it already has). The connected client loads it to open its socket; a caller that only needs a
 * pure export, such as `verify_app_signature`, loads it without connecting anything.
 */
import type { WasmModule } from './InternalServiceWasmClient.js';

export async function loadWasmModule(): Promise<WasmModule> {
    try {
        console.log('Loading WASM module...');

        // Imported from the PACKAGE, not from /wasm/.
        //
        // Tried and reverted: pointing this at the served /wasm/ copy makes
        // `vite build` stop needing an uncommitted wasm-pack artefact, and
        // the production build does succeed. But Vite refuses to let source
        // import anything under /public — "copied as-is during build
        // without going through the plugin transforms" — so the dev server
        // answers 500 and WASM never initialises. Prod-only verification
        // would have shipped that.
        //
        // The CI consequence (bundle-budget and PWA gates cannot build
        // without a prior WASM build) belongs in CI, not here.
        // @ts-ignore - Dynamic import of WASM glue code
        const wasmModule = await import('../citadel_internal_service_wasm_client.js');

        // Initialize the WASM module with explicit path to .wasm binary in public directory
        // This is needed because Vite mangles import.meta.url used by wasm-bindgen
        const wasmBinaryUrl = '/wasm/citadel_internal_service_wasm_client_bg.wasm';
        console.log('Initializing WASM module with binary from:', wasmBinaryUrl);
        await wasmModule.default(wasmBinaryUrl);
        console.log('WASM module initialized successfully');

        // WASM-BOUNDARY: dynamic import returns untyped module namespace, cast to our interface
        return wasmModule as unknown as WasmModule;
    } catch (error) {
        console.error('Failed to load WASM module:', error);
        throw new Error(`Failed to load WASM module: ${error}`);
    }
}
