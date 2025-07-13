# Citadel Internal Service WASM Client

A TypeScript wrapper for the Citadel Internal Service WASM client, providing a clean, type-safe interface for P2P communication through the Citadel network.

## Overview

The `InternalServiceWasmClient` provides a high-level TypeScript API that wraps the compiled WASM module, handling:

- WASM module initialization and lifecycle management
- Automatic WebSocket connection management
- Type-safe request/response handling with proper BigInt/u64 conversion
- Background message processing
- P2P connection management
- Error handling and timeouts

## Quick Start

```typescript
import { InternalServiceWasmClient } from 'citadel-websocket-client';

// Create and initialize the client
const client = new InternalServiceWasmClient({
    websocketUrl: 'ws://127.0.0.1:8080',
    messageHandler: (message) => {
        console.log('Received:', message);
    },
    errorHandler: (error) => {
        console.error('Error:', error);
    }
});

// Initialize the WASM module and connect
await client.init();

// Register a new user
await client.register({
    serverAddr: '127.0.0.1:8080',
    fullName: 'Alice Test',
    username: 'alice_test',
    password: 'password123'
});

// Open P2P connection and send messages
await client.openP2PConnection('16959591334653179280');
await client.sendP2PMessage('16959591334653179280', 'Hello!');
```

## API Reference

### Constructor

```typescript
new InternalServiceWasmClient(config: WasmClientConfig)
```

**WasmClientConfig:**
- `websocketUrl: string` - WebSocket URL for the internal service
- `messageHandler?: (message: InternalServiceResponse) => void` - Handler for incoming messages
- `errorHandler?: (error: Error) => void` - Handler for errors
- `timeout?: number` - Default timeout for operations (30s default)

### Methods

#### `init(): Promise<void>`
Initialize the WASM module and establish WebSocket connection.

#### `connect(options: ConnectOptions): Promise<ConnectSuccess>`
Connect to the Citadel service with existing credentials.

**ConnectOptions:**
- `username: string` - Username for authentication
- `password: string` - Password for authentication
- `connectMode?` - Connection mode (optional)
- `udpMode?: string` - UDP mode (default: 'Disabled')
- `keepAliveTimeout?` - Keep-alive timeout settings
- `sessionSecuritySettings?` - Security settings
- `serverPassword?` - Server password if required

#### `register(options: RegisterOptions): Promise<RegisterSuccess>`
Register a new user with the Citadel service.

**RegisterOptions:**
- `serverAddr: string` - Citadel server address
- `fullName: string` - Full name of the user
- `username: string` - Desired username
- `password: string` - Desired password
- `connectAfterRegister?: boolean` - Auto-connect after registration (default: true)
- `sessionSecuritySettings?` - Security settings
- `serverPassword?` - Server password if required

#### `openP2PConnection(peerCid: string): Promise<void>`
Open a P2P connection to another peer using their CID.

#### `sendP2PMessage(peerCid: string, message: string, options?: MessageOptions): Promise<void>`
Send a message to a connected peer.

**MessageOptions:**
- `cid?: string` - Sender CID (auto-detected if not provided)
- `peerCid?: string` - Target peer CID
- `securityLevel?: string` - Security level (default: 'Standard')

#### `sendDirectToInternalService(request: InternalServiceRequest): Promise<void>`
Send a raw request directly to the internal service.

#### `nextMessage(): Promise<InternalServiceResponse>`
Get the next message from the service (for manual message processing).

#### `close(): Promise<void>`
Close the connection and clean up all resources.

### Utility Methods

#### `getVersion(): string`
Get the WASM client version.

#### `isInitialized(): boolean`
Check if the client is initialized.

#### `getCurrentCid(): string | null`
Get the current user's CID.

#### `getP2PConnections(): string[]`
Get list of active P2P connection CIDs.

#### `setMessageHandler(handler: (message: InternalServiceResponse) => void): void`
Set/update the message handler.

#### `setErrorHandler(handler: (error: Error) => void): void`
Set/update the error handler.

## Features

