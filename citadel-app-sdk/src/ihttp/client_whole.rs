//! A client call answered whole: the streamed response assembled into one [`Response`].

use super::client::{call_stream, ClientEvent, Key};
use super::{Outbound, Response};
use crate::abi::RawAbi;
use citadel_app_sdk_types::calls::{Address, Transport};
use citadel_app_sdk_types::Refused;

pub(crate) fn call<A: 'static, R: RawAbi + 'static>(
    raw: &R,
    to: Address,
    transport: Transport,
    request: Outbound,
    f: impl FnOnce(&mut A, Result<Response, Refused>) + 'static,
) -> Key {
    let (mut f, mut head, mut body) = (Some(f), None, Vec::new());
    call_stream::<A, R>(raw, to, transport, request, move |app, event| match event {
        ClientEvent::Head(h, via) => head = Some((h, via)),
        ClientEvent::Body(b) => body.extend_from_slice(&b),
        ClientEvent::End => {
            let result = head.take().map(|(head, via)| Response {
                head,
                via,
                body: std::mem::take(&mut body),
            });
            if let Some(f) = f.take() {
                f(
                    app,
                    result.ok_or_else(|| Refused::Unavailable("response without a head".into())),
                );
            }
        }
        ClientEvent::Failed(e) => {
            if let Some(f) = f.take() {
                f(app, Err(e));
            }
        }
    })
}
