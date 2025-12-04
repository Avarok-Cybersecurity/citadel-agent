// UUID implementation removed - using simple implementation below

// Import the main generated types from ts-rs
import type { InternalServiceRequest } from './types/InternalServiceRequest';
import type { InternalServiceResponse } from './types/InternalServiceResponse';
import type { ConnectSuccess } from './types/ConnectSuccess';
import type { RegisterSuccess } from './types/RegisterSuccess';
import type { ConfigCommand } from './types/ConfigCommand';
import type { ConnectionManagementSuccess } from './types/ConnectionManagementSuccess';
import type { ConnectionManagementFailure } from './types/ConnectionManagementFailure';

// WASM module will be loaded dynamically

// Type definitions for WASM functions (based on our WASM implementation)
interface WasmModule {
    init(url: string): Promise<void>;
    restart(url: string): Promise<void>;
    open_p2p_connection(cid: string): Promise<void>;
    next_message(): Promise<any>;
    send_p2p_message(cid: string, message: any): Promise<void>;
    send_p2p_message_reliable(localCid: string, peerCid: string, message: Uint8Array, securityLevel: string | null): Promise<void>;
    send_direct_to_internal_service(message: any): Promise<void>;
    close_connection(): Promise<void>;
    get_version(): string;
    is_initialized(): boolean;
}

export interface WasmClientConfig {
    websocketUrl: string;
    messageHandler?: (message: InternalServiceResponse) => void;
    errorHandler?: (error: Error) => void;
    timeout?: number;
}

// Extract exact field types from generated union types, excluding request_id (generated internally)
type ConnectRequestFields = Extract<InternalServiceRequest, { Connect: any }>['Connect'];
type RegisterRequestFields = Extract<InternalServiceRequest, { Register: any }>['Register'];
type MessageRequestFields = Extract<InternalServiceRequest, { Message: any }>['Message'];

// User-facing types (exactly the same as bindgen types)
export type ConnectOptions = ConnectRequestFields;
export type RegisterOptions = RegisterRequestFields;
export type MessageOptions = MessageRequestFields;

// Each client needs its own WASM module instance since WASM uses global state
// that can only handle one connection at a time

export class InternalServiceWasmClient {
    private wasmModule: WasmModule | null = null;
    private config: WasmClientConfig;
    private isConnected = false;
    private currentCid: string | null = null;
    private p2pConnections = new Set<string>();
    private messageHandler?: (message: InternalServiceResponse) => void;
    private errorHandler?: (error: Error) => void;
    private initializationComplete = false;

    constructor(config: WasmClientConfig) {
        this.config = config;
        this.messageHandler = config.messageHandler;
        this.errorHandler = config.errorHandler;
    }

    /**
     * Initialize the WASM client and connect to the WebSocket
     */
    async init(): Promise<void> {
        try {
            console.log(`Initializing WASM client with URL: ${this.config.websocketUrl}`);

            // Load the shared WASM module
            const wasmModule = await this.loadWasmModule();
            this.wasmModule = wasmModule;

            // Initialize the WASM client with WebSocket URL
            console.log('Connecting to WebSocket...');
            await this.wasmModule.init(this.config.websocketUrl);
            console.log('WebSocket connected successfully');

            // Start background message processing
            this.startMessageProcessing();

            // Mark initialization as complete
            this.initializationComplete = true;

            console.log(`WASM client initialized successfully. Version: ${this.getVersion()}`);
        } catch (error) {
            console.error('WASM client initialization failed:', error);
            const err = new Error(`Failed to initialize WASM client: ${error}`);
            this.handleError(err);
            throw err;
        }
    }

    /**
     * Connect to the Citadel service
     */
    async connect(options: ConnectOptions): Promise<ConnectSuccess> {
        this.ensureInitialized();

        const connectRequest: InternalServiceRequest = {
            Connect: options
        };

        await this.wasmModule!.send_direct_to_internal_service(connectRequest);

        // Wait for ConnectSuccess response
        return this.waitForResponse<ConnectSuccess>('ConnectSuccess');
    }

    async restart_ws_connection(): Promise<void> {
        //
        await this.wasmModule!.restart(this.config.websocketUrl);
        this.startMessageProcessing();
        this.initializationComplete = true;
    }

