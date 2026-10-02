use super::*;
use citadel_internal_service_types::MessageSendSuccess;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

fn notification() -> InternalServiceResponse {
    InternalServiceResponse::MessageSendSuccess(MessageSendSuccess {
        cid: 1,
        peer_cid: None,
        request_id: None,
    })
}

type Rx = UnboundedReceiver<InternalServiceResponse>;

fn clients_for(ids: &[u128]) -> (Clients, Vec<Rx>) {
    let mut map = HashMap::new();
    let mut receivers = Vec::new();
    for id in ids {
        let (tx, rx) = unbounded_channel();
        map.insert(Uuid::from_u128(*id), tx);
        receivers.push(rx);
    }
    (Arc::new(RwLock::new(map)), receivers)
}

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// The goal of multi-window sessions, at the route: every window hears it.
#[test]
fn a_notification_reaches_every_attached_connection() {
    let (clients, mut rx) = clients_for(&[1, 2]);
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    let route = SessionRoute::new(subs, clients);

    assert_eq!(route.send(notification()), vec![id(1), id(2)]);
    assert!(rx[0].try_recv().is_ok(), "the primary received nothing");
    assert!(
        rx[1].try_recv().is_ok(),
        "the second window received nothing"
    );
}

/// And no one else does: another account's window on the same agent.
#[test]
fn a_notification_never_reaches_a_connection_outside_the_session() {
    let (clients, mut rx) = clients_for(&[1, 2, 3]);
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    let route = SessionRoute::new(subs, clients);

    route.send(notification());
    assert!(
        rx[2].try_recv().is_err(),
        "a stranger received the session's notification"
    );
}

/// A route captured at spawn follows a later attach and a later drop.
#[test]
fn a_route_follows_membership_changes_made_after_it_was_built() {
    let (clients, mut rx) = clients_for(&[1, 2]);
    let subs = SessionSubscribers::new(id(1));
    let route = SessionRoute::new(subs.clone(), clients);

    subs.attach(id(2));
    subs.detach(id(1));
    assert_eq!(route.send(notification()), vec![id(2)]);
    assert!(
        rx[0].try_recv().is_err(),
        "a detached connection still received"
    );
    assert!(rx[1].try_recv().is_ok());
}

#[test]
fn nobody_attached_is_a_drop_not_a_broadcast() {
    let (clients, mut rx) = clients_for(&[1]);
    let subs = SessionSubscribers::new(id(9));
    let route = SessionRoute::new(subs, clients);

    assert!(route.send(notification()).is_empty());
    assert!(rx[0].try_recv().is_err());
}

#[test]
fn a_closed_receiver_reports_nobody_listening() {
    let (clients, rx) = clients_for(&[1]);
    drop(rx);
    let route = SessionRoute::new(SessionSubscribers::new(id(1)), clients);
    assert!(route.send(notification()).is_empty());
}

#[test]
fn send_to_others_skips_the_requester() {
    let (clients, mut rx) = clients_for(&[1, 2]);
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    let route = SessionRoute::new(subs, clients);

    assert_eq!(route.send_to_others(id(1), notification()), vec![id(2)]);
    assert!(rx[0].try_recv().is_err(), "the requester got it twice");
    assert!(rx[1].try_recv().is_ok());
}

fn role_of(rx: &mut Rx) -> (SessionRole, u32) {
    match rx.try_recv() {
        Ok(InternalServiceResponse::SessionRoleNotification(n)) => (n.role, n.attached),
        other => panic!("expected a role notification, got {other:?}"),
    }
}

#[test]
fn roles_are_announced_to_members_and_displaced_connections() {
    let (clients, mut rx) = clients_for(&[1, 2, 3]);
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    announce_roles(&clients, 7, &subs, &[id(3)]);

    assert_eq!(role_of(&mut rx[0]), (SessionRole::Primary, 2));
    assert_eq!(role_of(&mut rx[1]), (SessionRole::Secondary, 2));
    assert_eq!(role_of(&mut rx[2]), (SessionRole::Detached, 0));
}
