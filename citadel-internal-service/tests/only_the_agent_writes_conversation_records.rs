//! The agent is the only writer of a hosted account's conversation records:
//! a window's LocalDB write to one is refused.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::hosted_pair;
    use crate::common::group::recv_until;
    use crate::common::setup_log;
    use citadel_internal_service_types::InternalServiceRequest;
    use uuid::Uuid;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_window_cannot_write_a_hosted_accounts_conversation_record() {
        setup_log();
        let mut world = hosted_pair().await;
        let (alice, bob) = (world.alice.2, world.bob.2);
        let probe = Uuid::new_v4();
        world
            .alice
            .0
            .send(InternalServiceRequest::LocalDBSetKV {
                request_id: probe,
                cid: 0,
                peer_cid: None,
                key: format!("msgs_with_peer_{alice}_with_{bob}_metadata"),
                value: b"{}".to_vec(),
            })
            .unwrap();
        let answer = recv_until(&mut world.alice.1, "the write", |r| {
            r.request_id() == Some(&probe)
        })
        .await;
        assert!(
            answer.is_error(),
            "a window wrote the agent's conversation record: {answer:?}"
        );
    }
}
