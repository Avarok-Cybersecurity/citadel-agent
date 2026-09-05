#[cfg(not(target_arch = "wasm32"))]
use crate::codec::SerializingCodec;
use crate::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
};
#[cfg(not(target_arch = "wasm32"))]
use citadel_io::tokio::net::TcpStream;
#[cfg(not(target_arch = "wasm32"))]
use citadel_io::tokio_util::codec::{Decoder, Framed, LengthDelimitedCodec};
use futures::{Sink, Stream, StreamExt};
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct InternalServiceConnector<T: IOInterface> {
    pub sink: WrappedSink<T>,
    pub stream: WrappedStream<T>,
}

impl<T: IOInterface> InternalServiceConnector<T> {
    pub async fn from_io(mut io: T) -> Option<Self> {
        let (sink, stream) = io.next_connection().await?;
        Some(Self {
            sink: WrappedSink { inner: sink },
            stream: WrappedStream { inner: stream },
        })
    }
}

pub struct WrappedStream<T: IOInterface> {
    pub inner: T::Stream,
}

pub struct WrappedSink<T: IOInterface> {
    pub inner: T::Sink,
}

/// What to do with one frame taken off the agent socket.
///
/// Separated from the poll loop so it can be tested: the loop needs a live `IOInterface`, and
/// the decision is the part that was wrong.
#[derive(Debug)]
pub(crate) enum FramePlan {
    /// A response for the caller.
    Deliver(Box<InternalServiceResponse>),
    /// Not a response, or not decodable. The socket is still open; keep reading.
    Skip,
    /// The socket is finished.
    End,
}

/// Classify one frame.
///
/// This used to be `_ => Poll::Ready(None)`, which said "the stream has ended" about three
/// different things: a genuine end, a REQUEST arriving on a response stream, and a DECODE
/// ERROR. The last is the dangerous one -- the socket is still open and healthy, so the
/// messenger's `while let Some(..) = stream.next()` simply exited and every later message was
/// dropped while `isConnected()` kept answering true. One frame the client could not parse (a
/// newer agent's enum variant reaching a cached bundle, say) ended messaging for that session
/// with nothing reported anywhere.
pub(crate) fn classify_frame(item: Option<std::io::Result<InternalServicePayload>>) -> FramePlan {
    match item {
        Some(Ok(InternalServicePayload::Response(response))) => {
            FramePlan::Deliver(Box::new(response))
        }
        Some(Ok(InternalServicePayload::Request(_))) => FramePlan::Skip,
        Some(Err(_)) => FramePlan::Skip,
        None => FramePlan::End,
    }
}

/// How many unusable frames in a row before this stream is treated as finished.
///
/// A socket that only produces garbage IS dead, and skipping forever would spin. A handful
/// survives a version skew or a corrupt frame without hiding a genuinely broken peer.
const CONSECUTIVE_UNUSABLE_LIMIT: usize = 16;

impl<T: IOInterface> Stream for WrappedStream<T> {
    type Item = InternalServiceResponse;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut unusable = 0usize;
        loop {
            let item = futures::ready!(self.inner.poll_next_unpin(cx));
            match classify_frame(item) {
                FramePlan::Deliver(response) => return Poll::Ready(Some(*response)),
                FramePlan::End => return Poll::Ready(None),
                FramePlan::Skip => {
                    unusable += 1;
                    if unusable >= CONSECUTIVE_UNUSABLE_LIMIT {
                        log::error!(
                            target: "citadel",
                            "{CONSECUTIVE_UNUSABLE_LIMIT} unusable frames in a row from the agent socket; treating it as closed"
                        );
                        return Poll::Ready(None);
                    }
                    log::warn!(
                        target: "citadel",
                        "Unusable frame from the agent socket (#{unusable}); the socket is still open, continuing"
                    );
                }
            }
        }
    }
}

impl<T: IOInterface> Sink<InternalServiceRequest> for WrappedSink<T> {
    type Error = std::io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }

    fn start_send(
        mut self: Pin<&mut Self>,
        item: InternalServiceRequest,
    ) -> Result<(), Self::Error> {
        Pin::new(&mut self.inner).start_send(InternalServicePayload::Request(item))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn wrap_tcp_conn(
    conn: citadel_io::tokio::net::TcpStream,
) -> Framed<TcpStream, SerializingCodec<InternalServicePayload>> {
    let length_delimited = LengthDelimitedCodec::builder()
        .length_field_offset(0) // default value
        .max_frame_length(1024 * 1024 * 64) // 64 MB
        .length_field_type::<u32>()
        .length_adjustment(0)
        .new_codec();

    let serializing_codec = SerializingCodec {
        inner: length_delimited,
        _pd: std::marker::PhantomData,
    };
    serializing_codec.framed(conn)
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    /// A frame the client cannot decode must not be read as "the socket has closed".
    ///
    /// The socket is open; only this frame is unusable. Reading it as end-of-stream ended the
    /// messenger's read loop for good while the UI still reported a live connection.
    #[test]
    fn a_decode_error_does_not_end_the_stream() {
        let err = std::io::Error::new(std::io::ErrorKind::InvalidData, "unknown variant");
        assert!(matches!(classify_frame(Some(Err(err))), FramePlan::Skip));
    }

    #[test]
    fn end_of_stream_is_the_only_end() {
        assert!(matches!(classify_frame(None), FramePlan::End));
    }

    #[test]
    fn the_three_outcomes_are_distinct() {
        // Stated so a future edit that collapses them again has to say so here first.
        let err = std::io::Error::other("x");
        let skip = classify_frame(Some(Err(err)));
        let end = classify_frame(None);
        assert!(
            !matches!(skip, FramePlan::End),
            "a decode error is not an end"
        );
        assert!(
            !matches!(end, FramePlan::Skip),
            "an end is not a skippable frame"
        );
    }
}
