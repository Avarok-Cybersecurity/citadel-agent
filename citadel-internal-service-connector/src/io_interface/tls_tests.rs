//! Tests for io_interface/tls.rs, beside it: the module is at the file-length limit.
use super::*;
use crate::io_interface::origin_policy::OriginPolicy;
use crate::io_interface::websockets::WebSocketInterface;
use crate::io_interface::IOInterface;
use citadel_io::tokio;
use std::net::SocketAddr;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

const NAME: &str = "local.test";
const ORIGIN: &str = "https://app.test";

/// A TLS listener with a self-signed certificate for NAME, and a client config that trusts
/// exactly that certificate -- what a browser has once the operator's CA is trusted.
async fn listener() -> (WebSocketInterface, SocketAddr, TlsConnector) {
    let ck = rcgen::generate_simple_self_signed(vec![NAME.to_string()]).unwrap();
    let acceptor = acceptor_from_pem(
        ck.cert.pem().as_bytes(),
        ck.signing_key.serialize_pem().as_bytes(),
    )
    .unwrap();
    let interface = WebSocketInterface::new_tls(
        "127.0.0.1:0".parse().unwrap(),
        OriginPolicy::parse(ORIGIN).unwrap(),
        Some(NAME),
        acceptor,
    )
    .await
    .unwrap();
    let addr = interface.local_addr().unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(ck.cert.der().clone()).unwrap();
    let provider = CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()));
    let client = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (interface, addr, TlsConnector::from(Arc::new(client)))
}

/// Dial the listener over TLS as `host_header`, presenting ORIGIN; the first HTTP status.
async fn handshake(
    addr: SocketAddr,
    connector: &TlsConnector,
    host_header: &str,
) -> Result<(), Box<tokio_tungstenite::tungstenite::Error>> {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let sni = tokio_rustls::rustls::pki_types::ServerName::try_from(NAME).unwrap();
    let tls = connector
        .connect(sni, tcp)
        .await
        .expect("TLS to the published name");
    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("wss://{host_header}/"))
        .header("Host", host_header)
        .header("Origin", ORIGIN)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();
    tokio_tungstenite::client_async(request, tls)
        .await
        .map(|_| ())
        .map_err(Box::new)
}

#[tokio::test]
async fn a_page_dialling_the_published_name_over_tls_is_accepted() {
    let (mut interface, addr, connector) = listener().await;
    let server = tokio::spawn(async move { interface.next_connection().await.is_some() });
    handshake(addr, &connector, &format!("{NAME}:{}", addr.port()))
        .await
        .expect("101");
    assert!(server.await.unwrap(), "the server never saw the connection");
}

#[tokio::test]
async fn a_permitted_origin_on_a_foreign_host_is_refused_403() {
    // DNS rebinding: the page's Origin is allowed, the TLS name verifies, but the request
    // names a host this listener is not. The Host check must refuse it.
    let (mut interface, addr, connector) = listener().await;
    let server = tokio::spawn(async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            interface.next_connection(),
        )
        .await
    });
    let err = handshake(addr, &connector, &format!("evil.test:{}", addr.port()))
        .await
        .unwrap_err();
    match *err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status().as_u16(), 403)
        }
        other => panic!("expected an HTTP 403 refusal, got {other:?}"),
    }
    assert!(
        server.await.unwrap().is_err(),
        "the refused handshake must not become a connection"
    );
}

#[tokio::test]
async fn plain_ws_to_the_tls_listener_is_not_a_connection() {
    let (mut interface, addr, _connector) = listener().await;
    let server = tokio::spawn(async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            interface.next_connection(),
        )
        .await
    });
    let tcp = TcpStream::connect(addr).await.unwrap();
    let result =
        tokio_tungstenite::client_async(format!("ws://127.0.0.1:{}/", addr.port()), tcp).await;
    assert!(
        result.is_err(),
        "a plain ws:// handshake must not succeed against a TLS listener"
    );
    assert!(
        server.await.unwrap().is_err(),
        "the server must not hand out a connection for it"
    );
}

#[test]
fn a_self_signed_pair_becomes_an_acceptor() {
    let ck = rcgen::generate_simple_self_signed(vec!["local.test".to_string()]).unwrap();
    assert!(acceptor_from_pem(
        ck.cert.pem().as_bytes(),
        ck.signing_key.serialize_pem().as_bytes()
    )
    .is_ok());
}

#[test]
fn html_where_a_certificate_should_be_is_named_not_swallowed() {
    let ck = rcgen::generate_simple_self_signed(vec!["local.test".to_string()]).unwrap();
    let err = acceptor_from_pem(
        b"<html>404</html>",
        ck.signing_key.serialize_pem().as_bytes(),
    )
    .err()
    .expect("must be refused");
    assert!(err.contains("certificate"), "{err}");
    let err = acceptor_from_pem(ck.cert.pem().as_bytes(), b"<html>404</html>")
        .err()
        .expect("html is not a key");
    assert!(err.contains("private key"), "{err}");
}
