# 🛡️ Citadel Internal Service

The **Citadel Internal Service** (also referred to as the **Citadel Agent**) is a local daemon (`citadeld`) that manages the lifecycle of secure connections for developer applications. 

It sits architecturally between your client applications (CLI, GUI, Web/WASM, etc) and the Citadel Protocol SDK. By exposing a lightweight IPC interface (UDS/TCP/WebSocket), it acts as a **multiplexing agent**, allowing multiple local apps to leverage the Citadel Protocol simultaneously without needing to embed the full heavy-weight SDK or manage complex connection states individually.

---

## 📑 Table of Contents

- [🏗️ High-Level Architecture](#-high-level-architecture)
- [🔄 Protocol & API Workflows](#-protocol--api-workflows)
    - [Request/Response Model](#requestresponse-model)
    - [Connection Lifecycle](#connection-lifecycle)
    - [File Transfer Logic](#file-transfer-logic)
    - [Local Key-Value Store](#local-key-value-store)
- [📦 Components](#-components)
- [⚡ Automated TypeScript Generation](#-automated-typescript-generation)
- [🛠️ Development & Usage](#-development--usage)

---

## 🏗️ High-Level Architecture

The Agent abstracts the complexity of the Citadel Protocol SDK. Instead of every app managing its own encrypted tunnels and P2P hole-punching logic, they simply send `InternalServiceRequest` payloads to the Agent, which handles the heavy lifting.

```mermaid
graph TD
    subgraph "Local Machine (User's Computer)"
        ClientApp["Client Application<br/>(GUI / CLI / (Web) App)"]
        
        subgraph "Citadel Agent (Internal Service)"
            Router[Protocol Router]
            SDK[Citadel Protocol SDK]
            KV[Local KV DB]
        end
        
        ClientApp -- "IPC (UDS/WS/TCP)<br/>Payload: InternalServiceRequest" --> Router
        Router -- "Payload: InternalServiceResponse<br/>(Success / Failure / Notification)" --> ClientApp
        
        Router <--> SDK
        SDK <--> KV
    end

    subgraph "External Network"
        RemoteServer[Citadel Server]
        RemotePeer[Remote Peer]

        RemoteServer ~~~ RemotePeer
    end

    SDK <== "Encrypted Tunnel" ==> RemoteServer
    SDK <== "Encrypted P2P Channel" ==> RemotePeer

    style ClientApp fill:#e1f5fe,stroke:#01579b,stroke-width:2px
    style Router fill:#fff9c4,stroke:#fbc02d,stroke-width:2px
    style SDK fill:#ffccbc,stroke:#bf360c,stroke-width:2px
```

---

## 🔄 Protocol & API Workflows

The Internal Service operates on a strict **Request/Response** pattern, occasionally supplemented by asynchronous **Notifications** (e.g., when a peer sends a message or a file transfer request arrives).

### Request/Response Model

The API surface is defined by the `InternalServiceRequest` enum. Below is a categorical breakdown of available commands.

```mermaid
classDiagram
    class InternalServiceRequest {
        <<Enum>>
        All available API commands
    }

    namespace Authentication {
        class SessionCommands {
            Connect
            Disconnect
            Register
            Deregister
            GetSessions
            GetAccountInformation
        }
    }

    namespace P2P_Networking {
        class PeerCommands {
            ListAllPeers
            ListRegisteredPeers
            PeerConnect
            PeerConnectAccept
            PeerDisconnect
            PeerRegister
        }
        class Messaging {
            Message
        }
    }

    namespace Collaboration {
        class GroupCommands {
            GroupCreate
            GroupInvite
            GroupMessage
            GroupLeave
            GroupKick
            GroupRespondRequest
            GroupRequestJoin
        }
    }

    namespace Data_and_Files {
        class FileSystem {
            SendFile
            DownloadFile
            DeleteVirtualFile
            RespondFileTransfer
            PickFile
        }
        class LocalDatabase {
            LocalDBGetKV
            LocalDBSetKV
            LocalDBDeleteKV
            LocalDBGetAllKV
            LocalDBClearAllKV
        }
    }

    namespace System {
        class Admin {
            ConnectionManagement
            Batched
        }
    }

    InternalServiceRequest <|-- SessionCommands
    InternalServiceRequest <|-- PeerCommands
    InternalServiceRequest <|-- Messaging
    InternalServiceRequest <|-- GroupCommands
    InternalServiceRequest <|-- FileSystem
    InternalServiceRequest <|-- LocalDatabase
    InternalServiceRequest <|-- Admin
```

### Connection Lifecycle

Establishing a connection involves authentication with the central server, followed by P2P handshakes. Note that `PeerConnectNotification` is an async event that requires a subsequent `PeerConnectAccept` request from the client.

```mermaid
sequenceDiagram
autonumber
    participant Client
    participant Agent
    participant Network

    Note over Client, Network: 1. Server Registration Phase (Once per user)
    Client->>Agent: Request::Register { full_name, username... }
    Agent->>Network: Create Account
    Network-->>Agent: Account Created
    Agent-->>Client: Response::RegisterSuccess { cid }

    Note over Client, Network: 2. Server Connection Phase
    Client->>Agent: Request::Connect { username, password... }
    Agent->>Network: Handshake & Auth
    Network-->>Agent: Authorized
    Agent-->>Client: Response::ConnectSuccess { cid }

    Note over Client, Network: 3. P2P Registration Phase (Once per peer)
    Client->>Agent: Request::PeerRegister { peer_cid... }
    Agent->>Network: Register Peer Link
    Network-->>Agent: Link Acknowledged
    Agent-->>Client: Response::PeerRegisterSuccess

    Note over Client, Network: 4. P2P Connection Phase
    Client->>Agent: Request::PeerConnect { peer_cid }
    Agent->>Network: Signal Peer
    
    par Async Notification
        Network-->>Agent: Incoming Connection Signal
        Agent-->>Client: Response::PeerConnectNotification { peer_cid... }
        Note right of Client: Client must now Accept/Decline
    end

    Client->>Agent: Request::PeerConnectAccept { accept: true }
    Agent->>Network: Finalize Hole Punching
    Agent-->>Client: Response::PeerConnectAcceptSuccess
```

### File Transfer Logic

File transfers require a handshake to ensure the recipient is ready to receive data.

```mermaid
stateDiagram-v2
    [*] --> Idle

    state "Sender Flow" as Sender {
        Idle --> Requesting: SendFile
        Requesting --> Transferring: Accepted by Peer
        Requesting --> Failed: Rejected/Timeout
        Transferring --> Completed: Tick(100%)
    }

    state "Receiver Flow" as Receiver {
        [*] --> Pending: FileTransferRequestNotification
        Pending --> Downloading: RespondFileTransfer(Accept=true)
        Pending --> Rejected: RespondFileTransfer(Accept=false)
        Downloading --> Finished: Tick(100%)
    }

    note right of Pending
        The Receiver gets a 'Notification' event
        and MUST send a 'Respond' request
        to proceed.
    end note
```

### Local Key-Value Store

The Agent provides a secure, encrypted local Key-Value store, allowing client apps to persist state without implementing their own database logic.

```mermaid
graph LR
    subgraph "Client App"
        Set[Request::LocalDBSetKV]
        Get[Request::LocalDBGetKV]
        GetAll[Request::LocalDBGetAllKV]
    end

    subgraph "Citadel Agent"
        DB[(Encrypted Local Storage)]
    end

    Set --> DB
    Get --> DB
    GetAll --> DB

    DB -->|Response::LocalDBSetKVSuccess| Set
    DB -->|Response::LocalDBGetKVSuccess| Get
    DB -->|Response::LocalDBGetAllKVSuccess| GetAll
```

---

## 📦 Components

This repository is organized into the following crates:

- **`citadel-internal-service`**: The core daemon implementation.
- **`citadel-internal-service-types`**: Shared type definitions with automated TypeScript generation.
- **`citadel-internal-service-connector`**: Interfaces for handling connections and IO.
- **`typescript-client`**: A generated TypeScript client with WebSocket support.

---

## ⚡ Automated TypeScript Generation

The project features **fully automated TypeScript type generation** from Rust types using [ts-rs](https://github.com/Aleph-Alpha/ts-rs). All types are properly annotated and automatically exported with dependency resolution.

### Quick Start

Generate TypeScript types with a single command:

```bash
./generate_types.sh
```

This script automatically:
- ✅ Builds the Rust crate with TypeScript features
- ✅ Generates all TypeScript types (85+ files) with proper imports  
- ✅ Creates a convenient `index.ts` for easy imports
- ✅ Fixes any missing import statements
- ✅ Verifies TypeScript compilation

### Features

- **Zero Configuration**: Works out of the box.
- **Complete Type Coverage**: All request/response types included.
- **Automatic Imports**: Dependencies resolved automatically.
- **Build Integration**: Types generated directly to `typescript-client/src/types/`.
- **Validation**: Automatically verifies TypeScript compilation.

### Architecture

The automated generation uses `ts-rs` libraries with `#[ts(export)]` annotations, a custom build script, and test-based triggers to ensure complete coverage.

```
citadel-internal-service-types/
├── src/lib.rs                    # All types with #[ts(export)]
├── build.rs                      # Sets TS_RS_EXPORT_DIR
├── examples/generate_ts_types.rs # Export trigger
└── tests/                        # Export tests for all types

typescript-client/src/types/
├── index.ts                      # Convenient re-exports
├── InternalServiceRequest.ts     # Main request enum
├── InternalServiceResponse.ts    # Main response enum
└── *.ts                          # Individual type files (85+)
```

---

## 🛠️ Development & Usage

### Building

```bash
# Build with TypeScript features
cd citadel-internal-service-types
cargo build --features typescript

# Build TypeScript client
cd typescript-client
npm run build
```

### Testing

```bash
# Run Rust tests
cargo test

# Test TypeScript client
cd typescript-client
npm test
```

### Regenerating Types

Simply run the generation script whenever Rust types change. The script is idempotent and safe to run multiple times.

```bash
./generate_types.sh
```