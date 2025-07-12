//! Generate TypeScript bindings using ts-rs automatic export functionality
//!
//! This example generates TypeScript types with automatic dependency resolution
//! and proper import statements by using ts-rs's built-in export functionality.

use citadel_internal_service_types::*;

#[cfg(feature = "typescript")]
use ts_rs::TS;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Generating TypeScript types...");

    #[cfg(feature = "typescript")]
    {
        use std::env;

        // Check the export directory from environment variable
        let export_dir = env::var("TS_RS_EXPORT_DIR").unwrap_or_else(|_| "./bindings".to_string());

        println!("Exporting to: {}", export_dir);

        // Use export_all to export all dependencies with proper imports
        // These methods automatically use the TS_RS_EXPORT_DIR environment variable
        InternalServiceRequest::export_all()?;
        InternalServiceResponse::export_all()?;
        InternalServicePayload::export_all()?;

        println!("TypeScript types generated successfully!");
        println!("Types available in: {}", export_dir);
    }

    #[cfg(not(feature = "typescript"))]
    {
        println!("TypeScript feature not enabled. Run with: cargo run --example generate_ts_types --features typescript");
    }

    Ok(())
}
