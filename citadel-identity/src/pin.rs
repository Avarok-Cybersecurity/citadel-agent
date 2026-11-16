//! A refreshed `/.well-known/citadel` against what the account pinned at registration: the key
//! never changes silently, and a document never goes back in time.

use crate::keys::{fingerprint, MlDsa65Pk};
use crate::well_known::{WellKnownCitadel, WellKnownError};

/// A refreshed document against what the account pinned: same key, not older.
pub fn check_against_pin(
    doc: &WellKnownCitadel,
    pinned_key: &MlDsa65Pk,
    pinned_issued_at_s: u64,
) -> Result<(), WellKnownError> {
    if doc.server_key != *pinned_key {
        return Err(WellKnownError::KeyChanged {
            pinned_fp: fingerprint(pinned_key),
            offered_fp: fingerprint(&doc.server_key),
        });
    }
    if doc.issued_at_s < pinned_issued_at_s {
        return Err(WellKnownError::Stale {
            pinned_issued_at_s,
            issued_at_s: doc.issued_at_s,
        });
    }
    Ok(())
}
