//! A page newer than its agent may send a request that agent has never heard of. The frame
//! must be refused as one bad frame, and the socket must read on: the page's next request is
//! still served. That is all a newer page may rely on from an older agent.

use super::*;
use citadel_internal_service_types::InternalServiceRequest;
use futures::StreamExt;
use uuid::Uuid;

#[tokio::test]
async fn an_unknown_request_is_one_bad_frame_and_the_socket_reads_on() {
    let mut interface = WebSocketInterface::new("127.0.0.1:0".parse().unwrap(), OriginPolicy::Any)
        .await
        .unwrap();
    let bound = interface.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_sink, mut stream) = interface.next_connection().await.expect("a connection");
        let first = stream.next().await.expect("a first frame");
        let second = stream.next().await.expect("a second frame");
        (first.is_err(), second)
    });

    let mut client = WebSocketClient::connect(bound).await.unwrap();
    let id = Uuid::nil();
    client
        .send_json_string(format!(
            r#"{{"Request":{{"SomeLaterRequest":{{"request_id":"{id}","cid":1}}}}}}"#
        ))
        .await
        .unwrap();
    client
        .send(InternalServicePayload::Request(
            InternalServiceRequest::GetSessions { request_id: id },
        ))
        .await
        .unwrap();

    let (first_refused, second) = server.await.unwrap();
    assert!(first_refused, "the unknown request parsed");
    assert!(matches!(
        second,
        Ok(InternalServicePayload::Request(
            InternalServiceRequest::GetSessions { .. }
        ))
    ));
}
