//! An agent whose localhost clients are treated like a browser page.
//!
//! The agent's TCP interface marks its caller as one that can already read the
//! filesystem, so a `SendFile` naming any path is accepted. A browser page
//! cannot read files, and over the WebSocket interface the agent refuses a
//! path it did not hand out. This is that rule, on TCP, so in-process tests can
//! reach it.

use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_connector::io_interface::IOInterface;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

/// An interface a test can bind on a fresh address.
pub trait BindableInterface: IOInterface + Sync {
    fn bind(addr: SocketAddr) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send>>;
}

impl BindableInterface for TcpIOInterface {
    fn bind(addr: SocketAddr) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send>> {
        Box::pin(TcpIOInterface::new(addr))
    }
}

pub struct BrowserLikeTcp(TcpIOInterface);

impl IOInterface for BrowserLikeTcp {
    type Sink = <TcpIOInterface as IOInterface>::Sink;
    type Stream = <TcpIOInterface as IOInterface>::Stream;
    const CALLER_CAN_ALREADY_READ_LOCAL_FILES: bool = false;

    // The trait is `#[async_trait]`; this is the signature it expands to.
    fn next_connection<'a, 'f>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Option<(Self::Sink, Self::Stream)>> + Send + 'f>>
    where
        'a: 'f,
        Self: 'f,
    {
        self.0.next_connection()
    }
}

impl BindableInterface for BrowserLikeTcp {
    fn bind(addr: SocketAddr) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send>> {
        Box::pin(async move { TcpIOInterface::new(addr).await.map(BrowserLikeTcp) })
    }
}
