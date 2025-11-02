# Internal Service Requests Documentation

This document catalogs all `InternalServiceRequest` variants, their handlers, purposes, and resource cleanup requirements.

## Request Types

| Name | Handler Path | Purpose | Cleanup | Notes |
|------|-------------|---------|---------|-------|
| **Connect** | `src/kernel/requests/connect.rs` | Authenticate and connect user to hypernode | Removes existing sessions for username before connecting (line 37-55); Session stored in `server_connection_map` (line 125-128) | Pre-connect cleanup prevents "Session Already Connected" errors; Connection stream reader spawned but does NOT clean up session on close |
| **Register** | `src/kernel/requests/register.rs` | Register new user account on hypernode | N/A (delegates to Connect if `connect_after_register=true`) | Registration itself doesn't create resources; subsequent Connect does |
| **Disconnect** | `src/kernel/requests/disconnect.rs` | Explicitly disconnect from hypernode | Removes session from `server_connection_map` (line 24); Sends DisconnectFromHypernode to protocol | Primary cleanup point for explicit disconnections |
| **Message** | `src/kernel/requests/message.rs` | Send message to server or peer | N/A | Stateless operation; no resources allocated |
| **SendFile** | `src/kernel/requests/file/upload.rs` | Upload file to server or peer | File transfer handler stored in `Connection::c2s_file_transfer_handlers` or peer handler map | Handler cleaned up on transfer complete via `clear_object_transfer_handler()` |
| **RespondFileTransfer** | `src/kernel/requests/file/respond_file_transfer.rs` | Accept/reject incoming file transfer | Creates ObjectTransferHandler on accept | Handler auto-removed on completion |
| **DownloadFile** | `src/kernel/requests/file/download.rs` | Download file from virtual directory | Creates pull-based file transfer | Transfer state cleaned up on completion |
| **DeleteVirtualFile** | `src/kernel/requests/file/delete_virtual_file.rs` | Delete file from virtual directory | N/A | Stateless operation |
| **PeerConnect** | `src/kernel/requests/peer/connect.rs` | Establish P2P connection with peer | Checks if peer already connected (line 34-42); Creates `PeerConnection` in `Connection::peers` map | Peer connection persists until PeerDisconnect |
| **PeerDisconnect** | `src/kernel/requests/peer/disconnect.rs` | Disconnect from peer | Removes from `Connection::peers` via `clear_peer_connection()` | Proper cleanup implemented |
| **PeerRegister** | `src/kernel/requests/peer/register.rs` | Register peer for future P2P | N/A (registration only, no connection) | May connect afterward if `connect_after_register=true` |
| **ListAllPeers** | `src/kernel/requests/peer/list_all.rs` | List all peers (registered + online) | N/A | Read-only query |
| **ListRegisteredPeers** | `src/kernel/requests/peer/list_registered.rs` | List registered peers only | N/A | Read-only query |
| **GroupCreate** | `src/kernel/requests/group/create.rs` | Create new message group | Group channel stored in `Connection::groups` | **POTENTIAL LEAK**: No explicit cleanup found for groups |
| **GroupInvite** | `src/kernel/requests/group/invite.rs` | Invite user to group | N/A | Stateless invitation |
| **GroupRequestJoin** | `src/kernel/requests/group/request_join.rs` | Request to join group | N/A | Stateless request |
| **GroupRespondRequest** | `src/kernel/requests/group/respond_request.rs` | Accept/reject join request | N/A | Stateless response |
| **GroupMessage** | `src/kernel/requests/group/message.rs` | Send message to group | N/A | Stateless operation |
| **GroupLeave** | `src/kernel/requests/group/leave.rs` | Leave group | Should remove from `Connection::groups` | Cleanup TBD - needs verification |
| **GroupEnd** | `src/kernel/requests/group/end.rs` | End/close group | Should remove from `Connection::groups` | Cleanup TBD - needs verification |
| **GroupKick** | `src/kernel/requests/group/kick.rs` | Kick member from group | N/A (removes on other end) | Stateless for requester |
| **GroupListGroups** | `src/kernel/requests/group/group_list_groups.rs` | List all groups | N/A | Read-only query |
| **LocalDBGetKV** | `src/kernel/requests/local_db/get_kv.rs` | Get key-value pair from LocalDB | N/A | Read-only operation |
| **LocalDBSetKV** | `src/kernel/requests/local_db/set_kv.rs` | Set key-value pair in LocalDB | N/A | Database operation, persisted |
| **LocalDBDeleteKV** | `src/kernel/requests/local_db/delete_kv.rs` | Delete key from LocalDB | Calls `remote.remove(&key)` (line 40) | Proper cleanup |
| **LocalDBGetAllKV** | `src/kernel/requests/local_db/get_all_kv.rs` | Get all KV pairs from LocalDB | N/A | Read-only operation |
| **LocalDBClearAllKV** | `src/kernel/requests/local_db/clear_all_kv.rs` | Clear all KV pairs from LocalDB | Calls `remote.clear_all()` | Bulk cleanup operation |
| **GetSessions** | `src/kernel/requests/get_sessions.rs` | Query all active sessions | N/A | Read-only diagnostic query |
| **GetAccountInformation** | `src/kernel/requests/get_account_information.rs` | Get account details | N/A | Read-only query |
| **ConnectionManagement** | `src/kernel/requests/connection_management.rs` | Manage orphan sessions (claim, disconnect, list) | `ClaimSession`: N/A; `DisconnectOrphanSession`: Removes from `server_connection_map` (line 82, 111) | Specialized cleanup for orphan mode |

## Resource Lifecycle Summary

### Session Resources (`server_connection_map`)
- **Created**: `connect.rs:125-128` after successful connection
- **Cleaned up**:
  1. `disconnect.rs:24` - Explicit disconnect request
  2. `ext.rs:89` - TCP connection drop (if not in orphan mode)
  3. `connect.rs:37-55` - Pre-connect cleanup for same username (**NEW**)
  4. `connection_management.rs:82,111` - Orphan session disconnect

### P2P Resources (`Connection::peers`)
- **Created**: `peer/connect.rs` after successful P2P connection
- **Cleaned up**: `peer/disconnect.rs` via `clear_peer_connection()`

### File Transfer Resources
- **Created**: File upload/download requests create handlers
- **Cleaned up**: Auto-removed via `clear_object_transfer_handler()` on completion

### Group Resources (`Connection::groups`)
- **Created**: `group/create.rs`
- **Cleaned up**: ⚠️ **Needs verification** - GroupLeave/GroupEnd should clean up

## Critical Notes

1. **No RAII Pattern**: No `Drop` implementations found. All cleanup is manual via `.remove()` calls.

2. **Session Cleanup Race Condition FIXED**:
   - Removed redundant cleanup in spawned task (`connect.rs:139`)
   - Added pre-connect cleanup by username (`connect.rs:37-55`)
   - This prevents "Session Already Connected" errors

3. **Orphan Mode**: Allows sessions to persist when TCP drops, enabling reconnection without re-authentication.

4. **Group Cleanup TODO**: Verify that GroupLeave/GroupEnd properly clean up `Connection::groups` HashMap.

## Best Practices

1. **Always cleanup in request handlers**, not in background tasks
2. **Check for existing resources** before creating new ones (see PeerConnect:34-42)
3. **Use orphan mode** for reconnection scenarios instead of creating duplicate sessions
4. **Document cleanup points** when adding new resource-holding requests
