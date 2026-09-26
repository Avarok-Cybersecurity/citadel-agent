//! A session can say which groups it is in -- the ones it joined, not only the
//! ones it owns.
//!
//! The UI keeps its group list in the browser. `GroupListGroupsFor` answers
//! only for groups an owner created, so a member signing in from a new browser
//! saw "No conversations yet" for groups it was in (measured live: Lara and Max
//! in a fresh browser, both in "Sweep Group"). The session's live group
//! channels are exactly its membership, and `GroupListJoined` reports them.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{joined_group_on_one_service, recv_until, OneServiceGroup};
    use crate::common::open_localhost_connection;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use citadel_sdk::prelude::MessageGroupKey;
    use std::error::Error;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    type Tx = UnboundedSender<InternalServiceRequest>;
    type Rx = UnboundedReceiver<InternalServiceResponse>;

    async fn list_joined(tx: &Tx, rx: &mut Rx, cid: u64) -> InternalServiceResponse {
        let request_id = Uuid::new_v4();
        tx.send(InternalServiceRequest::GroupListJoined { cid, request_id })
            .expect("service channel open");
        recv_until(rx, "GroupListJoined answer", |r| match r {
            InternalServiceResponse::GroupListJoinedSuccess(s) => s.request_id == Some(request_id),
            InternalServiceResponse::GroupListJoinedFailure(f) => f.request_id == Some(request_id),
            _ => false,
        })
        .await
    }

    fn listed(answer: InternalServiceResponse) -> Vec<MessageGroupKey> {
        match answer {
            InternalServiceResponse::GroupListJoinedSuccess(s) => s.groups,
            other => panic!("the joined groups could not be read: {other:?}"),
        }
    }

    async fn list_owned(tx: &Tx, rx: &mut Rx, cid: u64) -> Vec<MessageGroupKey> {
        let request_id = Uuid::new_v4();
        tx.send(InternalServiceRequest::GroupListGroupsFor {
            cid,
            peer_cid: None,
            request_id,
        })
        .expect("service channel open");
        match recv_until(rx, "GroupListGroupsFor answer", |r| match r {
            InternalServiceResponse::GroupListGroupsSuccess(s) => s.request_id == Some(request_id),
            InternalServiceResponse::GroupListGroupsFailure(f) => f.request_id == Some(request_id),
            _ => false,
        })
        .await
        {
            InternalServiceResponse::GroupListGroupsSuccess(s) => s.group_list.unwrap_or_default(),
            other => panic!("the owned groups could not be read: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_member_and_an_owner_both_list_the_group() -> Result<(), Box<dyn Error>> {
        let OneServiceGroup {
            service_addr: _,
            owner: (owner_tx, mut owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service("listjoined").await?;

        // The gap this closes: the only list there was says nothing to a member.
        assert!(
            !list_owned(&member_tx, &mut member_rx, member_cid).await.contains(&group_key),
            "control: the owned-groups list was expected NOT to name a group the member only joined"
        );

        assert_eq!(
            listed(list_joined(&member_tx, &mut member_rx, member_cid).await),
            vec![group_key]
        );
        assert_eq!(
            listed(list_joined(&owner_tx, &mut owner_rx, owner_cid).await),
            vec![group_key]
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_connection_that_does_not_hold_the_session_is_refused() -> Result<(), Box<dyn Error>>
    {
        let OneServiceGroup {
            service_addr,
            owner: _owner,
            member: (_member_tx, _member_rx, member_cid),
            group_key: _,
        } = joined_group_on_one_service("listjoinedgate").await?;

        let (other_tx, mut other_rx) = open_localhost_connection(service_addr).await?;
        match list_joined(&other_tx, &mut other_rx, member_cid).await {
            InternalServiceResponse::GroupListJoinedFailure(_) => Ok(()),
            other => panic!("another connection read the session's groups: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_group_the_member_left_is_not_listed() -> Result<(), Box<dyn Error>> {
        let OneServiceGroup {
            service_addr: _,
            owner: _owner,
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service("listjoinedleave").await?;

        assert_eq!(
            listed(list_joined(&member_tx, &mut member_rx, member_cid).await),
            vec![group_key],
            "control: listed while a member"
        );

        let request_id = Uuid::new_v4();
        member_tx.send(InternalServiceRequest::GroupLeave {
            cid: member_cid,
            group_key,
            request_id,
        })?;
        let left = recv_until(&mut member_rx, "GroupLeave answer", |r| match r {
            InternalServiceResponse::GroupLeaveSuccess(s) => s.request_id == Some(request_id),
            InternalServiceResponse::GroupLeaveFailure(f) => f.request_id == Some(request_id),
            _ => false,
        })
        .await;
        assert!(
            matches!(left, InternalServiceResponse::GroupLeaveSuccess(_)),
            "the member could not leave: {left:?}"
        );

        assert!(listed(list_joined(&member_tx, &mut member_rx, member_cid).await).is_empty());
        Ok(())
    }
}
