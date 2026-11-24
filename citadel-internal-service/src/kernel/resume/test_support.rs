//! A real resume credential for tests: registered, signed in and granted through the SDK's
//! own client and server functions, so what the agent's rules are tested against is what a
//! server hands out.

use citadel_sdk::prelude::{ResumeCredential, ResumePolicy, SecBuffer, ThrottlePolicy};
use citadel_user::auth::pq::client::{ClientLogin, ClientProof, ClientRegistration, NewRecovery};
use citadel_user::auth::pq::login_transcript;
use citadel_user::auth::pq::messages::{AuthChallenge, ChallengeCeremony, LoginProof};
use citadel_user::auth::pq::oprf::OprfSeed;
use citadel_user::auth::pq::record::KsfParams;
use citadel_user::auth::pq::resume::DeviceKey;
use citadel_user::auth::pq::server::{
    build_login_challenge, grant_after, registration_reply, AccountAuth, Expectation,
    PqAuthServerSettings,
};
use std::time::Duration;

/// When every test credential is issued (Unix milliseconds), by server and device alike.
pub(crate) const ISSUED_AT: u64 = 1_700_000_000_000;
const CID: u64 = 7;
const USER: &str = "alice";

/// A credential issued at [`ISSUED_AT`] to live `lifetime_s` seconds.
pub(crate) async fn credential(lifetime_s: u64) -> ResumeCredential {
    credential_accepted_at(lifetime_s, ISSUED_AT).await
}

/// A credential issued at [`ISSUED_AT`] by the server's clock and accepted when this
/// computer's clock read `local_ms`: a device whose clock is off by the difference.
pub(crate) async fn credential_accepted_at(lifetime_s: u64, local_ms: u64) -> ResumeCredential {
    let policy = ResumePolicy::new(Duration::from_secs(lifetime_s)).expect("policy");
    let settings = PqAuthServerSettings::new(
        OprfSeed::from_bytes([9; 32]),
        KsfParams::FLOOR,
        ThrottlePolicy::RECOMMENDED,
    )
    .expect("settings")
    .with_resume(policy);
    let password = SecBuffer::from("pw");

    let (start, client) = ClientRegistration::start(USER, &password).expect("start");
    let (reply, pending) = registration_reply(&settings, &start).expect("reply");
    let (finish, _) = client
        .finish(&reply, NewRecovery::None)
        .await
        .expect("finish");
    let record = pending.finish(finish, ISSUED_AT).expect("record");

    let (start, client) = ClientLogin::start(USER, Some(&password), None).expect("login");
    let account = AccountAuth::PostQuantum(&record);
    let (challenge, expectation) =
        build_login_challenge(&settings, account, &start).expect("challenge");
    let Expectation::Factors(expected) = expectation else {
        panic!("not a post-quantum challenge");
    };
    let sent = AuthChallenge {
        challenge: challenge.clone(),
        ceremony: ChallengeCeremony {
            rp_id: None,
            resume_max_lifetime_s: Some(policy.max_lifetime_s()),
        },
    };
    let transcript = login_transcript(CID, &start, &sent);
    let proof = client
        .respond(&challenge, &transcript, None)
        .await
        .expect("proof");
    let ClientProof::Factors {
        proof: LoginProof::Factors(finish),
        ..
    } = proof
    else {
        panic!("not a factor proof");
    };
    let verified = expected.bind(transcript).verify(&finish).expect("verified");

    let device = DeviceKey::generate();
    let request = device.request(lifetime_s).expect("request");
    let grant =
        grant_after(&settings, CID, &record, &verified, &request, ISSUED_AT).expect("grant");
    ResumeCredential::accept(grant, device, CID, &request, local_ms).expect("credential")
}
