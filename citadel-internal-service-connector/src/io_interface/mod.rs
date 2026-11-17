use async_trait::async_trait;
use citadel_internal_service_types::InternalServicePayload;
use futures::{Sink, Stream};

#[cfg(feature = "websockets")]
mod handshake_log;
pub mod in_memory;
#[cfg(feature = "websockets")]
pub mod origin_policy;
#[cfg(not(target_arch = "wasm32"))]
pub mod tcp;
#[cfg(feature = "websockets")]
pub mod websockets;

#[async_trait]
pub trait IOInterface: Sized + Send + 'static {
    type Sink: Sink<InternalServicePayload, Error = std::io::Error> + Unpin + Send + 'static;
    type Stream: Stream<Item = std::io::Result<InternalServicePayload>> + Unpin + Send + 'static;
    async fn next_connection(&mut self) -> Option<(Self::Sink, Self::Stream)>;

    /// The browser `Origin` header this connection's handshake carried, as the
    /// transport saw it. `None` when the transport has no handshake header (a
    /// native client over TCP or in memory) -- never a value the client sent in
    /// a request. A sign-in proof signs exactly this (`IdentitySignChallenge`),
    /// so a transport must not invent one.
    fn handshake_origin(sink: &Self::Sink) -> Option<&str>;

    /// Whether a caller on this interface may name a file by absolute path.
    ///
    /// `SendFile` accepts `FileSource::Path`, which the agent opens and sends
    /// to a peer. The agent holds the ratchets, so the protocol then encrypts
    /// and delivers that file faithfully -- to whoever the caller nominated.
    ///
    /// The question is not whether the caller is local. Every interface here is
    /// loopback. It is whether naming a path GAINS the caller anything:
    ///
    ///   * A native process on this machine runs as the user and can already
    ///     open any file the agent could. Refusing the path would protect
    ///     nothing and would break the CLI and desktop contract.
    ///   * Script in a browser page cannot read the filesystem at all. For it,
    ///     an accepted path is a genuine escalation -- and it is the reachable
    ///     attacker, because the production CSP grants `unsafe-eval` so that
    ///     MDX documents can execute, and `PickFile` hands the page real
    ///     absolute paths to ask for.
    ///
    /// So this is a property of the CALLER'S OTHER CAPABILITIES, not of trust,
    /// and it is knowable at compile time because the service is generic over
    /// exactly one interface for the life of the process.
    ///
    /// A browser interface still has both safe routes: `PickFileRef` for a file
    /// the user chose through a picker, and `ByteContents` for one the user
    /// chose through the page.
    const CALLER_CAN_ALREADY_READ_LOCAL_FILES: bool;
}
