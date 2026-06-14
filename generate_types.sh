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

# Fix protocol type imports from @avarok/citadel-protocol-types
# ts-rs generates type references (e.g., connect_mode: ConnectMode) via #[ts(type = "...")]
# but does NOT generate the import statements for external packages.
echo "🔧 Fixing @avarok/citadel-protocol-types imports in generated files..."
PROTOCOL_TYPES=("ConnectMode" "UdpMode" "SessionSecuritySettings" "SecurityLevel" "TransferType" "ObjectId" "PreSharedKey" "MessageGroupKey" "UserIdentifier" "VirtualObjectMetadata" "ObjectTransferStatus" "MemberState")

for ts_file in *.ts; do
    [ "$ts_file" = "index.ts" ] && continue

    # Find which protocol types are used in this file (excluding existing imports)
    needed_types=()
    for ptype in "${PROTOCOL_TYPES[@]}"; do
        if grep -q "$ptype" "$ts_file" && ! grep -q "import.*$ptype.*@avarok/citadel-protocol-types" "$ts_file"; then
            needed_types+=("$ptype")
        fi
    done

    # Add import if any protocol types are needed
    if [ ${#needed_types[@]} -gt 0 ]; then
        types_str=$(IFS=", "; echo "${needed_types[*]}")
        import_line="import type { $types_str } from '@avarok/citadel-protocol-types';"
        echo "  Adding import to $ts_file: { $types_str }"
        add_import "$ts_file" "$import_line"
    fi
done

echo "📦 Creating index.ts file for convenient imports..."
cat > index.ts << 'EOF'
// Auto-generated index for all TypeScript types
// This provides a convenient single import point for all types

export * from './InternalServiceRequest.js';
export * from './InternalServiceResponse.js';
export * from './InternalServicePayload.js';

// Export all individual types
export * from './AccountInformation.js';
export * from './Accounts.js';
export * from './BatchedResponseData.js';
export * from './ConfigCommand.js';
export * from './ConnectFailure.js';
export * from './ConnectSuccess.js';
export * from './ConnectionManagementFailure.js';
export * from './ConnectionManagementSuccess.js';
export * from './DeleteVirtualFileFailure.js';
export * from './DeleteVirtualFileSuccess.js';
export * from './DeregisterFailure.js';
export * from './DeregisterSuccess.js';
export * from './DisconnectFailure.js';
export * from './DisconnectNotification.js';
export * from './DownloadFileFailure.js';
export * from './DownloadFileSuccess.js';
export * from './FileSource.js';
export * from './FileTransferRequestNotification.js';
export * from './FileTransferStatusNotification.js';
export * from './FileTransferTickNotification.js';
export * from './GetSessionsResponse.js';
export * from './GroupBroadcastHandleFailure.js';
export * from './GroupChannelCreateFailure.js';
export * from './GroupChannelCreateSuccess.js';
export * from './GroupCreateFailure.js';
export * from './GroupCreateSuccess.js';
export * from './GroupDisconnectNotification.js';
export * from './GroupEndFailure.js';
export * from './GroupEndNotification.js';
export * from './GroupEndSuccess.js';
export * from './GroupInviteFailure.js';
export * from './GroupInviteNotification.js';
export * from './GroupInviteSuccess.js';
export * from './GroupJoinRequestNotification.js';
export * from './GroupKickFailure.js';
export * from './GroupKickSuccess.js';
export * from './GroupLeaveFailure.js';
export * from './GroupLeaveNotification.js';
export * from './GroupLeaveSuccess.js';
export * from './GroupListGroupsFailure.js';
export * from './GroupListGroupsResponse.js';
export * from './GroupListGroupsSuccess.js';
export * from './GroupMemberStateChangeNotification.js';
export * from './GroupMembershipResponse.js';
export * from './GroupMessageFailure.js';
export * from './GroupMessageNotification.js';
export * from './GroupMessageResponse.js';
export * from './GroupMessageSuccess.js';
export * from './GroupRequestJoinAcceptResponse.js';
export * from './GroupRequestJoinDeclineResponse.js';
export * from './GroupRequestJoinFailure.js';
export * from './GroupRequestJoinPendingNotification.js';
export * from './GroupRequestJoinSuccess.js';
export * from './GroupRespondRequestFailure.js';
export * from './GroupRespondRequestSuccess.js';
export * from './ListAllPeersFailure.js';
export * from './ListAllPeersResponse.js';
export * from './ListRegisteredPeersFailure.js';
export * from './ListRegisteredPeersResponse.js';
export * from './LocalDBClearAllKVFailure.js';
export * from './LocalDBClearAllKVSuccess.js';
export * from './LocalDBDeleteKVFailure.js';
export * from './LocalDBDeleteKVSuccess.js';
export * from './LocalDBGetAllKVFailure.js';
export * from './LocalDBGetAllKVSuccess.js';
export * from './LocalDBGetKVFailure.js';
export * from './LocalDBGetKVSuccess.js';
export * from './LocalDBSetKVFailure.js';
export * from './LocalDBSetKVSuccess.js';
export * from './MessageNotification.js';
export * from './MessageSendFailure.js';
export * from './MessageSendSuccess.js';
export * from './PeerConnectAcceptFailure.js';
export * from './PeerConnectAcceptSuccess.js';
export * from './PeerConnectFailure.js';
export * from './PeerConnectNotification.js';
export * from './PeerConnectSuccess.js';
export * from './PeerDisconnectFailure.js';
export * from './PeerDisconnectSuccess.js';
export * from './PeerInformation.js';
export * from './PeerRegisterFailure.js';
export * from './PeerRegisterNotification.js';
export * from './PeerRegisterSuccess.js';
export * from './PeerSessionInformation.js';
export * from './PickFileFailure.js';
export * from './PickFileSuccess.js';
export * from './RegisterFailure.js';
export * from './RegisterSuccess.js';
export * from './SendFileRequestFailure.js';
export * from './SendFileRequestSuccess.js';
export * from './ServiceConnectionAccepted.js';
export * from './SessionAlreadyActive.js';
export * from './SessionInformation.js';

// Re-export protocol types used in InternalServiceRequest fields
// so downstream packages can reference them directly.
export type { ConnectMode, UdpMode, SessionSecuritySettings, SecurityLevel, TransferType, ObjectId, PreSharedKey, MessageGroupKey, UserIdentifier } from '@avarok/citadel-protocol-types';
EOF

echo "🎉 TypeScript types generated successfully!"
echo "📁 Types are available in: typescript-client/src/types/"
echo "📦 All imports automatically fixed!"
echo "🏗️  Index file created for convenient imports!"
echo ""
echo "Note: Run npm install && npm run build in typescript-client/ to compile"
echo "      (This is handled automatically by sync-wasm-clients.sh)" 