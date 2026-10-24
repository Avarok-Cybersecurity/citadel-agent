//! What a stream's `Cancel` tells the app that opened it. The host hands the reason over in the
//! wire's closed set (`citadel-ihttp-types` `CancelReason`, snake case), the same word whichever
//! plane the stream took, so one cause is one refusal on P2P and through the relay.

use citadel_app_sdk_types::Refused;
use std::string::String;

/// The reason a caller's own `Cancel` names (`CancelReason::Caller` on the wire).
pub(crate) const CANCEL_CALLER: &str = "caller";

/// The refusal a `Cancel{reason}` ends a call with: a limit names which, a protocol fault is
/// policy, and every other end (the server refused, failed, closed, has no route, cannot be
/// reached) is unavailable under the reason's own name.
pub(crate) fn refusal_of(reason: &str) -> Refused {
    let limit = |what: &str| Refused::Limit(what.into());
    match reason {
        "timeout" => limit("time"),
        "too_large" => limit("bytes"),
        "busy" => limit("streams"),
        "rate_limited" => limit("rate"),
        "protocol" => Refused::Policy("protocol".into()),
        other => Refused::Unavailable(String::from(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ihttp::tests::{caller, feed, ok_host, App};
    use crate::pending::drain;
    use citadel_app_sdk_types::calls::{Address, FrameKind, HttpMethod, Transport};

    #[test]
    fn a_servers_cancel_ends_the_call_with_the_mapped_refusal() {
        let fake = ok_host();
        let mut app = App::default();
        let to = Address::User {
            app_id: "net.avarok.compute-pool".into(),
            username: "bob".into(),
        };
        let out = crate::ihttp::Outbound {
            method: HttpMethod::Post,
            path: "/job/chunks".into(),
            query: String::new(),
            headers: vec![],
            body: b"work".to_vec(),
        };
        crate::ihttp::client_whole::call(&fake, to, Transport::P2p, out, |a: &mut App, r| {
            a.responses.push(r)
        });
        drain(&mut app);
        let cancel = FrameKind::Cancel {
            reason: "timeout".into(),
        };
        let from = caller("net.avarok.compute-pool");
        assert!(feed(&fake, &mut app, &from, 1, cancel).is_none());
        assert_eq!(app.responses, [Err(Refused::Limit("time".into()))]);
    }

    #[test]
    fn each_wire_reason_is_the_refusal_the_plan_names() {
        let limit = |w: &str| Refused::Limit(w.into());
        let unavailable = |w: &str| Refused::Unavailable(w.into());
        for (reason, want) in [
            ("timeout", limit("time")),
            ("too_large", limit("bytes")),
            ("busy", limit("streams")),
            ("rate_limited", limit("rate")),
            ("protocol", Refused::Policy("protocol".into())),
            ("caller", unavailable("caller")),
            ("no_route", unavailable("no_route")),
            ("refused", unavailable("refused")),
            ("failed", unavailable("failed")),
            ("closed", unavailable("closed")),
            ("unavailable", unavailable("unavailable")),
        ] {
            assert_eq!(refusal_of(reason), want, "{reason}");
        }
    }
}
