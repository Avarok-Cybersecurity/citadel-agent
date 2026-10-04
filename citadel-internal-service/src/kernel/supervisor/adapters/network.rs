//! The OS's word that an interface or address changed, from the `if-watch` crate.

use crate::kernel::supervisor::ports::NetworkWatch;
use citadel_sdk::logging::warn;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use if_watch::tokio::IfWatcher;
use if_watch::IfEvent;
use std::net::IpAddr;

pub(super) struct IfWatch(IfWatcher);

impl IfWatch {
    pub fn new() -> std::io::Result<Self> {
        IfWatcher::new().map(Self)
    }
}

/// Loopback addresses come and go with local tooling and carry no path to anyone.
fn is_route(event: &IfEvent) -> bool {
    let (IfEvent::Up(net) | IfEvent::Down(net)) = event;
    !matches!(net.addr(), IpAddr::V4(a) if a.is_loopback())
        && !matches!(net.addr(), IpAddr::V6(a) if a.is_loopback())
}

impl NetworkWatch for IfWatch {
    fn next_change(&mut self) -> BoxFuture<'_, Option<()>> {
        async move {
            loop {
                match self.0.next().await? {
                    Ok(event) if is_route(&event) => {
                        // A switch of networks arrives as a burst; it is one change.
                        while let Some(Some(Ok(_))) = self.0.next().now_or_never() {}
                        return Some(());
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(target: "citadel::supervisor", "the network watch failed: {err}");
                        return None;
                    }
                }
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use if_watch::IpNet;

    fn net(text: &str) -> IpNet {
        text.parse().expect("a test address")
    }

    #[test]
    fn loopback_is_not_a_route() {
        assert!(!is_route(&IfEvent::Up(net("127.0.0.1/8"))));
        assert!(!is_route(&IfEvent::Down(net("::1/128"))));
    }

    #[test]
    fn a_lan_address_is_a_route_either_way() {
        assert!(is_route(&IfEvent::Up(net("192.168.1.20/24"))));
        assert!(is_route(&IfEvent::Down(net("fd00::5/64"))));
    }
}
