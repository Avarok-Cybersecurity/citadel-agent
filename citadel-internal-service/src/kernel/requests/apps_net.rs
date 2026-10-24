//! `AppFetch`, `AppFetchCredit`, `AppFetchCancel` and `GetAppNetUsage` (kernel/apps/net).

use crate::kernel::apps::net::rate::InstallKey;
use crate::kernel::apps::net::stream::{Control, StreamIds, StreamSink};
use crate::kernel::apps::net::{AppNet, FetchInput};
use crate::kernel::apps::settings::AccountGrants;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::server_link::ServerLinks;
use crate::kernel::session_route::{deliver, Clients};
use crate::kernel::CitadelWorkspaceService;
use citadel_app_net_policy::REQUEST_TIMEOUT_MS;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AppFetchFailure, AppFetchRefusal, AppFetchResponse, AppFetchUnavailable, AppNetUsage,
    InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// A stream's events, to the one connection that opened it.
struct ConnectionSink {
    clients: Clients,
    connection: Uuid,
}

impl StreamSink for ConnectionSink {
    fn emit(&self, response: InternalServiceResponse) -> bool {
        !deliver(&self.clients, &[self.connection], response).is_empty()
    }
}

fn failure(cid: u64, request_id: Uuid, refusal: AppFetchRefusal) -> InternalServiceResponse {
    InternalServiceResponse::AppFetchFailure(AppFetchFailure {
        cid,
        request_id: Some(request_id),
        refusal,
    })
}

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let answer = |response| Some(HandledRequestResult { response, uuid });
    let unavailable =
        |cid, request_id, why| answer(failure(cid, request_id, AppFetchRefusal::Unavailable(why)));
    match request {
        InternalServiceRequest::AppFetch {
            request_id,
            cid,
            app_id,
            install_id,
            grant,
            request,
            stream,
        } => {
            if !this.attached(cid, uuid) {
                return unavailable(cid, request_id, AppFetchUnavailable::Session);
            }
            let Some(net) = this.apps.net.clone() else {
                return unavailable(cid, request_id, AppFetchUnavailable::NotConfigured);
            };
            let key = InstallKey { cid, install_id };
            let deadline = Instant::now() + Duration::from_millis(REQUEST_TIMEOUT_MS);
            let member = AccountGrants {
                settings: &this.apps.settings,
                io: this.host_io(),
            };
            let link = ServerLinks::new(this.server_connection_map.clone());
            let input = FetchInput {
                key: &key,
                app_id: &app_id,
                grant: &grant,
                request: &request,
                member: &member,
                link: &link,
            };
            if stream {
                return open_stream(this, net, uuid, request_id, key.clone(), input, deadline)
                    .await;
            }
            let response = match net.fetch(input, deadline).await {
                Ok(fetched) => InternalServiceResponse::AppFetchResponse(AppFetchResponse {
                    cid,
                    request_id: Some(request_id),
                    status: fetched.status,
                    headers: fetched.headers,
                    body: fetched.body,
                    redirects: fetched.redirects,
                }),
                Err(refusal) => failure(cid, request_id, refusal),
            };
            answer(response)
        }
        // Answered with nothing: a stream that is not this connection's is not there.
        InternalServiceRequest::AppFetchCredit {
            cid, fetch_id, n, ..
        } => {
            if this.attached(cid, uuid) {
                let _open = this
                    .apps
                    .streams
                    .control(uuid, fetch_id, Control::Credit(n));
            }
            None
        }
        InternalServiceRequest::AppFetchCancel { cid, fetch_id, .. } => {
            if this.attached(cid, uuid) {
                let _open = this.apps.streams.control(uuid, fetch_id, Control::Cancel);
            }
            None
        }
        InternalServiceRequest::GetAppNetUsage {
            request_id,
            cid,
            install_id,
        } => {
            if !this.attached(cid, uuid) {
                return unavailable(cid, request_id, AppFetchUnavailable::Session);
            }
            let key = InstallKey {
                cid,
                install_id: install_id.clone(),
            };
            let usage = this
                .apps
                .net
                .as_ref()
                .map(|net| net.meter.read(&key))
                .unwrap_or_default();
            answer(InternalServiceResponse::AppNetUsage(AppNetUsage {
                cid,
                request_id: Some(request_id),
                install_id,
                bytes_in: usage.bytes_in,
                bytes_out: usage.bytes_out,
                requests: usage.requests,
                refused: usage.refused,
            }))
        }
        _ => unreachable!("dispatched by variant"),
    }
}

/// Register the stream before its head is emitted, so credit sent the instant the head
/// arrives finds it; then every event goes through the sink, in order, and the request
/// itself answers nothing.
async fn open_stream<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    net: Arc<AppNet>,
    uuid: Uuid,
    request_id: Uuid,
    key: InstallKey,
    input: FetchInput<'_>,
    deadline: Instant,
) -> Option<HandledRequestResult> {
    let cid = key.cid;
    let refused = |refusal| {
        Some(HandledRequestResult {
            response: failure(cid, request_id, refusal),
            uuid,
        })
    };
    let Some(mut controls) = this.apps.streams.register(uuid, request_id) else {
        return refused(AppFetchRefusal::Unavailable(AppFetchUnavailable::Duplicate));
    };
    let opened = match net.open(input, deadline).await {
        Ok(opened) => opened,
        Err(refusal) => {
            this.apps.streams.close(uuid, request_id);
            return refused(refusal);
        }
    };
    let apps = this.apps.clone();
    let sink = ConnectionSink {
        clients: this.tx_to_localhost_clients.clone(),
        connection: uuid,
    };
    drop(tokio::spawn(async move {
        let ids = StreamIds {
            cid,
            fetch_id: request_id,
            key,
        };
        net.stream(opened, &ids, &mut controls, &sink, deadline)
            .await;
        apps.streams.close(uuid, request_id);
    }));
    None
}
