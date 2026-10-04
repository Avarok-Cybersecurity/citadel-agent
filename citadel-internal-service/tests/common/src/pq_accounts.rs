//! Accounts in a given sign-in state, set up through the agent as a user would: register, sign
//! in with the password, enrol a key right after (the PRF salt exists only once the server has
//! the account), and choose a policy.

use crate::pq::{username, CRED, PASSWORD};
use crate::pq_window::{FakeKey, Offer, Window};
use citadel_internal_service_types::RecoveryCodes;
use citadel_sdk::prelude::{SignInManagementOp, SignInManagementOutcome, SignInPolicy};
use std::error::Error;
use std::net::SocketAddr;

pub struct Account {
    pub username: String,
    pub codes: RecoveryCodes,
}

pub fn password_offer() -> Offer {
    Offer {
        password: Some(PASSWORD.to_string()),
        ..Default::default()
    }
}

pub fn add_key() -> SignInManagementOp {
    SignInManagementOp::AddSecurityKey {
        credential_id: CRED.to_vec(),
        label: "YubiKey".to_string(),
    }
}

/// A signed-out account whose policy is `policy`, with `key` enrolled unless the policy is
/// `Password`.
pub async fn account(
    window: &mut Window,
    server: SocketAddr,
    policy: SignInPolicy,
    key: &FakeKey,
) -> Result<Account, Box<dyn Error>> {
    let username = username("acct");
    let (_, codes) = window.register(server, &username, PASSWORD).await??;
    let cid = window.sign_in(&username, &password_offer()).await??;
    if policy != SignInPolicy::Password {
        let added = window
            .manage(cid, add_key(), Some(PASSWORD), Some(key))
            .await??;
        assert!(
            matches!(added, SignInManagementOutcome::Added { .. }),
            "{added:?}"
        );
        let set = SignInManagementOp::SetSignInPolicy { policy };
        let set = window.manage(cid, set, Some(PASSWORD), Some(key)).await??;
        assert_eq!(set, SignInManagementOutcome::PolicySet);
    }
    window.disconnect(cid).await?;
    Ok(Account { username, codes })
}