    /**
     * Register a new user with the Citadel service
     */
    async register(options: RegisterOptions): Promise<RegisterSuccess> {
        this.ensureInitialized();

        const registerRequest: InternalServiceRequest = {
            Register: options
        };

        console.log('InternalServiceWasmClient.register - sending request:', JSON.stringify(registerRequest, null, 2));

        // For registration with connect_after_register=false, we need to handle this differently
        // The response might come before the connection closes
        const responsePromise = this.waitForResponse<RegisterSuccess>('RegisterSuccess', 5000);

        await this.wasmModule!.send_direct_to_internal_service(registerRequest);

        return responsePromise;
    }

    /**
     * Open a P2P connection to another peer
     */
    async openP2PConnection(peerCid: string): Promise<void> {
        this.ensureInitialized();

        await this.wasmModule!.open_p2p_connection(peerCid);
        this.p2pConnections.add(peerCid);
    }

    /**
     * Send a message to a peer via P2P connection
     */
    async sendP2PMessage(options: MessageOptions): Promise<void> {
        this.ensureInitialized();

        if (!this.currentCid) {
            throw new Error('Not connected to Citadel service. Call connect() first.');
        }

        const peerCidStr = options.peer_cid?.toString();
        if (!peerCidStr || !this.p2pConnections.has(peerCidStr)) {
            throw new Error(`No P2P connection to peer ${peerCidStr}. Call openP2PConnection() first.`);
        }

        const messageRequest: InternalServiceRequest = {
            Message: options
        };

        await this.wasmModule!.send_p2p_message(peerCidStr, messageRequest);
    }

    /**
     * Send a reliable P2P message using ISM layer for guaranteed delivery.
     * This method uses the ISM (InterSession Messaging) layer instead of bypassing it.
     * @param localCid - The local user's CID
     * @param peerCid - The target peer's CID
     * @param message - The message bytes to send
     * @param securityLevel - Optional security level: 'Standard', 'Reinforced', 'High', or 'Extreme'
     */
    async sendP2PMessageReliable(
        localCid: string,
        peerCid: string,
        message: Uint8Array,
        securityLevel?: 'Standard' | 'Reinforced' | 'High' | 'Extreme'
    ): Promise<void> {
        this.ensureInitialized();

        await this.wasmModule!.send_p2p_message_reliable(
            localCid,
            peerCid,
            message,
            securityLevel || null
        );
    }

    /**
     * Send a direct message to the internal service
     */
    async sendDirectToInternalService(request: InternalServiceRequest): Promise<void> {
        this.ensureInitialized();
        await this.wasmModule!.send_direct_to_internal_service(request);
    }

    /**
     * Get the next message from the service (for manual message processing)
     */
    async nextMessage(): Promise<InternalServiceResponse> {
        this.ensureInitialized();
        return await this.wasmModule!.next_message();
    }

    /**
     * Close the connection and clean up resources
     */
    async close(): Promise<void> {
        if (this.wasmModule) {
            await this.wasmModule.close_connection();
        }

        this.isConnected = false;
        this.currentCid = null;
        this.p2pConnections.clear();
        this.wasmModule = null;
        this.initializationComplete = false;
    }

    /**
     * Get the WASM client version
     */
    getVersion(): string {
        return this.wasmModule?.get_version() || 'Unknown';
    }

    /**
     * Check if the client is initialized
     */
    isInitialized(): boolean {
        return this.wasmModule?.is_initialized() || false;
    }

    /**
     * Get the current CID
     */
    getCurrentCid(): string | null {
        return this.currentCid;
    }

    /**
     * Get the list of active P2P connections
     */
    getP2PConnections(): string[] {
        return Array.from(this.p2pConnections);
    }

    /**
     * Set a message handler for incoming messages
     */
    setMessageHandler(handler: (message: InternalServiceResponse) => void): void {
        this.messageHandler = handler;
    }

    /**
     * Set an error handler for errors
     */
    setErrorHandler(handler: (error: Error) => void): void {
        this.errorHandler = handler;
    }

    // Private methods

    private async loadWasmModule(): Promise<WasmModule> {
        try {
            console.log('Loading WASM module...');

            // Import the WASM JS module using relative path from src/ to package root
            // @ts-ignore - Dynamic import of WASM glue code
            const wasmModule = await import('../citadel_internal_service_wasm_client.js');

            // Initialize the WASM module with explicit path to .wasm binary in public directory
            // This is needed because Vite mangles import.meta.url used by wasm-bindgen
            const wasmBinaryUrl = '/wasm/citadel_internal_service_wasm_client_bg.wasm';
            console.log('Initializing WASM module with binary from:', wasmBinaryUrl);
            await wasmModule.default(wasmBinaryUrl);
            console.log('WASM module initialized successfully');

            return wasmModule as any;
        } catch (error) {
            console.error('Failed to load WASM module:', error);
            throw new Error(`Failed to load WASM module: ${error}`);
        }
    }

