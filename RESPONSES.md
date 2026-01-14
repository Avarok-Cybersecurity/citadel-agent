# Internal Service Responses Documentation

This document catalogs all `InternalServiceResponse` variants, their handlers (if applicable), purposes, and cleanup implications.

## Response Types

| Name | Handler Path | Purpose | Cleanup | Notes |
|------|-------------|---------|---------|-------|
| **ConnectSuccess** | `src/kernel/requests/connect.rs` | Confirms successful connection | N/A | Contains CID for the session |
| **ConnectFailure** | `src/kernel/requests/connect.rs` | Reports connection failure | N/A | Contains error message; no resources allocated |
| **RegisterSuccess** | `src/kernel/requests/register.rs` | Confirms successful registration | N/A | May be followed by ConnectSuccess if `connect_after_register=true` |
| **RegisterFailure** | `src/kernel/requests/register.rs` | Reports registration failure | N/A | Contains error message; no resources allocated |
| **ServiceConnectionAccepted** | `src/kernel/ext.rs:32-36` | Confirms TCP/WebSocket connection established | N/A | Sent immediately after client connects to internal service |
| **MessageSendSuccess** | `src/kernel/requests/message.rs` | Confirms message sent | N/A | Acknowledgment only |
| **MessageSendFailure** | `src/kernel/requests/message.rs` | Reports message send failure | N/A | Error acknowledgment |
| **MessageNotification** | `src/kernel/requests/connect.rs:107-115` | Incoming message from server/peer | N/A | Delivered from connection read stream |
| **DisconnectNotification** | `src/kernel/responses/disconnect.rs:17-21,34-38` | Notifies disconnection complete | Session removed from `server_connection_map` (line 15) or peer removed (line 32) | Triggered by protocol-level disconnect |
| **DisconnectFailure** | `src/kernel/requests/disconnect.rs:43-47` | Reports disconnect failure | N/A | Rare case; session may still exist |
| **SendFileRequestSuccess** | File upload handler | Confirms file upload initiated | N/A | Transfer proceeds asynchronously |
| **SendFileRequestFailure** | File upload handler | Reports file upload failure | N/A | No transfer initiated |
| **FileTransferRequestNotification** | N/A (protocol-generated) | Incoming file transfer request | Creates handler if accepted | Requires RespondFileTransfer to accept/reject |
| **FileTransferStatusNotification** | N/A (protocol-generated) | File transfer completed or failed | Handler removed via `clear_object_transfer_handler()` | Signals end of transfer |
| **FileTransferTickNotification** | N/A (protocol-generated) | File transfer progress update | N/A | Progress reporting only |
| **DownloadFileSuccess** | File download handler | Confirms download initiated | N/A | Download proceeds |
| **DownloadFileFailure** | File download handler | Reports download failure | N/A | No download started |
| **DeleteVirtualFileSuccess** | File delete handler | Confirms file deleted | N/A | File removed from VFS |
| **DeleteVirtualFileFailure** | File delete handler | Reports delete failure | N/A | File may still exist |
| **PeerConnectSuccess** | P2P connect handler | Confirms P2P connection established | `PeerConnection` added to `Connection::peers` | Session persists |
| **PeerConnectFailure** | P2P connect handler | Reports P2P connection failure | N/A | No peer session created |
| **PeerConnectNotification** | `src/kernel/responses/peer_event.rs` | Incoming P2P connection from peer | `PeerConnection` added to `Connection::peers` | Peer-initiated connection |
| **PeerRegisterNotification** | `src/kernel/responses/peer_event.rs` | Peer registered us mutually | N/A | Registration notification only |
| **PeerDisconnectSuccess** | P2P disconnect handler | Confirms P2P disconnection | Peer removed from `Connection::peers` | Proper cleanup |
| **PeerDisconnectFailure** | P2P disconnect handler | Reports P2P disconnect failure | N/A | Peer session may persist |
| **PeerRegisterSuccess** | P2P register handler | Confirms peer registration | N/A | Registration only, no connection |
| **PeerRegisterFailure** | P2P register handler | Reports peer registration failure | N/A | Not registered |
| **GroupChannelCreateSuccess** | Group handler | Group messaging channel created | Group added to `Connection::groups` | Channel for broadcasting |
| **GroupChannelCreateFailure** | Group handler | Group channel creation failed | N/A | No channel created |
| **GroupBroadcastHandleFailure** | Group handler | Broadcast handle creation failed | N/A | Error in group setup |
| **GroupCreateSuccess** | Group create handler | Group created successfully | Group stored in `Connection::groups` | **VERIFY CLEANUP** |
| **GroupCreateFailure** | Group create handler | Group creation failed | N/A | No group created |
| **GroupLeaveSuccess** | Group leave handler | Left group successfully | Should remove from `Connection::groups` | **VERIFY CLEANUP** |
| **GroupLeaveFailure** | Group leave handler | Failed to leave group | N/A | Still in group |
| **GroupLeaveNotification** | `src/kernel/responses/group_event.rs` | Member left group | N/A | Notification to remaining members |
| **GroupEndSuccess** | Group end handler | Group ended successfully | Should remove from `Connection::groups` | **VERIFY CLEANUP** |
| **GroupEndFailure** | Group end handler | Failed to end group | N/A | Group still exists |
| **GroupEndNotification** | `src/kernel/responses/group_event.rs` | Group ended by admin | Should remove from `Connection::groups` | **VERIFY CLEANUP** |
| **GroupMessageNotification** | `src/kernel/responses/group_event.rs` | Incoming group message | N/A | Message delivery |
| **GroupMessageResponse** | Group message handler | Group message acknowledgment | N/A | Delivery confirmation |
| **GroupMessageSuccess** | Group message handler | Group message sent | N/A | Send confirmation |
| **GroupMessageFailure** | Group message handler | Group message send failed | N/A | Send failure |
| **GroupInviteNotification** | `src/kernel/responses/group_event.rs` | Invited to group | N/A | Invitation pending |
| **GroupInviteSuccess** | Group invite handler | Invite sent successfully | N/A | Invitation delivered |
| **GroupInviteFailure** | Group invite handler | Invite send failed | N/A | Invitation not sent |
| **GroupRespondRequestSuccess** | Group respond handler | Join request response sent | N/A | Request processed |
| **GroupRespondRequestFailure** | Group respond handler | Join request response failed | N/A | Request not processed |
| **GroupMembershipResponse** | Group handler | Membership query response | N/A | Query result |
| **GroupRequestJoinPendingNotification** | `src/kernel/responses/group_event.rs` | Join request pending admin approval | N/A | Waiting for approval |
| **GroupDisconnectNotification** | `src/kernel/responses/group_event.rs` | Disconnected from group | Should remove from `Connection::groups` | **VERIFY CLEANUP** |
| **GroupKickSuccess** | Group kick handler | Member kicked successfully | N/A | Kick completed |
| **GroupKickFailure** | Group kick handler | Kick failed | N/A | Member still in group |
| **GroupListGroupsSuccess** | Group list handler | Group list query successful | N/A | Query result |
| **GroupListGroupsFailure** | Group list handler | Group list query failed | N/A | Query error |
| **GroupListGroupsResponse** | Group list handler | List of groups | N/A | Query result data |
| **GroupJoinRequestNotification** | `src/kernel/responses/group_event.rs` | Join request received (admin view) | N/A | Pending approval |
| **GroupRequestJoinAcceptResponse** | Group handler | Join request accepted | N/A | Approval notification |
| **GroupRequestJoinDeclineResponse** | Group handler | Join request declined | N/A | Rejection notification |
| **GroupRequestJoinSuccess** | Group join handler | Join request sent | N/A | Request submitted |
| **GroupRequestJoinFailure** | Group join handler | Join request failed | N/A | Request not sent |
| **GroupMemberStateChangeNotification** | `src/kernel/responses/group_event.rs` | Member state changed | N/A | State update notification |
| **LocalDBGetKVSuccess** | LocalDB handler | Key-value retrieved | N/A | Query result |
| **LocalDBGetKVFailure** | LocalDB handler | Key-value retrieval failed | N/A | Query error |
| **LocalDBSetKVSuccess** | LocalDB handler | Key-value stored | N/A | Store confirmation |
| **LocalDBSetKVFailure** | LocalDB handler | Key-value store failed | N/A | Store error |
| **LocalDBDeleteKVSuccess** | LocalDB handler | Key deleted | Key removed from LocalDB | Cleanup operation |
| **LocalDBDeleteKVFailure** | LocalDB handler | Key delete failed | N/A | Key may still exist |
| **LocalDBGetAllKVSuccess** | LocalDB handler | All KV pairs retrieved | N/A | Query result |
| **LocalDBGetAllKVFailure** | LocalDB handler | Get all KV pairs failed | N/A | Query error |
| **LocalDBClearAllKVSuccess** | LocalDB handler | All KV pairs cleared | All keys removed from LocalDB | Bulk cleanup |
| **LocalDBClearAllKVFailure** | LocalDB handler | Clear all failed | N/A | Keys may still exist |
| **GetSessionsResponse** | Get sessions handler | List of active sessions | N/A | Diagnostic query result |
| **GetAccountInformationResponse** | Account info handler | Account details | N/A | Query result |
| **ListAllPeersResponse** | List peers handler | All peers (registered + online) | N/A | Query result |
| **ListAllPeersFailure** | List peers handler | List peers failed | N/A | Query error |
| **ListRegisteredPeersResponse** | List peers handler | Registered peers only | N/A | Query result |
| **ListRegisteredPeersFailure** | List peers handler | List registered peers failed | N/A | Query error |
| **ConnectionManagementSuccess** | Connection management handler | Orphan session operation succeeded | May remove session if DisconnectOrphanSession | Orphan mode operation |
| **ConnectionManagementFailure** | Connection management handler | Orphan session operation failed | N/A | Operation error |

