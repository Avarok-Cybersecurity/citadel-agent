//! Every range R4.3 names, at both ends and in the middle, with and without `private`; the
//! addresses just outside each; and IPv4 written inside IPv6.

use super::*;
use crate::ranges::{AddressClass as C, ForbiddenRange as F, PrivateRange as P};
use std::net::{Ipv4Addr, Ipv6Addr};

const V4: [(&str, u32, AddressClass); 16] = [
    ("0.0.0.0", 8, C::Forbidden(F::ThisNetwork)),
    ("10.0.0.0", 8, C::Private(P::Rfc1918)),
    ("100.64.0.0", 10, C::Private(P::Shared)),
    ("127.0.0.0", 8, C::Private(P::Loopback)),
    ("169.254.0.0", 16, C::Private(P::LinkLocal)),
    ("172.16.0.0", 12, C::Private(P::Rfc1918)),
    ("192.0.0.0", 24, C::Forbidden(F::IetfProtocol)),
    ("192.0.2.0", 24, C::Forbidden(F::Documentation)),
    ("192.88.99.0", 24, C::Forbidden(F::Reserved)),
    ("192.168.0.0", 16, C::Private(P::Rfc1918)),
    ("198.18.0.0", 15, C::Forbidden(F::Benchmarking)),
    ("198.51.100.0", 24, C::Forbidden(F::Documentation)),
    ("203.0.113.0", 24, C::Forbidden(F::Documentation)),
    ("224.0.0.0", 4, C::Forbidden(F::Multicast)),
    ("240.0.0.0", 4, C::Forbidden(F::Reserved)),
    ("255.255.255.255", 32, C::Forbidden(F::Reserved)),
];

const V6: [(&str, u32, AddressClass); 11] = [
    ("::", 128, C::Forbidden(F::Unspecified)),
    ("::1", 128, C::Private(P::Loopback)),
    ("::2", 127, C::Forbidden(F::Ipv4Compatible)),
    ("64:ff9b:1::", 48, C::Forbidden(F::Reserved)),
    ("100::", 64, C::Forbidden(F::Discard)),
    ("2001:db8::", 32, C::Forbidden(F::Documentation)),
    ("2001::", 32, C::Forbidden(F::Teredo)),
    ("fc00::", 7, C::Private(P::UniqueLocal)),
    ("fe80::", 10, C::Private(P::LinkLocal)),
    ("fec0::", 10, C::Private(P::SiteLocal)),
    ("ff00::", 8, C::Forbidden(F::Multicast)),
];

fn v4_points(net: &str, prefix: u32) -> [Ipv4Addr; 3] {
    let base = u32::from(net.parse::<Ipv4Addr>().unwrap());
    let span = if prefix == 32 { 0 } else { u32::MAX >> prefix };
    [base, base + span / 2, base + span].map(Ipv4Addr::from)
}

fn v6_points(net: &str, prefix: u32) -> [Ipv6Addr; 3] {
    let base = u128::from(net.parse::<Ipv6Addr>().unwrap());
    let span = if prefix == 128 {
        0
    } else {
        u128::MAX >> prefix
    };
    [base, base + span / 2, base + span].map(Ipv6Addr::from)
}

/// The verdict for a listed public name that resolved to `address`.
fn reach(private: bool, address: IpAddr) -> Verdict {
    let grant = host_grant(private, &["api.acme.com"]);
    decide(
        &get("https://api.acme.com/x"),
        &[address],
        &grant,
        &deployment(),
    )
}

fn expect(address: IpAddr, class: AddressClass) {
    assert_eq!(classify(address), class, "{address}");
    let denied = matches!(
        reach(false, address),
        Verdict::Policy(DenyReason::Address(..))
    );
    assert!(denied, "{address} reached without private");
    let with_private = reach(true, address);
    match class {
        C::Private(_) => assert!(with_private.is_allowed(), "{address} with private"),
        _ => assert!(!with_private.is_allowed(), "{address} reached with private"),
    }
}

#[test]
fn every_ipv4_range_is_classified_and_decided_at_both_ends_and_inside() {
    for (net, prefix, class) in V4 {
        for point in v4_points(net, prefix) {
            // 169.254.169.254 is a metadata endpoint, refused even with private.
            if point != Ipv4Addr::new(169, 254, 169, 254) {
                expect(IpAddr::V4(point), class);
            }
        }
    }
}