    private startMessageProcessing(): void {
        if (this.initializationComplete && this.wasmModule) {
            console.warn("Will not run startMessageProcessing since already initialized")
            return;
        }

        // Start a background task to process messages
        this.processMessages();
    }

    private async processMessages(): Promise<void> {
        try {
            while (this.wasmModule && this.isInitialized()) {
                try {
                    const message = await this.wasmModule.next_message();
                    this.handleMessage(message);
                } catch (error) {
                    // Handle specific errors or continue processing
                    if (error && error.toString().includes('Stream closed')) {
                        break;
                    }
                    this.handleError(new Error(`Message processing error: ${error}`));
                    // Wait a bit before retrying
                    await new Promise(resolve => setTimeout(resolve, 1000));
                }
            }
        } catch (error) {
            this.handleError(new Error(`Message processing loop error: ${error}`));
        }
        this.initializationComplete = false;
    }

    private handleMessage(message: InternalServiceResponse): void {
        console.log('InternalServiceWasmClient: Received message:', JSON.stringify(message));
        try {
            // Handle specific message types for client state management
            if ('ConnectSuccess' in message) {
                this.currentCid = message.ConnectSuccess.cid.toString();
                this.isConnected = true;
                console.log('InternalServiceWasmClient: ConnectSuccess received, CID:', this.currentCid);
            } else if ('RegisterSuccess' in message) {
                this.currentCid = message.RegisterSuccess.cid.toString();
                this.isConnected = true;
                console.log('InternalServiceWasmClient: RegisterSuccess received, CID:', this.currentCid);
            } else if ('ServiceConnectionAccepted' in message) {
                // Connection to service established
                console.log('Service connection accepted');
            }

            // Call the user-provided message handler
            if (this.messageHandler) {
                console.log('InternalServiceWasmClient: Calling user message handler');
                this.messageHandler(message);
            }
        } catch (error) {
            this.handleError(new Error(`Error handling message: ${error}`));
        }
    }

    private handleError(error: Error): void {
        if (this.errorHandler) {
            this.errorHandler(error);
        } else {
            console.error('WASM Client Error:', error);
        }
    }

    private async waitForResponse<T>(responseType: string, timeout: number = 30000): Promise<T> {
        return new Promise((resolve, reject) => {
            const timeoutId = setTimeout(() => {
                reject(new Error(`Timeout waiting for ${responseType} after ${timeout}ms`));
            }, timeout);

            const originalHandler = this.messageHandler;

            const responseHandler = (message: InternalServiceResponse) => {
                // For registration with connect_after_register=true, accept either RegisterSuccess or ConnectSuccess
                const isRegisterRequest = responseType === 'RegisterSuccess';
                const hasConnectSuccess = 'ConnectSuccess' in message;
                const hasRegisterSuccess = responseType in message;

                if (hasRegisterSuccess || (isRegisterRequest && hasConnectSuccess)) {
                    clearTimeout(timeoutId);
                    this.messageHandler = originalHandler; // Restore original handler

                    // Always call the original handler to ensure events are propagated
                    if (originalHandler) {
                        originalHandler(message);
                    }

                    if (hasRegisterSuccess) {
                        resolve((message as any)[responseType]);
                    } else if (isRegisterRequest && hasConnectSuccess) {
                        // Registration with connect_after_register=true returns ConnectSuccess
                        resolve((message as any)['ConnectSuccess']);
                    }
                } else if (responseType.replace('Success', 'Failure') in message) {
                    clearTimeout(timeoutId);
                    this.messageHandler = originalHandler; // Restore original handler
                    const failure = (message as any)[responseType.replace('Success', 'Failure')];
                    reject(new Error(failure.message || `${responseType} failed`));
                } else {
                    // Call original handler for other messages
                    if (originalHandler) {
                        originalHandler(message);
                    }
                }
            };

            this.messageHandler = responseHandler;
        });
    }

