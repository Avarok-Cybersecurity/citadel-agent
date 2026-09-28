//! How the browser's ILM spends bytes on the wire.
//!
//! The browser is the path to the cloud relay, where every frame is paid for,
//! so it turns on both traffic reductions. Each one is used toward a peer only
//! once that peer has advertised it, so an older client on the other end still
//! receives exactly the frames it always has.

use citadel_internal_service_connector::messenger::{
    CompressionHint, DynamicCompression, IlmOptions,
};
use wasm_bindgen::JsValue;

pub(crate) const BROWSER_ILM_OPTIONS: IlmOptions = IlmOptions {
    piggyback_acks: true,
    dynamic_compression: DynamicCompression::All,
};

/// The optional hint a JS caller passes with a reliable send.
///
/// Absent means "do not compress" -- the behaviour every existing caller
/// already has. A value that is not one of the known names is refused rather
/// than ignored, so a typo in the UI fails loudly instead of silently sending
/// uncompressed forever.
pub(crate) fn parse_compression_hint(
    value: Option<&str>,
) -> Result<Option<CompressionHint>, JsValue> {
    value
        .map(|name| {
            name.parse::<CompressionHint>()
                .map_err(|err| JsValue::from_str(&err.to_string()))
        })
        .transpose()
}
