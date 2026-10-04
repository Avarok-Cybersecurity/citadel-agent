//! A window on the agent, driving post-quantum sign-in the way the UI will: it sends a request,
//! and while it waits for the answer it serves the security-key challenges that request raises
//! with a fake key.

use crate::send;
use citadel_internal_service_connector::connector::{
    InternalServiceConnector, WrappedSink, WrappedStream,
};
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse, RecoveryCodes,
    SecurityKeyChallengeNotification, SignInManagementOp, SignInManagementOutcome, StepUp,
};
use citadel_sdk::prelude::*;
use futures::StreamExt;
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use uuid::Uuid;

/// Longer than the SDK's 60 s key window, so a test of that window is answered by the agent and
/// not cut short here.
pub const WAIT: Duration = Duration::from_secs(90);

/// A security key: answers every challenge for `credential_id` with `prf`.
#[derive(Clone, Debug)]
pub struct FakeKey {
    pub credential_id: Vec<u8>,
    pub prf: [u8; 32],
}

/// What a window offers to `Connect`.
#[derive(Clone, Debug, Default)]
pub struct Offer {
    pub password: Option<String>,
    pub key: Option<FakeKey>,
    pub recovery_code: Option<String>,
    /// A Turnstile token, for a server that checks fresh sign-ins.
    pub admission: Option<String>,
}

/// The request's final answer, and what was asked of the window on the way.
#[derive(Debug)]
pub struct Outcome {
    pub response: InternalServiceResponse,
    pub challenges: Vec<SecurityKeyChallengeNotification>,
}

pub struct Window {
    pub sink: WrappedSink<TcpIOInterface>,
    pub stream: WrappedStream<TcpIOInterface>,
}

impl Window {
    pub async fn open(agent: SocketAddr) -> Result<Self, Box<dyn Error>> {
        let (sink, stream) = InternalServiceConnector::connect(agent).await?.split();
        Ok(Self { sink, stream })
    }

    pub async fn send(&mut self, request: InternalServiceRequest) -> Result<(), Box<dyn Error>> {
        send(&mut self.sink, request).await
    }

    /// The next response with a request id of `request_id`, whatever it is.
    pub async fn next_of(
        &mut self,
        request_id: Uuid,
    ) -> Result<InternalServiceResponse, Box<dyn Error>> {
        loop {
            let response = tokio::time::timeout(WAIT, self.stream.next())
                .await?
                .ok_or("the agent closed the connection")?;
            if response.request_id() == Some(&request_id) {
                return Ok(response);
            }
        }
    }

    /// The next response with a request id of `request_id`, answering its key challenges with
    /// `key` (or leaving them unanswered when `None`).
    pub async fn answer_of(
        &mut self,
        request_id: Uuid,
        key: Option<&FakeKey>,
    ) -> Result<Outcome, Box<dyn Error>> {
        let mut challenges = Vec::new();
        loop {
            match self.next_of(request_id).await? {
                InternalServiceResponse::SecurityKeyChallengeNotification(challenge) => {
                    if let Some(key) = key {
                        self.send(key.answer(&challenge)).await?;
                    }
                    challenges.push(challenge);
                }
                InternalServiceResponse::SecurityKeyAnswerSuccess(_) => {}
                InternalServiceResponse::SecurityKeyAnswerFailure(refused) => {
                    return Err(format!("the agent refused the key's answer: {refused:?}").into())
                }
                response => {
                    return Ok(Outcome {
                        response,
                        challenges,
                    })
                }
            }
        }
    }

    /// Registers without connecting; the recovery codes, or the refusal.
    pub async fn register(
        &mut self,
        server: SocketAddr,
        username: &str,
        password: &str,
    ) -> Result<Result<(u64, RecoveryCodes), String>, Box<dyn Error>> {
        self.register_admitted(server, username, password, None)
            .await
            .map(|registered| registered.map_err(|(message, _)| message))
    }

