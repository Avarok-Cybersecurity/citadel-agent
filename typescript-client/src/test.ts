import { CitadelClient } from './CitadelClient';
import { MessageNotification } from './types';

async function runTests() {
    console.log('🚀 Starting Citadel WebSocket TypeScript Tests...\n');

    // Test configuration
    const serverUrl = 'ws://127.0.0.1:8081';
    const client = new CitadelClient({
        url: serverUrl,
        username: 'typescript_user',
        password: 'typescript_password',
        timeout: 10000
    });

    try {
        // Setup message handler
        setupEventHandlers(client);

        // Test 1: Connect and authenticate
        console.log('🔐 Test 1: Connecting and authenticating...');
        const connectResult = await client.connect({
            connectMode: { Standard: { force_login: false } },
            udpMode: "Disabled",
            keepAliveTimeout: { secs: 30, nanos: 0 },
            sessionSecuritySettings: {
                security_level: "Standard",
                secrecy_mode: "BestEffort",
                crypto_params: {
                    encryption_algorithm: "AES_GCM_256",
                    kem_algorithm: "Kyber",
                    sig_algorithm: "None"
                },
                header_obfuscator_settings: "Disabled"
            }
        });

        console.log('✅ Connect request successful!');
        console.log('   CID:', connectResult.cid);
        console.log('   Request ID:', connectResult.request_id);
        console.log('');

        // Test 2: Send a message
        console.log('💬 Test 2: Sending a message...');
        const messageResult = await client.sendMessage('Hello from TypeScript client!');

        console.log('✅ Message request successful!');
        console.log('   CID:', messageResult.cid);
        console.log('   Request ID:', messageResult.request_id);
        console.log('');

        // Test 3: Test multiple messages
        console.log('🔄 Test 3: Testing multiple messages...');
        for (let i = 1; i <= 3; i++) {
            const result = await client.sendMessage(`Message ${i} from TypeScript`);
            console.log(`   Message ${i} sent successfully (CID: ${result.cid})`);
        }
        console.log('✅ Multiple messages sent successfully!\n');

        // Test 4: Disconnect
        console.log('🔌 Test 4: Disconnecting from server...');
        await client.disconnect();
        console.log('✅ Successfully disconnected\n');

        console.log('🎉 All tests completed successfully!');

    } catch (error) {
        console.error('❌ Test failed:', error);
        process.exit(1);
    }
}

// Handle event listeners
function setupEventHandlers(client: CitadelClient) {
    client.onMessage((notification: MessageNotification) => {
        const messageStr = Buffer.from(notification.message).toString('utf-8');
        console.log('📨 Received message notification:');
        console.log('   From CID:', notification.peer_cid);
        console.log('   Message:', messageStr);
        console.log('   Request ID:', notification.request_id);
    });
}

// Main execution
if (require.main === module) {
    runTests().catch((error) => {
        console.error('❌ Fatal error:', error);
        process.exit(1);
    });
}

export { runTests, setupEventHandlers }; 