#[test]
fn every_ipv6_range_is_classified_and_decided_at_both_ends_and_inside() {
    for (net, prefix, class) in V6 {
        for point in v6_points(net, prefix) {
            expect(IpAddr::V6(point), class);
        }
    }
}

#[test]
fn ipv4_inside_ipv6_is_judged_as_the_ipv4_it_carries() {
    for (net, prefix, class) in V4 {
        for point in v4_points(net, prefix) {
            if point == Ipv4Addr::new(169, 254, 169, 254) {
                continue;
            }
            let [a, b, c, d] = point.octets();
            let low = |hi: u8, lo: u8| u16::from(hi) << 8 | u16::from(lo);
            let forms = [
                point.to_ipv6_mapped(),
                Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, low(a, b), low(c, d)),
                Ipv6Addr::new(0x2002, low(a, b), low(c, d), 0, 0, 0, 0, 1),
            ];
            for form in forms {
                assert_eq!(canonical(IpAddr::V6(form)), IpAddr::V4(point), "{form}");
                expect(IpAddr::V6(form), class);
            }
        }
    }
}

#[test]
fn the_addresses_just_outside_each_range_are_public() {
    for text in [
        "1.0.0.0",
        "9.255.255.255",
        "11.0.0.0",
        "100.63.255.255",
        "100.128.0.0",
        "126.255.255.255",
        "128.0.0.0",
        "169.253.255.255",
        "169.255.0.0",
        "172.15.255.255",
        "172.32.0.0",
        "191.255.255.255",
        "192.0.1.0",
        "192.0.3.0",
        "192.88.98.255",
        "192.88.100.0",
        "192.167.255.255",
        "192.169.0.0",
        "198.17.255.255",
        "198.20.0.0",
        "198.51.99.255",
        "198.51.101.0",
        "203.0.112.255",
        "203.0.114.0",
        "223.255.255.255",
        "8.8.8.8",
        "2000::",
        "2001:1::",
        "2001:db7:ffff::",
        "2001:db9::",
        "2003::",
        "3fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2606:4700::1111",
    ] {
        assert_eq!(classify(ip(text)), C::Public, "{text}");
        assert!(reach(false, ip(text)).is_allowed(), "{text} refused");
    }
}

#[test]
fn ipv6_outside_global_unicast_is_reserved() {
    for text in [
        "1fff:ffff::",
        "4000::",
        "fbff::",
        "fe00::",
        "fe7f::",
        "::1:0:0:0",
    ] {
        assert_eq!(classify(ip(text)), C::Forbidden(F::Reserved), "{text}");
        assert!(!reach(true, ip(text)).is_allowed(), "{text}");
    }
}

#[test]
fn one_private_address_among_public_ones_refuses_the_request() {
    let grant = host_grant(false, &["api.acme.com"]);
    let resolved = [ip("93.184.216.34"), ip("10.0.0.7")];
    let verdict = decide(
        &get("https://api.acme.com/"),
        &resolved,
        &grant,
        &deployment(),
    );
    assert!(matches!(reason(verdict), DenyReason::Address(a, _) if a == ip("10.0.0.7")));
    // Control: the public address alone is allowed, and pinned.
    let verdict = decide(
        &get("https://api.acme.com/"),
        &resolved[..1],
        &grant,
        &deployment(),
    );
    let Verdict::Allow(allowed) = verdict else {
        panic!("refused")
    };
    assert_eq!(allowed.connect, vec!["93.184.216.34:443".parse().unwrap()]);
}

#[test]
fn server_egress_never_reaches_a_private_address() {
    let grant = NetGrant::new(Egress::Server, &["api.acme.com"]).unwrap();
    for text in ["10.1.2.3", "127.0.0.1", "fd00::1", "::ffff:192.168.1.1"] {
        let verdict = decide(
            &get("https://api.acme.com/"),
            &[ip(text)],
            &grant,
            &deployment(),
        );
        assert!(matches!(reason(verdict), DenyReason::Address(..)), "{text}");
    }
    let verdict = decide(
        &get("https://api.acme.com/"),
        &[ip("8.8.8.8")],
        &grant,
        &deployment(),
    );
    assert!(verdict.is_allowed(), "control: a public address is reached");
}
