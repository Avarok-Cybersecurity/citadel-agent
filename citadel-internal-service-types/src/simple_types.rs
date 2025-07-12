//! Simplified types for TypeScript generation
//! This module contains simplified versions of the main types that can be generated
//! without external dependency issues.

use serde::{Deserialize, Serialize};
#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Simplified version of InternalServicePayload for TypeScript generation
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SimpleInternalServicePayload {
    Request(SimpleInternalServiceRequest),
    Response(SimpleInternalServiceResponse),
}

/// Simplified version of InternalServiceRequest for TypeScript generation
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SimpleInternalServiceRequest {
    Connect {
        request_id: String, // Using String instead of Uuid to avoid dependency issues
        username: String,
        password: Vec<u8>, // Using Vec<u8> instead of SecBuffer
        connect_mode: String, // Simplified as string
        udp_mode: String, // Simplified as string
        keep_alive_timeout: Option<u64>, // Simplified as seconds
        session_security_settings: String, // Simplified as string
        server_password: Option<String>, // Simplified
    },
    Message {
        request_id: String,
        message: Vec<u8>,
        cid: u64,
        peer_cid: Option<u64>,
        security_level: String, // Simplified as string
    },
    Disconnect {
        request_id: String,
    },
}

/// Simplified version of InternalServiceResponse for TypeScript generation
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SimpleInternalServiceResponse {
    ConnectSuccess {
        cid: u64,
        request_id: Option<String>,
    },
    MessageSuccess {
        request_id: Option<String>,
    },
    DisconnectSuccess {
        request_id: Option<String>,
    },
    Error {
        request_id: Option<String>,
        error: String,
    },
}

#[cfg(feature = "typescript")]
pub fn export_simple_types() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    
    // Get git repository root directory
    let git_root_output = Command::new("git")
        .args(&["rev-parse", "--show-toplevel"])
        .output()?;
    
    if !git_root_output.status.success() {
        return Err("Failed to get git repository root".into());
    }
    
    let git_root = String::from_utf8(git_root_output.stdout)?
        .trim()
        .to_string();
    
    let export_path = Path::new(&git_root).join("bindings");
    fs::create_dir_all(&export_path)?;
    
    // Export simplified types manually
    println!("Generating TypeScript definitions...");
    
    let payload_ts = SimpleInternalServicePayload::decl();
    fs::write(export_path.join("SimpleInternalServicePayload.ts"), payload_ts)?;
    
    let request_ts = SimpleInternalServiceRequest::decl();
    fs::write(export_path.join("SimpleInternalServiceRequest.ts"), request_ts)?;
    
    let response_ts = SimpleInternalServiceResponse::decl();
    fs::write(export_path.join("SimpleInternalServiceResponse.ts"), response_ts)?;
    
    // Create an index.ts file for easy importing
    let index_content = r#"// Auto-generated TypeScript bindings for Citadel Internal Service
export * from './SimpleInternalServicePayload';
export * from './SimpleInternalServiceRequest';
export * from './SimpleInternalServiceResponse';

// Type aliases for compatibility
export type InternalServicePayload = SimpleInternalServicePayload;
export type InternalServiceRequest = SimpleInternalServiceRequest;
export type InternalServiceResponse = SimpleInternalServiceResponse;
"#;
    
    let index_path = export_path.join("index.ts");
    println!("Writing index.ts to: {}", index_path.display());
    fs::write(&index_path, index_content)?;
    
    println!("Simple TypeScript types exported to {}", export_path.display());
    Ok(())
} 