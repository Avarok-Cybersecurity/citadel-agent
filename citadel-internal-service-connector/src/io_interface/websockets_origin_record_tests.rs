//! The connection's sink carries the `Origin` its handshake carried (`sign-in-with-citadel.md`
//! §8): what `IdentitySignChallenge` signs. A native client that sends none has none.
//!
//! Control: record `None` in `origin_check` instead of the header →
//! `the_handshake_origin_is_recorded_on_the_sink` goes red.

use super::*;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

async fn origin_seen(policy: OriginPolicy, origin: Option<&str>) -> Option<String> {
    let mut interface = WebSocketInterface::new("127.0.0.1:0".parse().unwrap(), policy)
        .await
        .unwrap();
    let bound = interface.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (sink, _stream) = interface.next_connection().await.expect("a connection");
        WebSocketInterface::handshake_origin(&sink).map(str::to_string)
    });
    let mut request = format!("ws://{bound}").into_client_request().unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
    }
    let (_client, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the accept loop yields")
        .unwrap()
}

#[tokio::test]
async fn the_handshake_origin_is_recorded_on_the_sink() {
    let listed = OriginPolicy::parse("https://work.avarok.net").unwrap();
    assert_eq!(
        origin_seen(listed, Some("https://work.avarok.net"))
            .await
            .as_deref(),
        Some("https://work.avarok.net")
    );
    // Under `*` a foreign page is admitted, and its own origin is what it gets recorded.
    assert_eq!(
        origin_seen(OriginPolicy::Any, Some("https://evil.example"))
            .await
            .as_deref(),
        Some("https://evil.example")
    );
}

#[tokio::test]
async fn a_native_client_without_an_origin_records_none() {
    assert_eq!(origin_seen(OriginPolicy::Any, None).await, None);
}
