//! IPv6 literals as a server's canonical domain (owner, 2026-11-19).
//!
//! A server reached at `[2001:db8::1]:12349` had no canonical domain: the port was split at the
//! LAST `:` and the host had to be `[a-z0-9-.]` labels, so the agent could derive no RE-VFS key
//! and refused the sign-in. A bracketed literal is now parsed as an `Ipv6Addr` and written in its
//! one compressed form.
//!
//! Every other input must canonicalise exactly as before, because the canonical domain is an
//! input of every account's keys: `FROZEN` is the pre-change output of the old implementation
//! over a corpus of names, IPv4 addresses, ports, paths and malformed input, recorded by running
//! it, error values included.
//!
//! Controls: write the address back as typed (`format!("[{literal}]")`) and
//! `one_address_has_one_canonical_form` goes red; send bracketed input down the name path again
//! and `bracketed_ipv6_literals_are_server_names` goes red; accept an unbracketed literal and
//! `unbracketed_and_malformed_ipv6_is_refused` goes red.

use citadel_identity::canonical_domain;

/// (dialled, `{:?}` of the old `canonical_domain(dialled).map(as_str)`).
const FROZEN: &[(&str, &str)] = &[
    ("main.work.avarok.net", r#"Ok("main.work.avarok.net")"#),
    (
        "wss://Main.Work.Avarok.NET/",
        r#"Ok("main.work.avarok.net")"#,
    ),
    (
        "https://main.work.avarok.net:443//",
        r#"Ok("main.work.avarok.net")"#,
    ),
    (
        "wss://host.example:443/tenant-a/",
        r#"Ok("host.example/tenant-a")"#,
    ),
    ("host.example:8443", r#"Ok("host.example:8443")"#),
    ("localhost", r#"Ok("localhost")"#),
    ("LOCALHOST:12349", r#"Ok("localhost:12349")"#),
    ("127.0.0.1", r#"Ok("127.0.0.1")"#),
    ("127.0.0.1:12349", r#"Ok("127.0.0.1:12349")"#),
    ("127.0.0.1:443", r#"Ok("127.0.0.1")"#),
    ("wss://127.0.0.1:12349", r#"Ok("127.0.0.1:12349")"#),
    ("ws://10.0.0.7:80/", r#"Ok("10.0.0.7:80")"#),
    ("192.168.1.20:8080/t/x", r#"Ok("192.168.1.20:8080/t/x")"#),
    ("a-b.c-d.example", r#"Ok("a-b.c-d.example")"#),
    ("xn--bcher-kva.example", r#"Ok("xn--bcher-kva.example")"#),
    ("1.2.3", r#"Ok("1.2.3")"#),
    ("host.example/A/B", r#"Ok("host.example/a/b")"#),
    (
        "wss://work.avarok.net/tenant_1/",
        r#"Ok("work.avarok.net/tenant_1")"#,
    ),
    ("", r#"Err(Empty)"#),
    ("a b.example", r#"Err(ForbiddenChar(' '))"#),
    ("user@host.example", r#"Err(ForbiddenChar('@'))"#),
    ("host.example?x=1", r#"Err(ForbiddenChar('?'))"#),
    ("host.example#f", r#"Err(ForbiddenChar('#'))"#),
    (
        "-host.example",
        r#"Err(BadHost("label \"-host\" edges on '-'"))"#,
    ),
    ("host..example", r#"Err(BadHost("label \"\""))"#),
    ("host.example:0", r#"Err(BadPort("0"))"#),
    ("host.example:99999", r#"Err(BadPort("99999"))"#),
    ("host.example:", r#"Err(BadPort(""))"#),
    ("host.example/a/../b", r#"Err(BadPath("segment \"..\""))"#),
    ("1x://host.example", r#"Err(BadScheme)"#),
    ("höst.example", r#"Err(ForbiddenChar('ö'))"#),
    ("host.example:08443", r#"Err(BadPort("08443"))"#),
    (
        "host_name.example",
        r#"Err(BadHost("label \"host_name\""))"#,
    ),
    ("host.example:+443", r#"Err(BadPort("+443"))"#),
    (":443", r#"Err(Empty)"#),
    ("/path", r#"Err(Empty)"#),
    ("host.example//x", r#"Err(BadPath("segment \"\""))"#),
    (
        "a.-b.example",
        r#"Err(BadHost("label \"-b\" edges on '-'"))"#,
    ),
    ("host.example:1:2", r#"Err(BadHost("label \"example:1\""))"#),
    ("ws://", r#"Err(Empty)"#),
    ("]host.example", r#"Err(BadHost("label \"]host\""))"#),
    ("host[.example", r#"Err(BadHost("label \"host[\""))"#),
];

#[test]
fn names_and_ipv4_canonicalise_exactly_as_before() {
    for (dialled, before) in FROZEN {
        let now = canonical_domain(dialled).map(|d| d.as_str().to_string());
        assert_eq!(&format!("{now:?}"), before, "{dialled:?} changed");
        if let Ok(once) = now {
            assert_eq!(
                canonical_domain(&once).unwrap().as_str(),
                once,
                "not idempotent"
            );
        }
    }
}

#[test]
fn bracketed_ipv6_literals_are_server_names() {
    let cases = [
        ("[2001:db8::1]", "[2001:db8::1]"),
        ("[2001:db8::1]:12349", "[2001:db8::1]:12349"),
        ("[2001:db8::1]:443", "[2001:db8::1]"),
        ("wss://[2001:db8::1]:12349/", "[2001:db8::1]:12349"),
        (
            "wss://[2001:db8::1]:443/tenant-a/",
            "[2001:db8::1]/tenant-a",
        ),
        ("https://[::1]", "[::1]"),
        ("[::ffff:192.0.2.1]:8443", "[::ffff:192.0.2.1]:8443"),
    ];
    for (dialled, expected) in cases {
        let once = canonical_domain(dialled).unwrap_or_else(|e| panic!("{dialled:?}: {e}"));
        assert_eq!(once.as_str(), expected, "{dialled:?}");
        assert_eq!(
            canonical_domain(once.as_str()).unwrap(),
            once,
            "not idempotent: {dialled:?}"
        );
    }
}

#[test]
fn one_address_has_one_canonical_form() {
    let spellings = [
        "[2001:db8::1]:12349",
        "[2001:DB8::1]:12349",
        "[2001:0db8:0000:0000:0000:0000:0000:0001]:12349",
        "[2001:db8:0:0::1]:12349",
        "wss://[2001:Db8::0:1]:12349/",
    ];
    for dialled in spellings {
        assert_eq!(
            canonical_domain(dialled).unwrap().as_str(),
            "[2001:db8::1]:12349",
            "{dialled:?}"
        );
    }
}

#[test]
fn unbracketed_and_malformed_ipv6_is_refused() {
    let refused = [
        "2001:db8::1",
        "2001:db8::1:12349",
        "wss://2001:db8::1/",
        "[2001:db8::1",
        "2001:db8::1]",
        "[2001:db8::1]12349",
        "[2001:db8::1]:",
        "[2001:db8::1]:0",
        "[2001:db8::1]:99999",
        "[2001:db8::g]",
        "[]",
        "[192.0.2.1]",
        "[host.example]",
        "[[2001:db8::1]]",
        "[2001:db8::1%25en0]",
        "[2001:db8::1]:12349:1",
    ];
    for dialled in refused {
        assert!(canonical_domain(dialled).is_err(), "{dialled:?} accepted");
    }
}