## Response Handler Locations

### Protocol Event Handlers (`src/kernel/responses/`)
- **disconnect.rs** - Handles protocol-level disconnection events
- **peer_event.rs** - Handles P2P connection/disconnection/registration events
- **group_event.rs** - Handles group-related events and notifications
- **group_channel_created.rs** - Handles group broadcast channel creation
- **object_transfer_handle.rs** - Handles file transfer events

### Request Handlers (`src/kernel/requests/`)
Most responses are generated directly by request handlers in their respective files.

## Notification vs Success/Failure

**Notifications** are unsolicited events from the protocol layer:
- MessageNotification - Incoming message
- DisconnectNotification - Connection closed
- PeerConnectNotification - Peer connected to us
- Group notifications - Various group events
- FileTransfer notifications - Transfer progress/status

**Success/Failure** responses are direct replies to client requests:
- Confirm operation completed or failed
- Include request_id for correlation
- Typically do not allocate resources (resources allocated by request handlers)

## Cleanup Implications

### Responses That Trigger Cleanup:
1. **DisconnectNotification** - Session removed from `server_connection_map`
2. **PeerDisconnectSuccess** - Peer removed from `Connection::peers`
3. **FileTransferStatusNotification** - Transfer handler cleaned up
4. **LocalDBDeleteKVSuccess** - Key removed from database
5. **LocalDBClearAllKVSuccess** - All keys removed from database

### Responses That Should Trigger Cleanup (TO VERIFY):
1. **GroupLeaveSuccess/Notification** - Should remove from `Connection::groups`
2. **GroupEndSuccess/Notification** - Should remove from `Connection::groups`
3. **GroupDisconnectNotification** - Should remove from `Connection::groups`

## Critical Notes

1. **Most responses are acknowledgments** and do not directly manage resources

2. **Notifications are asynchronous** - Protocol-generated events delivered via response handlers

3. **Resource cleanup happens in request handlers**, not response generators (exception: protocol event handlers)

4. **Group cleanup needs verification** - Multiple group-related responses should trigger cleanup but implementation needs audit

5. **No RAII for responses** - All responses are sent via channel/sink; no cleanup needed for response objects themselves