    /// As `register`, with an admission (Turnstile) token; a refusal also carries its reason.
    pub async fn register_admitted(
        &mut self,
        server: SocketAddr,
        username: &str,
        password: &str,
        token: Option<&str>,
    ) -> Result<Result<(u64, RecoveryCodes), (String, Option<FailureReason>)>, Box<dyn Error>> {
        let request_id = Uuid::new_v4();
        self.send(InternalServiceRequest::Register {
            request_id,
            server_addr: server.to_string(),
            full_name: username.to_string(),
            username: username.to_string(),
            proposed_password: password.into(),
            connect_after_register: false,
            session_security_settings: Default::default(),
            server_password: None,
            admission_token: token.map(str::to_string),
        })
        .await?;
        Ok(match self.answer_of(request_id, None).await?.response {
            InternalServiceResponse::RegisterSuccess(success) => {
                Ok((success.cid, success.recovery_codes.clone()))
            }
            InternalServiceResponse::RegisterFailure(failure) => {
                Err((failure.message, failure.reason_code))
            }
            other => Err((format!("not a registration answer: {other:?}"), None)),
        })
    }

    pub async fn connect(
        &mut self,
        username: &str,
        offer: &Offer,
    ) -> Result<Outcome, Box<dyn Error>> {
        let request_id = Uuid::new_v4();
        self.send(InternalServiceRequest::Connect {
            request_id,
            username: username.to_string(),
            password: offer.password.as_deref().map(SecBuffer::from),
            security_key: offer.key.is_some(),
            recovery_code: offer.recovery_code.as_deref().map(SecBuffer::from),
            admission_token: offer.admission.clone(),
            connect_mode: ConnectMode::Standard { force_login: false },
            udp_mode: UdpMode::Disabled,
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            server_password: None,
        })
        .await?;
        self.answer_of(request_id, offer.key.as_ref()).await
    }

    /// The session's CID, or the refusal's message.
    pub async fn sign_in(
        &mut self,
        username: &str,
        offer: &Offer,
    ) -> Result<Result<u64, String>, Box<dyn Error>> {
        Ok(match self.connect(username, offer).await?.response {
            InternalServiceResponse::ConnectSuccess(success) => Ok(success.cid),
            InternalServiceResponse::ConnectFailure(failure) => Err(failure.message),
            other => Err(format!("{other:?}")),
        })
    }

    pub async fn manage(
        &mut self,
        cid: u64,
        op: SignInManagementOp,
        password: Option<&str>,
        key: Option<&FakeKey>,
    ) -> Result<Result<SignInManagementOutcome, String>, Box<dyn Error>> {
        let request_id = Uuid::new_v4();
        let step_up = StepUp {
            password: password.map(SecBuffer::from),
            security_key: key.is_some(),
        };
        self.send(InternalServiceRequest::SignInManagement {
            request_id,
            cid,
            op,
            step_up,
        })
        .await?;
        Ok(match self.answer_of(request_id, key).await?.response {
            InternalServiceResponse::SignInManagementSuccess(success) => Ok(success.outcome),
            InternalServiceResponse::SignInManagementFailure(failure) => Err(failure.message),
            other => Err(format!("not a management answer: {other:?}")),
        })
    }

    pub async fn disconnect(&mut self, cid: u64) -> Result<(), Box<dyn Error>> {
        let request_id = Uuid::new_v4();
        self.send(InternalServiceRequest::Disconnect { request_id, cid })
            .await?;
        match self.answer_of(request_id, None).await?.response {
            InternalServiceResponse::DisconnectNotification(_) => Ok(()),
            other => Err(format!("not a disconnect answer: {other:?}").into()),
        }
    }
}

impl FakeKey {
    pub fn new(prf: u8) -> Self {
        Self {
            credential_id: crate::pq::CRED.to_vec(),
            prf: [prf; 32],
        }
    }

    pub fn answer(&self, challenge: &SecurityKeyChallengeNotification) -> InternalServiceRequest {
        InternalServiceRequest::SecurityKeyAnswer {
            request_id: challenge.request_id.unwrap_or_default(),
            challenge_id: challenge.challenge_id,
            credential_id: self.credential_id.clone(),
            prf_output: SecBuffer::from(self.prf.to_vec()),
        }
    }
}
