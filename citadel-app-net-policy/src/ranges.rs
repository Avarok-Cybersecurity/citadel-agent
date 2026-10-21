//! What kind of address an IP is, for the policy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Reachable only by host egress granted `private`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateRange {
    /// 127.0.0.0/8, ::1
    Loopback,
    /// 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
    Rfc1918,
    /// 169.254.0.0/16, fe80::/10
    LinkLocal,
    /// 100.64.0.0/10 (carrier-grade NAT)
    Shared,
    /// fc00::/7
    UniqueLocal,
    /// fec0::/10 (deprecated site-local)
    SiteLocal,
}

/// Never reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenRange {
    /// 0.0.0.0/8
    ThisNetwork,
    /// ::
    Unspecified,
    /// 192.0.0.0/24
    IetfProtocol,
    /// 198.18.0.0/15
    Benchmarking,
    /// 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32
    Documentation,
    /// 224.0.0.0/4, ff00::/8
    Multicast,
    /// 240.0.0.0/4 (with 255.255.255.255), 192.88.99.0/24, 64:ff9b:1::/48, and IPv6 outside
    /// 2000::/3 not named elsewhere
    Reserved,
    /// ::/96 other than :: and ::1 (deprecated IPv4-compatible)
    Ipv4Compatible,
    /// 2001::/32
    Teredo,
    /// 100::/64
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    Public,
    Private(PrivateRange),
    Forbidden(ForbiddenRange),
}

/// The IPv4 address an IPv6 address carries, where it carries one the network will deliver to
/// (mapped, NAT64, 6to4); otherwise the address itself.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => embedded_v4(v6).map_or(ip, IpAddr::V4),
    }
}

fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let low = || Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
    if s[..5] == [0, 0, 0, 0, 0] && s[5] == 0xffff {
        return Some(low());
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return Some(low());
    }
    if s[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    None
}

pub fn classify(ip: IpAddr) -> AddressClass {
    match canonical(ip) {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn in_v4(ip: Ipv4Addr, net: [u8; 4], prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(ip) & mask == u32::from(Ipv4Addr::from(net)) & mask
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    use AddressClass::{Forbidden, Private};
    use ForbiddenRange as F;
    use PrivateRange as P;
    const TABLE: [([u8; 4], u32, AddressClass); 16] = [
        ([0, 0, 0, 0], 8, Forbidden(F::ThisNetwork)),
        ([10, 0, 0, 0], 8, Private(P::Rfc1918)),
        ([100, 64, 0, 0], 10, Private(P::Shared)),
        ([127, 0, 0, 0], 8, Private(P::Loopback)),
        ([169, 254, 0, 0], 16, Private(P::LinkLocal)),
        ([172, 16, 0, 0], 12, Private(P::Rfc1918)),
        ([192, 0, 0, 0], 24, Forbidden(F::IetfProtocol)),
        ([192, 0, 2, 0], 24, Forbidden(F::Documentation)),
        ([192, 88, 99, 0], 24, Forbidden(F::Reserved)),
        ([192, 168, 0, 0], 16, Private(P::Rfc1918)),
        ([198, 18, 0, 0], 15, Forbidden(F::Benchmarking)),
        ([198, 51, 100, 0], 24, Forbidden(F::Documentation)),
        ([203, 0, 113, 0], 24, Forbidden(F::Documentation)),
        ([224, 0, 0, 0], 4, Forbidden(F::Multicast)),
        ([240, 0, 0, 0], 4, Forbidden(F::Reserved)),
        ([255, 255, 255, 255], 32, Forbidden(F::Reserved)),
    ];
    TABLE
        .iter()
        .find(|(net, prefix, _)| in_v4(ip, *net, *prefix))
        .map(|(_, _, class)| *class)
        .unwrap_or(AddressClass::Public)
}

fn in_v6(ip: Ipv6Addr, net: [u16; 8], prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    u128::from(ip) & mask == u128::from(Ipv6Addr::from(net)) & mask
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    use AddressClass::{Forbidden, Private};
    use ForbiddenRange as F;
    use PrivateRange as P;
    const TABLE: [([u16; 8], u32, AddressClass); 12] = [
        ([0, 0, 0, 0, 0, 0, 0, 0], 128, Forbidden(F::Unspecified)),
        ([0, 0, 0, 0, 0, 0, 0, 1], 128, Private(P::Loopback)),
        ([0, 0, 0, 0, 0, 0, 0, 0], 96, Forbidden(F::Ipv4Compatible)),
        ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48, Forbidden(F::Reserved)),
        ([0x100, 0, 0, 0, 0, 0, 0, 0], 64, Forbidden(F::Discard)),
        (
            [0x2001, 0xdb8, 0, 0, 0, 0, 0, 0],
            32,
            Forbidden(F::Documentation),
        ),
        ([0x2001, 0, 0, 0, 0, 0, 0, 0], 32, Forbidden(F::Teredo)),
        ([0x2000, 0, 0, 0, 0, 0, 0, 0], 3, AddressClass::Public),
        ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7, Private(P::UniqueLocal)),
        ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10, Private(P::LinkLocal)),
        ([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10, Private(P::SiteLocal)),
        ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8, Forbidden(F::Multicast)),
    ];
    TABLE
        .iter()
        .find(|(net, prefix, _)| in_v6(ip, *net, *prefix))
        .map(|(_, _, class)| *class)
        .unwrap_or(Forbidden(F::Reserved))
}
