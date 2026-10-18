//! The JSON shapes the page sends and reads for `ReadPulledFileChunk`, by name:
//! the UI is built against these field names.

use crate::{InternalServiceRequest, InternalServiceResponse, ReadPulledFileChunkSuccess};
use serde_json::{json, Value};
use std::path::PathBuf;
use uuid::Uuid;

#[test]
fn the_request_is_read_from_the_page_s_shape() {
    let id = Uuid::new_v4();
    let wire = json!({ "ReadPulledFileChunk": {
        "request_id": id, "cid": 7, "peer_cid": 9,
        "virtual_directory": "/shared/a.docx", "offset": 1048576, "max_len": 1048576
    }});
    let request: InternalServiceRequest = serde_json::from_value(wire).unwrap();
    let InternalServiceRequest::ReadPulledFileChunk {
        request_id,
        cid,
        peer_cid,
        virtual_directory,
        offset,
        max_len,
    } = &request
    else {
        panic!("not a ReadPulledFileChunk: {request:?}")
    };
    assert_eq!(
        (
            *request_id,
            *cid,
            *peer_cid,
            virtual_directory,
            *offset,
            *max_len
        ),
        (
            id,
            7,
            Some(9),
            &PathBuf::from("/shared/a.docx"),
            1 << 20,
            1 << 20
        )
    );
    assert_eq!(request.session_cid(), Some(7), "gated by ownership");
}

#[test]
fn the_success_carries_bytes_as_a_number_array() {
    let response =
        InternalServiceResponse::ReadPulledFileChunkSuccess(ReadPulledFileChunkSuccess {
            cid: 7,
            request_id: None,
            total_size: 3,
            offset: 0,
            data: vec![1, 2, 3],
        });
    let wire: Value = serde_json::to_value(&response).unwrap();
    assert_eq!(
        wire,
        json!({ "ReadPulledFileChunkSuccess": {
            "cid": 7, "request_id": null, "total_size": 3, "offset": 0, "data": [1, 2, 3]
        }})
    );
    assert!(!response.is_error());
}

#[test]
fn the_failure_is_an_error() {
    let wire = json!({ "ReadPulledFileChunkFailure": {
        "cid": 7, "request_id": null, "message": "pull it first"
    }});
    let response: InternalServiceResponse = serde_json::from_value(wire).unwrap();
    assert!(response.is_error());
}
