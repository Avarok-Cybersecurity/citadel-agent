//! TLS on the agent's own loopback socket.
//!
//! Why a certificate at all, for a server on the user's own machine: a page served over HTTPS
//! may not open a plain `ws://` socket -- browsers block it as mixed content -- and the hosted
//! UI is served over HTTPS. `wss://` needs a certificate the browser trusts, for a name the
//! browser can resolve: the operator points `local.<domain>` at 127.0.0.1 and holds a real
//! certificate for it. The private key is therefore carried by every copy of the agent and is
//! public by construction. That protects nothing that was being protected: there is no network
//! path to loopback to intercept. The certificate exists to make the browser willing to open the
//! socket. The controls doing real work are the Origin and Host allowlists.
//!
//! The certificate is a ninety-day one, so it is fetched or handed in, never embedded: an
//! installed copy that carried its own would stop working in the field with nothing to say why.
use citadel_io::tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use citadel_io::tokio::net::TcpStream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{crypto::CryptoProvider, ServerConfig};
pub use tokio_rustls::TlsAcceptor;

/// A plain TCP stream or a server-side TLS stream over one; the WebSocket layer runs on either.
pub enum Transport {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for Transport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Transport::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Transport::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Transport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Transport::Plain(s) => Pin::new(s).poll_write(cx, data),
            Transport::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, data),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Transport::Plain(s) => Pin::new(s).poll_flush(cx),
            Transport::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Transport::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Transport::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Build an acceptor from a PEM certificate chain and a PEM private key.
///
/// Errors name which of the two inputs is unusable: a site that answers a missing file with a
/// page of HTML would otherwise reach the TLS stack as a certificate and fail later and less
/// clearly.
pub fn acceptor_from_pem(certificate: &[u8], key: &[u8]) -> Result<TlsAcceptor, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(certificate)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("certificate PEM is not readable: {e}"))?;
    if certs.is_empty() {
        return Err("certificate PEM contains no certificate".to_string());
    }
    let key = PrivateKeyDer::from_pem_slice(key)
        .map_err(|e| format!("private key PEM is not readable: {e}"))?;
    // rustls 0.23 needs a process-level crypto provider; take the one already installed if the
    // host process chose one, else this crate's default. Both choices are deterministic.
    let provider = CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()));
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS protocol configuration: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate and key do not form a usable server identity: {e}"))?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
