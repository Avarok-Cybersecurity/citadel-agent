//! What every connection this agent makes to a workspace server demands of its certificate.
//!
//! A server that says its certificate is self-signed is taken without verifying it, as every
//! agent connection was before the SDK asked each connection to choose: workspace servers present
//! self-signed certificates, and an identity account proves its server by the key it pinned
//! (kernel/identity), not by the transport. One value, so a stricter one is one change.

use citadel_sdk::prelude::CertVerification;

pub(crate) const SERVER_CERTIFICATE: CertVerification = CertVerification::SelfSignedAccepted;