### BigInt/u64 Handling
The client automatically handles large CID values that exceed JavaScript's safe integer range by:
- Converting large u64 values to strings when sending to JavaScript
- Converting string CIDs back to u64 when sending to Rust/WASM
- Providing seamless BigInt ↔ string conversion

### Background Message Processing
Messages are processed in the background automatically, with handlers called for:
- `ConnectSuccess`/`ConnectFailure`
- `RegisterSuccess`/`RegisterFailure`
- `MessageNotification` (incoming P2P messages)
- `ServiceConnectionAccepted`
- All other response types

### Error Handling
Comprehensive error handling with:
- Automatic timeout detection
- Connection failure recovery
- Type-safe error messages
- Custom error handlers

### Connection Management
- Automatic WebSocket connection management
- P2P connection tracking
- Clean resource cleanup
- Connection state monitoring

## Examples

### Basic P2P Communication

```typescript
import { InternalServiceWasmClient } from 'citadel-websocket-client';

const client = new InternalServiceWasmClient({
    websocketUrl: 'ws://127.0.0.1:8080',
    messageHandler: (message) => {
        if ('MessageNotification' in message) {
            const notification = message.MessageNotification;
            const text = new TextDecoder().decode(new Uint8Array(notification.message));
            console.log(`Message from ${notification.peer_cid}: ${text}`);
        }
    }
});

await client.init();
await client.register({
    serverAddr: '127.0.0.1:8080',
    fullName: 'Test User',
    username: 'test_user',
    password: 'password123'
});

const peerCid = '16959591334653179280';
await client.openP2PConnection(peerCid);
await client.sendP2PMessage(peerCid, 'Hello from TypeScript!');
```

### Multiple Client Setup

```typescript
// Create Alice and Bob clients
const alice = new InternalServiceWasmClient({
    websocketUrl: 'ws://127.0.0.1:8080',
    messageHandler: (msg) => console.log('[Alice]', msg)
});

const bob = new InternalServiceWasmClient({
    websocketUrl: 'ws://127.0.0.1:8080',
    messageHandler: (msg) => console.log('[Bob]', msg)
});

// Initialize both
await alice.init();
await bob.init();

// Register both users
await alice.register({
    serverAddr: '127.0.0.1:8080',
    fullName: 'Alice Test',
    username: 'alice',
    password: 'password123'
});

await bob.register({
    serverAddr: '127.0.0.1:8080',
    fullName: 'Bob Test',
    username: 'bob',
    password: 'password456'
});

// Establish P2P connections
const aliceCid = alice.getCurrentCid()!;
const bobCid = bob.getCurrentCid()!;

await alice.openP2PConnection(bobCid);
await bob.openP2PConnection(aliceCid);

// Exchange messages
await alice.sendP2PMessage(bobCid, 'Hello Bob!');
await bob.sendP2PMessage(aliceCid, 'Hi Alice!');
```

## Building and Testing

```bash
# Build the WASM client and TypeScript wrapper
npm run setup-wasm

# Build only TypeScript
npm run build

# Test the WASM client
npm run test-wasm

# Development mode with WASM
npm run dev-wasm

# Clean build artifacts
npm run clean
```

## Troubleshooting

### Common Issues

1. **WASM module not found**: Ensure you've run `npm run build-wasm` to compile the WASM module
2. **Connection failures**: Verify the Citadel internal service is running on the specified WebSocket URL
3. **BigInt errors**: The client should handle this automatically, but ensure CIDs are passed as strings
4. **Registration failures**: Check server address and ensure the Citadel server is accessible

### Debug Logging

Enable debug logging by setting message and error handlers:

```typescript
const client = new InternalServiceWasmClient({
    websocketUrl: 'ws://127.0.0.1:8080',
    messageHandler: (message) => {
        console.log('Message:', JSON.stringify(message, null, 2));
    },
    errorHandler: (error) => {
        console.error('Error:', error.message);
    }
});
```

## Type Safety

The client provides full TypeScript type safety for:
- All request and response types
- Configuration options
- Message handlers
- Error handling
- Method parameters and return values

All types are generated from the Rust codebase ensuring consistency between the WASM module and TypeScript wrapper. 