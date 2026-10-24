//! The production connector itself, against a plain-HTTP server on `127.0.0.1:0`.
//!
//! The policy refuses loopback, as it must, so nothing here goes through it: the connector
//! is the layer below, and is handed what the policy would have handed it, a request already
//! decided and pinned. Each test fails if the protection it names is taken out of
//! `ReqwestConnector::open`: the name `pinned.invalid` cannot resolve, so only
//! `resolve_to_addrs` reaches the server; an environment proxy is honoured unless `no_proxy`;
//! and a redirect is followed unless the policy is `none`.

use super::super::ports::{Connector, PinnedRequest};
use super::ReqwestConnector;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The pinned name. `.invalid` never resolves (RFC 6761), so reaching the server proves
/// the connector connected to the pinned address without asking DNS.
const PINNED_HOST: &str = "pinned.invalid";
/// Set only in the child process `an_environment_proxy_is_never_used` starts.
const PROXY_PROBE: &str = "CITADEL_TEST_PROXY_PROBE";

/// Answers every connection with `reply`, recording each request's head.
struct Server {
    address: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
}

async fn serve(reply: &'static str) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let heads = Arc::new(Mutex::new(Vec::new()));
    let recorded = heads.clone();
    drop(tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            recorded
                .lock()
                .push(String::from_utf8_lossy(&head).into_owned());
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    }));
    Server { address, heads }
}

fn pinned(server: &Server, path: &str) -> PinnedRequest {
    let port = server.address.port();
    PinnedRequest {
        method: "GET".into(),
        url: format!("http://{PINNED_HOST}:{port}{path}"),
        host: PINNED_HOST.into(),
        connect: vec![server.address],
        headers: vec![("accept".into(), "text/plain".into())],
        body: vec![],
    }
}

const OK: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi";

async fn body_of(server: &Server, path: &str) -> Result<(u16, Vec<u8>), String> {
    let mut upstream = ReqwestConnector.open(pinned(server, path)).await?;
    let mut body = Vec::new();
    while let Some(chunk) = upstream.body.next_chunk().await? {
        body.extend(chunk);
    }
    Ok((upstream.head.status, body))
}

#[tokio::test]
async fn the_connector_reaches_the_pinned_address_and_never_resolves_the_name() {
    let server = serve(OK).await;
    let answer = body_of(&server, "/hello").await;
    assert_eq!(answer, Ok((200, b"hi".to_vec())));
    let heads = server.heads.lock().clone();
    assert_eq!(heads.len(), 1);
    let port = server.address.port();
    assert!(
        heads[0].starts_with("GET /hello HTTP/1.1\r\n"),
        "{}",
        heads[0]
    );
    // The name is still the request's: `Host` and, over TLS, the server name.
    assert!(
        heads[0]
            .to_ascii_lowercase()
            .contains(&format!("host: {PINNED_HOST}:{port}\r\n")),
        "{}",
        heads[0]
    );
}

#[tokio::test]
async fn a_redirect_is_handed_back_not_followed() {
    let server = serve(
        "HTTP/1.1 302 Found\r\nlocation: /elsewhere\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
    )
    .await;
    let upstream = ReqwestConnector
        .open(pinned(&server, "/start"))
        .await
        .expect("answered");
    assert_eq!(upstream.head.status, 302);
    assert_eq!(upstream.head.header("location"), Some("/elsewhere"));
    drop(upstream);
    let heads = server.heads.lock().clone();
    assert_eq!(
        heads.len(),
        1,
        "the connector followed the redirect: {heads:?}"
    );
}

/// Run in a child process with a proxy in its environment, because reqwest reads the
/// system proxy once per process. Without the probe variable it is the parent's run and
/// does nothing: the parent test below is the one that asserts.
#[tokio::test]
#[ignore = "run by an_environment_proxy_is_never_used, in a process of its own"]
async fn proxy_probe_child() {
    if std::env::var_os(PROXY_PROBE).is_none() {
        return;
    }
    let server = serve(OK).await;
    assert_eq!(
        body_of(&server, "/through").await,
        Ok((200, b"hi".to_vec()))
    );
    assert_eq!(server.heads.lock().len(), 1);
}

#[test]
fn an_environment_proxy_is_never_used() {
    let module = module_path!().split_once("::").map_or("", |(_, rest)| rest);
    let child = format!("{module}::proxy_probe_child");
    let exe = std::env::current_exe().expect("the test binary");
    let mut command = std::process::Command::new(exe);
    command
        .args([child.as_str(), "--exact", "--ignored", "--test-threads=1"])
        .env(PROXY_PROBE, "1");
    // A proxy that refuses every connection: a request sent through it fails.
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(name, "http://127.0.0.1:1");
    }
    for name in ["NO_PROXY", "no_proxy"] {
        command.env_remove(name);
    }
    let output = command.output().expect("the child ran");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the connector went through the environment's proxy:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
