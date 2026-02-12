import { InternalServiceWasmClient, InternalServiceResponse, isResponseType } from './index';

/**
 * Example usage of the InternalServiceWasmClient
 */
async function wasmClientExample() {
    // Create the WASM client
    const client = new InternalServiceWasmClient({
        websocketUrl: 'ws://127.0.0.1:8080',
        messageHandler: (message: InternalServiceResponse) => {
            console.log('Received message:', message);

            // Handle different message types
            if (isResponseType(message, 'MessageNotification')) {
                const notification = message.MessageNotification;
                const messageText = new TextDecoder().decode(new Uint8Array(notification.message));
                console.log(`Message from peer ${notification.peer_cid}: ${messageText}`);
            } else if (isResponseType(message, 'ConnectSuccess')) {
                console.log(`Connected with CID: ${message.ConnectSuccess.cid}`);
            } else if (isResponseType(message, 'RegisterSuccess')) {
                console.log(`Registered with CID: ${message.RegisterSuccess.cid}`);
            }
        },
        errorHandler: (error: Error) => {
            console.error('WASM Client Error:', error);
        }
    });

    try {
        // Step 1: Initialize the WASM client
        console.log('Initializing WASM client...');
        await client.init();
        console.log(`WASM Client version: ${client.getVersion()}`);

        // Step 2: Register a new user (optional)
        console.log('Registering user...');
        await client.register({
            request_id: InternalServiceWasmClient.generateUUID(),
            server_addr: '127.0.0.1:8080', // This should be the actual Citadel server address
            full_name: 'Alice Test User',
            username: 'alice_test',
            proposed_password: Array.from(new TextEncoder().encode('password123')),
            connect_after_register: true,
            session_security_settings: {
                security_level: 'Standard',
                secrecy_mode: 'BestEffort',
                crypto_params: {
                    encryption_algorithm: 'AES_GCM_256',
                    kem_algorithm: 'Kyber',
                    sig_algorithm: 'None'
                },
                header_obfuscator_settings: 'Disabled'
            },
            server_password: null
        });

        // Alternative: Connect with existing user
        // console.log('Connecting to service...');
        // await client.connect({
        //     username: 'alice_test',
        //     password: 'password123'
        // });

        console.log(`Connected! Current CID: ${client.getCurrentCid()}`);

        // Step 3: Open P2P connection to another peer
        const peerCid = '16959591334653179280'; // Example CID of another peer
        console.log(`Opening P2P connection to peer ${peerCid}...`);
        await client.openP2PConnection(peerCid);

        // Step 4: Send a P2P message
        console.log('Sending P2P message...');
        await client.sendP2PMessage({
            request_id: InternalServiceWasmClient.generateUUID(),
            message: Array.from(new TextEncoder().encode('Hello from TypeScript WASM client!')),
            cid: BigInt(client.getCurrentCid()!),
            peer_cid: BigInt(peerCid),
            security_level: 'Standard'
        });

        // Step 5: Keep the client running to receive messages
        console.log('Client is running. Press Ctrl+C to exit.');

        // In a real application, you might want to keep the process alive
        // process.stdin.setRawMode(true);
        // process.stdin.resume();
        // process.stdin.on('data', () => process.exit(0));

        // For demo purposes, wait a bit then close
        setTimeout(async () => {
            console.log('Closing client...');
            await client.close();
            console.log('Client closed.');
        }, 30000); // Close after 30 seconds

    } catch (error) {
        console.error('Example failed:', error);
        await client.close();
    }
}

/**
 * P2P Communication Example with two clients
 */
async function p2pExample() {
    console.log('Starting P2P communication example...');

    // Create two clients (Alice and Bob)
    const alice = new InternalServiceWasmClient({
        websocketUrl: 'ws://127.0.0.1:8080',
        messageHandler: (message: InternalServiceResponse) => {
            if (isResponseType(message, 'MessageNotification')) {
                const notification = message.MessageNotification;
                const messageText = new TextDecoder().decode(new Uint8Array(notification.message));
                console.log(`[Alice] Received from ${notification.peer_cid}: ${messageText}`);
            }
        }
    });

    const bob = new InternalServiceWasmClient({
        websocketUrl: 'ws://127.0.0.1:8080',
        messageHandler: (message: InternalServiceResponse) => {
            if (isResponseType(message, 'MessageNotification')) {
                const notification = message.MessageNotification;
                const messageText = new TextDecoder().decode(new Uint8Array(notification.message));
                console.log(`[Bob] Received from ${notification.peer_cid}: ${messageText}`);
            }
        }
    });

    try {
        // Initialize both clients
        await alice.init();
        await bob.init();

        // Register both users
        await alice.register({
            request_id: InternalServiceWasmClient.generateUUID(),
            server_addr: '127.0.0.1:8080',
            full_name: 'Alice Test',
            username: 'alice_test',
            proposed_password: Array.from(new TextEncoder().encode('password123')),
            connect_after_register: true,
            session_security_settings: {
                security_level: 'Standard',
                secrecy_mode: 'BestEffort',
                crypto_params: {
                    encryption_algorithm: 'AES_GCM_256',
                    kem_algorithm: 'Kyber',
                    sig_algorithm: 'None'
                },
                header_obfuscator_settings: 'Disabled'
            },
            server_password: null
        });

        await bob.register({
            request_id: InternalServiceWasmClient.generateUUID(),
            server_addr: '127.0.0.1:8080',
            full_name: 'Bob Test',
            username: 'bob_test',
            proposed_password: Array.from(new TextEncoder().encode('password456')),
            connect_after_register: true,
            session_security_settings: {
                security_level: 'Standard',
                secrecy_mode: 'BestEffort',
                crypto_params: {
                    encryption_algorithm: 'AES_GCM_256',
                    kem_algorithm: 'Kyber',
                    sig_algorithm: 'None'
                },
                header_obfuscator_settings: 'Disabled'
            },
            server_password: null
        });

        const aliceCid = alice.getCurrentCid()!;
        const bobCid = bob.getCurrentCid()!;

        console.log(`Alice CID: ${aliceCid}, Bob CID: ${bobCid}`);

        // Establish P2P connections
        await alice.openP2PConnection(bobCid);
        await bob.openP2PConnection(aliceCid);

        // Exchange messages
        await alice.sendP2PMessage({
            request_id: InternalServiceWasmClient.generateUUID(),
            message: Array.from(new TextEncoder().encode('Hello Bob, this is Alice!')),
            cid: BigInt(aliceCid),
            peer_cid: BigInt(bobCid),
            security_level: 'Standard'
        });
        await new Promise(resolve => setTimeout(resolve, 1000)); // Wait a bit

        await bob.sendP2PMessage({
            request_id: InternalServiceWasmClient.generateUUID(),
            message: Array.from(new TextEncoder().encode('Hi Alice, nice to meet you!')),
            cid: BigInt(bobCid),
            peer_cid: BigInt(aliceCid),
            security_level: 'Standard'
        });
        await new Promise(resolve => setTimeout(resolve, 1000)); // Wait a bit

        console.log('P2P communication example completed!');

        // Clean up
        setTimeout(async () => {
            await alice.close();
            await bob.close();
            console.log('Both clients closed.');
        }, 5000);

    } catch (error) {
        console.error('P2P example failed:', error);
        await alice.close();
        await bob.close();
    }
}

// Export the example functions
export { wasmClientExample, p2pExample };

// If this file is run directly, execute the basic example
if (require.main === module) {
    wasmClientExample().catch(console.error);
} 