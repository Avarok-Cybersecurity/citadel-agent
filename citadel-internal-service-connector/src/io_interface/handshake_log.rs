//! How loudly to report a failed WebSocket/TLS handshake.
//!
//! The launcher's port probe opens a socket, maybe sends a ClientHello, and
//! leaves; a browser or script that speaks plain HTTP to a `wss` port sends
//! bytes that are not a TLS record. Neither is a fault of the agent, and both
//! logged at ERROR on every probe. Anything else about a handshake is still
//! worth a warning, and the peer address says whose it was.

use log::Level;
use std::io::ErrorKind;
use std::net::SocketAddr;
use tokio_rustls::rustls::{self, InvalidMessage};
use tokio_tungstenite::tungstenite::Error as TungsteniteError;

pub(super) fn handshake_failure_level(err: &TungsteniteError) -> Level {
    let TungsteniteError::Io(io_err) = err else {
        return Level::Warn;
    };
    let not_tls = matches!(
        io_err
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>()),
        Some(rustls::Error::InvalidMessage(
            InvalidMessage::InvalidContentType
        ))
    );
    if io_err.kind() == ErrorKind::UnexpectedEof || not_tls {
        Level::Debug
    } else {
        Level::Warn
    }
}

pub(super) fn log_handshake_failure(err: &TungsteniteError, peer: SocketAddr) {
    log::log!(
        target: "citadel",
        handshake_failure_level(err),
        "WebSocket handshake from {peer} failed: {err}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn io_failure(err: io::Error) -> TungsteniteError {
        TungsteniteError::Io(err)
    }

    #[test]
    fn a_probe_that_hangs_up_is_debug() {
        let err = io_failure(io::Error::new(
            ErrorKind::UnexpectedEof,
            "tls handshake eof",
        ));
        assert_eq!(handshake_failure_level(&err), Level::Debug);
    }

    #[test]
    fn plain_http_on_a_tls_port_is_debug() {
        let tls = rustls::Error::InvalidMessage(InvalidMessage::InvalidContentType);
        let err = io_failure(io::Error::new(ErrorKind::InvalidData, tls));
        assert_eq!(handshake_failure_level(&err), Level::Debug);
    }

    /// Negative controls: a different I/O failure, a different TLS failure and a
    /// non-I/O failure must stay audible, or the classifier has just muted all.
    #[test]
    fn other_failures_stay_at_warn() {
        let denied = io_failure(io::Error::from(ErrorKind::PermissionDenied));
        assert_eq!(handshake_failure_level(&denied), Level::Warn);

        let other_tls = rustls::Error::InvalidMessage(InvalidMessage::MissingData("x"));
        let err = io_failure(io::Error::new(ErrorKind::InvalidData, other_tls));
        assert_eq!(handshake_failure_level(&err), Level::Warn);

        assert_eq!(
            handshake_failure_level(&TungsteniteError::ConnectionClosed),
            Level::Warn
        );
    }
}
