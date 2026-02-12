import WebSocket from 'ws';
import { v4 as uuidv4 } from 'uuid';
import {
    InternalServicePayload,
    InternalServiceRequest,
    InternalServiceResponse,
    ConnectSuccess,
    ConnectFailure,
    MessageSendSuccess,
    MessageSendFailure,
    MessageNotification
} from './types';
import { isResponseType, isVariant } from './type-guards';
import type { ConnectMode, UdpMode, SessionSecuritySettings, PreSharedKey, SecurityLevel } from '@avarok/citadel-protocol-types';

export interface CitadelClientConfig {
    url: string;
    username: string;
    password: string;
    timeout?: number;
}

export interface ConnectOptions {
    connectMode?: ConnectMode;
    udpMode?: UdpMode;
    keepAliveTimeout?: { secs: number; nanos: number } | null;
    sessionSecuritySettings?: SessionSecuritySettings;
    serverPassword?: PreSharedKey | null;
}

export interface MessageOptions {
    peer_cid?: bigint | null;
    security_level?: SecurityLevel;
}

export class CitadelClient {
    private ws: WebSocket | null = null;
    private config: CitadelClientConfig;
    private pendingRequests = new Map<string, {
        resolve: (value: InternalServiceResponse) => void;
        reject: (reason: unknown) => void;
        timeout: NodeJS.Timeout;
    }>();
    private messageHandlers = new Set<(message: MessageNotification) => void>();
    private isConnected = false;
    private connectedCid: bigint | null = null;
    private defaultTimeout: number;

    constructor(config: CitadelClientConfig) {
        this.config = config;
        this.defaultTimeout = config.timeout || 30000; // 30 seconds default
    }

    async connect(options: ConnectOptions = {}): Promise<ConnectSuccess> {
        return new Promise((resolve, reject) => {
            try {
                this.ws = new WebSocket(this.config.url);

                this.ws.on('open', () => {
                    this.isConnected = true;
                    this.sendConnectRequest(options).then(resolve).catch(reject);
                });

                this.ws.on('message', (data: WebSocket.Data) => {
                    this.handleMessage(data);
                });

                this.ws.on('error', (error) => {
                    console.error('WebSocket error:', error);
                    reject(error);
                });

                this.ws.on('close', () => {
                    this.isConnected = false;
                    this.connectedCid = null;
                    this.cleanup();
                });

            } catch (error) {
                reject(error);
            }
        });
    }

    private async sendConnectRequest(options: ConnectOptions): Promise<ConnectSuccess> {
        const requestId = uuidv4();
        const passwordBytes = Buffer.from(this.config.password, 'utf-8');

        const connectRequest: InternalServiceRequest = {
            Connect: {
                request_id: requestId,
                username: this.config.username,
                password: Array.from(passwordBytes),
                connect_mode: options.connectMode || { Standard: { force_login: false } },
                udp_mode: options.udpMode || "Disabled",
                keep_alive_timeout: options.keepAliveTimeout || { secs: 30, nanos: 0 },
                session_security_settings: options.sessionSecuritySettings || {
                    security_level: "Standard",
                    secrecy_mode: "BestEffort",
                    crypto_params: {
                        encryption_algorithm: "AES_GCM_256",
                        kem_algorithm: "MlKem",
                        sig_algorithm: "None"
                    },
                    header_obfuscator_settings: "Disabled"
                },
                server_password: options.serverPassword || null
            }
        };

        const payload: InternalServicePayload = { Request: connectRequest };
        return this.sendRequest(payload, requestId);
    }

    async sendMessage(message: string, options: MessageOptions = {}): Promise<MessageSendSuccess> {
        if (!this.connectedCid) {
            throw new Error('Not connected to Citadel service');
        }

        const requestId = uuidv4();
        const messageBytes = Buffer.from(message, 'utf-8');

        const messageRequest: InternalServiceRequest = {
            Message: {
                request_id: requestId,
                message: Array.from(messageBytes),
                cid: this.connectedCid,
                peer_cid: options.peer_cid || null,
                security_level: options.security_level || "Standard"
            }
        };

        const payload: InternalServicePayload = { Request: messageRequest };
        return this.sendRequest(payload, requestId);
    }

