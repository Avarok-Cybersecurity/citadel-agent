#!/bin/bash

# Automated TypeScript Type Generation Script
# This script builds the Rust crate and generates TypeScript types automatically

set -e  # Exit on any error

# Detect OS for sed compatibility
if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS
    SED_INPLACE="sed -i ''"
else
    # Linux
    SED_INPLACE="sed -i"
fi

echo "🔧 Building Rust crate with TypeScript features..."
cd citadel-internal-service-types
cargo build --features typescript

echo "📝 Generating TypeScript types with proper imports..."
TS_RS_EXPORT_DIR=../typescript-client/src/types cargo run --example generate_ts_types --features typescript

echo "🔧 Fixing missing imports in generated TypeScript files..."
cd ../typescript-client/src/types

# Helper function to add import at beginning of file (cross-platform)
add_import() {
    local file=$1
    local import_line=$2

    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS sed
        sed -i '' "1i\\
$import_line
" "$file"
    else
        # Linux sed
        sed -i "1i$import_line" "$file"
    fi
}

# Fix AccountInformation.ts
if [ -f "AccountInformation.ts" ]; then
    if ! grep -q "import.*PeerSessionInformation" AccountInformation.ts; then
        add_import "AccountInformation.ts" 'import type { PeerSessionInformation } from "./PeerSessionInformation";'
    fi
fi

# Fix Accounts.ts
if [ -f "Accounts.ts" ]; then
    if ! grep -q "import.*AccountInformation" Accounts.ts; then
        add_import "Accounts.ts" 'import type { AccountInformation } from "./AccountInformation";'
    fi
fi

# Fix ListAllPeersResponse.ts
if [ -f "ListAllPeersResponse.ts" ]; then
    if ! grep -q "import.*PeerInformation" ListAllPeersResponse.ts; then
        add_import "ListAllPeersResponse.ts" 'import type { PeerInformation } from "./PeerInformation";'
    fi
fi

# Fix ListRegisteredPeersResponse.ts
if [ -f "ListRegisteredPeersResponse.ts" ]; then
    if ! grep -q "import.*PeerInformation" ListRegisteredPeersResponse.ts; then
        add_import "ListRegisteredPeersResponse.ts" 'import type { PeerInformation } from "./PeerInformation";'
    fi
fi

# Fix SessionInformation.ts
if [ -f "SessionInformation.ts" ]; then
    if ! grep -q "import.*PeerSessionInformation" SessionInformation.ts; then
        add_import "SessionInformation.ts" 'import type { PeerSessionInformation } from "./PeerSessionInformation";'
    fi
fi

echo "📦 Creating index.ts file for convenient imports..."
cat > index.ts << 'EOF'
// Auto-generated index for all TypeScript types
// This provides a convenient single import point for all types

export * from './InternalServiceRequest';
export * from './InternalServiceResponse';
export * from './InternalServicePayload';

// Export all individual types
export * from './AccountInformation';
export * from './Accounts';
export * from './ConnectFailure';
export * from './ConnectSuccess';
export * from './DeleteVirtualFileFailure';
export * from './DeleteVirtualFileSuccess';
export * from './DisconnectFailure';
export * from './DisconnectNotification';
export * from './DownloadFileFailure';
export * from './DownloadFileSuccess';
export * from './FileTransferRequestNotification';
export * from './FileTransferStatusNotification';
export * from './FileTransferTickNotification';
export * from './GetSessionsResponse';
export * from './GroupBroadcastHandleFailure';
export * from './GroupChannelCreateFailure';
export * from './GroupChannelCreateSuccess';
export * from './GroupCreateFailure';
export * from './GroupCreateSuccess';
export * from './GroupDisconnectNotification';
export * from './GroupEndFailure';
export * from './GroupEndNotification';
export * from './GroupEndSuccess';
export * from './GroupInviteFailure';
export * from './GroupInviteNotification';
export * from './GroupInviteSuccess';
export * from './GroupJoinRequestNotification';
export * from './GroupKickFailure';
export * from './GroupKickSuccess';
export * from './GroupLeaveFailure';
export * from './GroupLeaveNotification';
export * from './GroupLeaveSuccess';
export * from './GroupListGroupsFailure';
export * from './GroupListGroupsResponse';
export * from './GroupListGroupsSuccess';
export * from './GroupMemberStateChangeNotification';
export * from './GroupMembershipResponse';
export * from './GroupMessageFailure';
export * from './GroupMessageNotification';
export * from './GroupMessageResponse';
export * from './GroupMessageSuccess';
export * from './GroupRequestJoinAcceptResponse';
export * from './GroupRequestJoinDeclineResponse';
export * from './GroupRequestJoinFailure';
export * from './GroupRequestJoinPendingNotification';
export * from './GroupRequestJoinSuccess';
export * from './GroupRespondRequestFailure';
export * from './GroupRespondRequestSuccess';
export * from './ListAllPeersFailure';
export * from './ListAllPeersResponse';
export * from './ListRegisteredPeersFailure';
export * from './ListRegisteredPeersResponse';
export * from './LocalDBClearAllKVFailure';
export * from './LocalDBClearAllKVSuccess';
export * from './LocalDBDeleteKVFailure';
export * from './LocalDBDeleteKVSuccess';
export * from './LocalDBGetAllKVFailure';
export * from './LocalDBGetAllKVSuccess';
export * from './LocalDBGetKVFailure';
export * from './LocalDBGetKVSuccess';
export * from './LocalDBSetKVFailure';
export * from './LocalDBSetKVSuccess';
export * from './MessageNotification';
export * from './MessageSendFailure';
export * from './MessageSendSuccess';
export * from './PeerConnectFailure';
export * from './PeerConnectNotification';
export * from './PeerConnectSuccess';
export * from './PeerDisconnectFailure';
export * from './PeerDisconnectSuccess';
export * from './PeerInformation';
export * from './PeerRegisterFailure';
export * from './PeerRegisterNotification';
export * from './PeerRegisterSuccess';
export * from './PeerSessionInformation';
export * from './RegisterFailure';
export * from './RegisterSuccess';
export * from './SendFileRequestFailure';
export * from './SendFileRequestSuccess';
export * from './ServiceConnectionAccepted';
export * from './SessionInformation';
EOF

echo "🎉 TypeScript types generated successfully!"
echo "📁 Types are available in: typescript-client/src/types/"
echo "📦 All imports automatically fixed!"
echo "🏗️  Index file created for convenient imports!"
echo ""
echo "Note: Run npm install && npm run build in typescript-client/ to compile"
echo "      (This is handled automatically by sync-wasm-clients.sh)" 