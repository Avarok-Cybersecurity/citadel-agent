use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
#[cfg(feature = "native-dialogs")]
use crate::kernel::PickedFileInfo;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PickFileFailure,
};
use citadel_sdk::prelude::Ratchet;
#[cfg(feature = "native-dialogs")]
use std::time::Instant;
use uuid::Uuid;

#[cfg(feature = "native-dialogs")]
use citadel_internal_service_types::PickFileSuccess;
#[cfg(feature = "native-dialogs")]
use citadel_sdk::logging::info;

use citadel_sdk::logging::error;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PickFile {
        request_id,
        cid,
        title,
        allowed_extensions,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // When native-dialogs feature is enabled, use rfd for file picking
    #[cfg(feature = "native-dialogs")]
    {
        // Use spawn_blocking because rfd doesn't have async support on macOS
        // Wrap in catch_unwind because rfd panics on macOS when running in non-windowed CLI mode
        let result = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut dialog = rfd::FileDialog::new();

                if let Some(title) = title {
                    dialog = dialog.set_title(&title);
                }

                if let Some(extensions) = &allowed_extensions {
                    // Convert Vec<String> to slice of &str
                    let ext_refs: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
                    dialog = dialog.add_filter("Files", &ext_refs);
                }

                dialog.pick_file()
            }))
        })
        .await;

        // Handle spawn_blocking outer result
        let inner_result = match result {
            Ok(inner) => inner,
            Err(err) => {
                error!(target: "citadel", "PickFile spawn_blocking error: {}", err);
                let response = InternalServiceResponse::PickFileFailure(PickFileFailure {
                    cid,
                    message: format!("File picker error: {}", err),
                    request_id: Some(request_id),
                });
                return Some(HandledRequestResult { response, uuid });
            }
        };

        // Handle catch_unwind inner result (panics from rfd on macOS CLI mode)
        let pick_result = match inner_result {
            Ok(result) => result,
            Err(panic_info) => {
                // Extract panic message if possible
                let panic_msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "Native file picker panicked (likely running in CLI mode on macOS)".to_string()
                };

                error!(target: "citadel", "PickFile panic caught: {}", panic_msg);
                let response = InternalServiceResponse::PickFileFailure(PickFileFailure {
                    cid,
                    message: format!("Native file picker failed: {}. Note: Native dialogs require a windowed application on macOS.", panic_msg),
                    request_id: Some(request_id),
                });
                return Some(HandledRequestResult { response, uuid });
            }
        };

        match pick_result {
            Some(path) => {
                // Get file metadata
                match std::fs::metadata(&path) {
                    Ok(metadata) => {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "unknown".to_string());

                        info!(target: "citadel", "PickFile Success: {:?}", path);

                        // Store picked file info for later SendFile reference
                        let picked_info = PickedFileInfo {
                            file_path: path.clone(),
                            file_name: file_name.clone(),
                            file_size: metadata.len(),
                            picked_at: Instant::now(),
                        };

                        // Store in connection's picked_files map
                        {
                            let mut map = this.server_connection_map.write();
                            if let Some(conn) = map.get_mut(&cid) {
                                conn.picked_files.insert(request_id, picked_info);
                                info!(target: "citadel", "PickFile stored for request_id: {:?}", request_id);
                            }
                        }

                        let response = InternalServiceResponse::PickFileSuccess(PickFileSuccess {
                            cid,
                            file_path: path,
                            file_name,
                            file_size: metadata.len(),
                            request_id: Some(request_id),
                        });

                        Some(HandledRequestResult { response, uuid })
                    }
                    Err(err) => {
                        error!(target: "citadel", "PickFile failed to get file metadata: {}", err);
                        let response = InternalServiceResponse::PickFileFailure(PickFileFailure {
                            cid,
                            message: format!("Failed to get file metadata: {}", err),
                            request_id: Some(request_id),
                        });

                        Some(HandledRequestResult { response, uuid })
                    }
                }
            }
            None => {
                // User cancelled the dialog
                info!(target: "citadel", "PickFile cancelled by user");
                let response = InternalServiceResponse::PickFileFailure(PickFileFailure {
                    cid,
                    message: "File selection cancelled".to_string(),
                    request_id: Some(request_id),
                });

                Some(HandledRequestResult { response, uuid })
            }
        }
    }

    // When native-dialogs feature is NOT enabled, return an error
    #[cfg(not(feature = "native-dialogs"))]
    {
        // Suppress unused variable warnings
        let _ = (title, allowed_extensions);

        error!(target: "citadel", "PickFile not available: native-dialogs feature is disabled");
        let response = InternalServiceResponse::PickFileFailure(PickFileFailure {
            cid,
            message: "File picker not available: native-dialogs feature is disabled. This feature requires a desktop environment.".to_string(),
            request_id: Some(request_id),
        });

        Some(HandledRequestResult { response, uuid })
    }
}
