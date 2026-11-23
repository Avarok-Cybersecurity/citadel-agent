//! Why a sign-in, a registration or a reconnect failed, as a code the UI can act on.

use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// A failure the UI can act on, beside the message it shows.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum FailureReason {
    /// The server needs an admission token (show the Turnstile widget) and none was sent.
    AdmissionRequired,
    /// The server refused the admission token that was sent (reset the widget and retry).
    AdmissionFailed,
    /// A Connect named a server, and this agent's account with that username is registered
    /// to a different one. Nothing was dialled: signing in would have reached that other server.
    AccountOnAnotherServer,
    /// A Connect named a username this agent holds no account for.
    NoSuchAccount,
    /// A Register named a username this agent already holds an account for on a different
    /// server (`RegisterFailure::held_for_server`). One agent holds one account per username
    /// (the SDK stores accounts by `username_to_cid`), so registering would overwrite it.
    UsernameHeldForAnotherServer,
    /// A `RegisterIdentity` named a username this agent already holds, on any server (a
    /// re-created server included): registering would overwrite the account here. The UI
    /// offers to forget the old account instead.
    UsernameHeld,
    /// `RegisterIdentity` / `ConnectIdentity` need the identity unlocked.
    IdentityLocked,
    /// `ConnectIdentity` named an account with no identity pin, or one another identity holds.
    NotAnIdentityAccount,
    /// The server did not prove the key pinned for the account; the session was disconnected.
    ServerIdentityMismatch,
    /// An `AddSecurityKey` the SDK refused before sending: the server has not enabled security
    /// keys. The UI says so.
    ServerCannotEnrolSecurityKeys,
    /// `/.well-known/citadel` could not be fetched or did not verify.
    ServerDiscoveryFailed,
    /// The server's verified `/.well-known/citadel` has no `citadel` member: an issuer-only
    /// server (work.avarok.net) that runs no node, so there is nothing to register on.
    NoCitadelEndpoint,
    /// The server runs a Citadel protocol version this agent does not accept (SDK code 550):
    /// one side must be updated.
    ProtocolVersionUnsupported,
    /// The server sealed this account's files in a format this agent does not read (SDK code
    /// 495).
    RevfsSealUnrecognised,
    /// A reconnect's sign-in was refused ("Authentication failed"): what the session kept no
    /// longer signs the account in (its password changed elsewhere, or it now asks for a
    /// security key). Only signing in again brings the session back.
    SignInRefused,
    /// A reconnect gave up after its whole window without the server once answering.
    ServerUnreachable,
    /// A reason this build does not know: a newer agent's. Decoded rather than refused, so
    /// the failure it rides on still reaches the page (as "update the page"); an unknown
    /// variant otherwise failed the whole response, and the page waited out a timeout.
    /// Never sent by the agent.
    #[serde(other)]
    Unknown,
}