    private ensureInitialized(): void {
        if (!this.wasmModule || !this.initializationComplete) {
            console.error('WASM client not properly initialized', {
                hasWasmModule: !!this.wasmModule,
                initializationComplete: this.initializationComplete,
                wasmIsInitialized: this.wasmModule?.is_initialized()
            });
            throw new Error('WASM client not initialized. Call init() first.');
        }
    }

    /**
     * Enable orphan mode for the current connection
     * When enabled, sessions will persist even when the TCP connection drops
     */
    async setOrphanMode(enabled: boolean): Promise<ConnectionManagementSuccess | ConnectionManagementFailure> {
        this.ensureInitialized();

        const configCommand: ConfigCommand = {
            SetConnectionOrphan: {
                allow_orphan_sessions: enabled
            }
        };

        const request: InternalServiceRequest = {
            ConnectionManagement: {
                request_id: InternalServiceWasmClient.generateUUID(),
                management_command: configCommand
            }
        };

        await this.wasmModule!.send_direct_to_internal_service(request);

        // Wait for ConnectionManagementSuccess or ConnectionManagementFailure
        return this.waitForConnectionManagementResponse();
    }

    /**
     * Claim an existing session (take over from another connection)
     * @param sessionCid The CID of the session to claim
     * @param onlyIfOrphaned If true, only claim if the session is orphaned
     */
    async claimSession(sessionCid: bigint | string, onlyIfOrphaned: boolean = false): Promise<ConnectionManagementSuccess | ConnectionManagementFailure> {
        this.ensureInitialized();

        const cid = typeof sessionCid === 'string' ? BigInt(sessionCid) : sessionCid;

        const configCommand: ConfigCommand = {
            ClaimSession: {
                session_cid: cid,
                only_if_orphaned: onlyIfOrphaned
            }
        };

        const request: InternalServiceRequest = {
            ConnectionManagement: {
                request_id: InternalServiceWasmClient.generateUUID(),
                management_command: configCommand
            }
        };

        await this.wasmModule!.send_direct_to_internal_service(request);

        return this.waitForConnectionManagementResponse();
    }

    /**
     * Disconnect orphan sessions
     * @param sessionCid Optional - if provided, disconnect specific session. If null, disconnect all orphan sessions.
     */
    async disconnectOrphan(sessionCid?: bigint | string | null): Promise<ConnectionManagementSuccess | ConnectionManagementFailure> {
        this.ensureInitialized();

        const cid = sessionCid ? (typeof sessionCid === 'string' ? BigInt(sessionCid) : sessionCid) : null;

        const configCommand: ConfigCommand = {
            DisconnectOrphan: {
                session_cid: cid
            }
        };

        const request: InternalServiceRequest = {
            ConnectionManagement: {
                request_id: InternalServiceWasmClient.generateUUID(),
                management_command: configCommand
            }
        };

        await this.wasmModule!.send_direct_to_internal_service(request);

        return this.waitForConnectionManagementResponse();
    }

    /**
     * Wait for a connection management response
     */
    private async waitForConnectionManagementResponse(timeout: number = 30000): Promise<ConnectionManagementSuccess | ConnectionManagementFailure> {
        return new Promise((resolve, reject) => {
            const timeoutId = setTimeout(() => {
                reject(new Error(`Timeout waiting for ConnectionManagement response after ${timeout}ms`));
            }, timeout);

            const originalHandler = this.messageHandler;

            const responseHandler = (message: InternalServiceResponse) => {
                if ('ConnectionManagementSuccess' in message) {
                    clearTimeout(timeoutId);
                    this.messageHandler = originalHandler;

                    if (originalHandler) {
                        originalHandler(message);
                    }

                    resolve(message.ConnectionManagementSuccess);
                } else if ('ConnectionManagementFailure' in message) {
                    clearTimeout(timeoutId);
                    this.messageHandler = originalHandler;

                    if (originalHandler) {
                        originalHandler(message);
                    }

                    resolve(message.ConnectionManagementFailure);
                } else {
                    // Pass through other messages
                    if (originalHandler) {
                        originalHandler(message);
                    }
                }
            };

            this.messageHandler = responseHandler;
        });
    }

    /**
     * Generate a UUID v4 string for use as request_id
     */
    static generateUUID(): string {
        return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function (c) {
            const r = Math.random() * 16 | 0;
            const v = c === 'x' ? r : (r & 0x3 | 0x8);
            return v.toString(16);
        });
    }
}

export default InternalServiceWasmClient;
