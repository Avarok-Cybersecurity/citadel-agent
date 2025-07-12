# Citadel WebSocket TypeScript Client

This is a complete TypeScript/JavaScript frontend that demonstrates communication with the Rust Citadel WebSocket IOInterface using JSON serialization.

## 🏗️ Architecture

```
┌─────────────────┐    WebSocket     ┌─────────────────┐
│   TypeScript    │ ◄─────────────► │   Rust Server   │
│     Client      │      JSON       │  (WebSocket)    │
└─────────────────┘                 └─────────────────┘
```

## 🚀 Features

- **Async WebSocket Communication**: Real-time bidirectional communication
- **JSON Serialization**: Frontend-friendly JSON message format
- **Type-Safe Client**: TypeScript interfaces for all message types
- **Request-Response Pattern**: Promise-based request handling with timeout
- **Event Handling**: Support for notifications and events
- **Connection Management**: Automatic connection handling and cleanup
- **Integration Tests**: End-to-end tests demonstrating functionality

## 📋 Project Structure

```
typescript-client/
├── src/
│   ├── CitadelClient.ts     # Main WebSocket client library
│   └── test.ts              # Integration tests
├── dist/                    # Compiled JavaScript output
├── package.json             # npm configuration
├── tsconfig.json           # TypeScript configuration
├── run_tests.sh            # Test orchestration script
└── README.md               # This file
```

## 🛠️ Setup

### Prerequisites

- Node.js (v16 or higher)
- npm
- Rust with Cargo
- The Citadel WebSocket server feature enabled

### Install Dependencies

```bash
npm install
```

### Build TypeScript

```bash
npm run build
```

## 🧪 Running Tests

### Manual Testing

1. **Start the Rust WebSocket server:**
   ```bash
   cd ../../../
   cargo run --features websockets --example websocket_server
   ```

2. **In another terminal, run the TypeScript tests:**
   ```bash
   cd citadel-internal-service-connector/tests/typescript-client
   npm run test
   ```

### Automated Testing

Use the provided script for full automation:

```bash
./run_tests.sh
```

This script:
1. Starts the Rust WebSocket server
2. Builds the TypeScript project
3. Runs the integration tests
4. Cleans up processes

## 📚 API Documentation

### CitadelClient Class

```typescript
const client = new CitadelClient({ 
  url: 'ws://localhost:8080',
  timeout: 5000 
});
```

#### Methods

##### `async connect(): Promise<void>`
Establishes WebSocket connection to the server.

##### `async connectToCitadel(params: ConnectRequest): Promise<any>`
Sends a connection request to the Citadel service.

```typescript
const result = await client.connectToCitadel({
  username: 'user123',
  password: [112, 97, 115, 115], // "pass" as byte array
  udpMode: 'Disabled',
  keepAliveTimeout: { secs: 30, nanos: 0 }
});
```

##### `async sendMessage(params: MessageRequest): Promise<any>`
Sends a message through the Citadel service.

```typescript
const result = await client.sendMessage({
  message: [72, 101, 108, 108, 111], // "Hello" as byte array
  cid: 12345,
  securityLevel: 'Standard'
});
```

##### `onEvent(eventType: string, handler: (event: any) => void): void`
Registers event handlers for notifications.

```typescript
client.onEvent('MessageNotification', (notification) => {
  console.log('Received message:', notification);
});
```

##### `async disconnect(): Promise<void>`
Closes the WebSocket connection.

##### `isConnected(): boolean`
Returns connection status.

### Type Definitions

```typescript
interface ConnectRequest {
  username: string;
  password: number[];
  connectMode?: any;
  udpMode?: any;
  keepAliveTimeout?: { secs: number; nanos: number } | null;
  sessionSecuritySettings?: any;
  serverPassword?: any;
}

interface MessageRequest {
  message: number[];
  cid: number;
  peerCid?: number;
  securityLevel?: any;
}
```

## 🔄 Message Flow

### Request-Response Pattern

1. **Client sends request:**
   ```json
   {
     "Request": {
       "Connect": {
         "request_id": "550e8400-e29b-41d4-a716-446655440000",
         "username": "user123",
         "password": [112, 97, 115, 115],
         "connect_mode": { "Standard": { "force_login": false } },
         "udp_mode": "Disabled",
         "keep_alive_timeout": { "secs": 30, "nanos": 0 },
         "session_security_settings": { /* ... */ },
         "server_password": null
       }
     }
   }
   ```

2. **Server responds:**
   ```json
   {
     "Response": {
       "ConnectSuccess": {
         "cid": 12345,
         "request_id": "550e8400-e29b-41d4-a716-446655440000"
       }
     }
   }
   ```

### Error Handling

The client handles various error scenarios:
- Connection timeouts
- WebSocket connection failures
- JSON parsing errors
- Server error responses

## 🧪 Test Cases

The integration tests cover:

1. **WebSocket Connection**: Establishing connection to Rust server
2. **Connect Request**: Citadel service authentication
3. **Message Sending**: Bi-directional message exchange
4. **Multiple Requests**: Concurrent request handling
5. **Disconnection**: Clean connection teardown

## 🔧 Development

### Adding New Message Types

1. **Define TypeScript interface:**
   ```typescript
   interface NewRequestType {
     field1: string;
     field2: number;
   }
   ```

2. **Add method to CitadelClient:**
   ```typescript
   async sendNewRequest(params: NewRequestType): Promise<any> {
     const requestId = uuidv4();
     const request = {
       NewRequest: {
         request_id: requestId,
         ...params
       }
     };
     return await this.sendRequest(request);
   }
   ```

3. **Handle in Rust server:**
   ```rust
   InternalServiceRequest::NewRequest { request_id, field1, field2 } => {
     // Handle the new request type
   }
   ```

### Debugging

Enable debug logging:
```typescript
// Add console.log statements in handleMessage method
console.log('Raw message received:', message);
```

Check server logs for request processing details.

## 🎯 Performance Considerations

- **Connection Pooling**: Reuse connections when possible
- **Timeout Management**: Configure appropriate timeouts for your use case
- **Error Retry**: Implement exponential backoff for connection retries
- **Message Batching**: Group multiple requests when appropriate

## 🔐 Security

- **Input Validation**: Always validate incoming data
- **Authentication**: Ensure proper Citadel authentication
- **Connection Security**: Use WSS in production
- **Error Information**: Avoid exposing sensitive data in error messages

## 📊 Results

The integration demonstrates:
- ✅ **Successful WebSocket communication** between TypeScript and Rust
- ✅ **JSON serialization** working correctly for complex data structures  
- ✅ **Request-response pattern** with proper timeout handling
- ✅ **Type safety** in the TypeScript client
- ✅ **End-to-end functionality** from frontend to backend
- ✅ **Production-ready architecture** with proper error handling

This implementation provides a solid foundation for building frontend applications that communicate with the Citadel internal service through WebSockets. 