    async disconnect(): Promise<void> {
        if (!this.connectedCid) {
            throw new Error('Not connected to Citadel service');
        }

        const requestId = uuidv4();
        const disconnectRequest: InternalServiceRequest = {
            Disconnect: {
                request_id: requestId,
                cid: this.connectedCid
            }
        };

        const payload: InternalServicePayload = { Request: disconnectRequest };

        try {
            await this.sendRequest(payload, requestId);
        } finally {
            this.close();
        }
    }

    close(): void {
        this.cleanup();
        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }
        this.isConnected = false;
        this.connectedCid = null;
    }

    onMessage(handler: (message: MessageNotification) => void): void {
        this.messageHandlers.add(handler);
    }

    offMessage(handler: (message: MessageNotification) => void): void {
        this.messageHandlers.delete(handler);
    }

    private sendRequest<T>(payload: InternalServicePayload, requestId: string): Promise<T> {
        return new Promise((resolve, reject) => {
            if (!this.ws || !this.isConnected) {
                reject(new Error('WebSocket not connected'));
                return;
            }

            const timeout = setTimeout(() => {
                this.pendingRequests.delete(requestId);
                reject(new Error(`Request timeout after ${this.defaultTimeout}ms`));
            }, this.defaultTimeout);

            // Heterogeneous promise map: resolve is typed as (T) => void from the Promise<T>,
            // but handleResponse always passes InternalServiceResponse. This cast is safe
            // because T is always narrowed from InternalServiceResponse at call sites.
            this.pendingRequests.set(requestId, {
                resolve: resolve as (value: InternalServiceResponse) => void,
                reject,
                timeout
            });

            try {
                const message = JSON.stringify(payload);
                this.ws.send(message);
            } catch (error) {
                this.pendingRequests.delete(requestId);
                clearTimeout(timeout);
                reject(error);
            }
        });
    }

    private handleMessage(data: WebSocket.Data): void {
        try {
            const message = data.toString();
            const payload: InternalServicePayload = JSON.parse(message);

            if (isVariant(payload, 'Response')) {
                this.handleResponse(payload.Response);
            }
        } catch (error) {
            console.error('Error handling message:', error);
        }
    }

    private handleResponse(response: InternalServiceResponse): void {
        const requestId = this.extractRequestId(response);

        if (requestId && this.pendingRequests.has(requestId)) {
            const { resolve, reject, timeout } = this.pendingRequests.get(requestId)!;
            this.pendingRequests.delete(requestId);
            clearTimeout(timeout);

            if (this.isErrorResponse(response)) {
                reject(new Error(this.extractErrorMessage(response)));
            } else {
                // Handle ConnectSuccess specially to extract CID
                if (isResponseType(response, 'ConnectSuccess')) {
                    this.connectedCid = response.ConnectSuccess.cid;
                }
                resolve(response);
            }
        }

        // Handle notifications
        if (this.isNotification(response)) {
            if (isResponseType(response, 'MessageNotification')) {
                this.messageHandlers.forEach(handler => {
                    try {
                        handler(response.MessageNotification);
                    } catch (error) {
                        console.error('Error in message handler:', error);
                    }
                });
            }
        }
    }

    private extractRequestId(response: InternalServiceResponse): string | null {
        const responseData = Object.values(response)[0] as Record<string, unknown> | undefined;
        return (typeof responseData?.request_id === 'string') ? responseData.request_id : null;
    }

    private isErrorResponse(response: InternalServiceResponse): boolean {
        return Object.keys(response).some(key => key.includes('Failure'));
    }

    private extractErrorMessage(response: InternalServiceResponse): string {
        const responseData = Object.values(response)[0] as Record<string, unknown> | undefined;
        return (typeof responseData?.message === 'string') ? responseData.message : 'Unknown error';
    }

    private isNotification(response: InternalServiceResponse): boolean {
        return Object.keys(response).some(key => key.includes('Notification'));
    }

    private cleanup(): void {
        // Reject all pending requests
        this.pendingRequests.forEach(({ reject, timeout }) => {
            clearTimeout(timeout);
            reject(new Error('Connection closed'));
        });
        this.pendingRequests.clear();
    }
} 