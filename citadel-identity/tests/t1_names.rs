//! T1 (rev b): names. `handle@domain` vs legacy, handle rules, `display_handle`, the 255 cap,
//! and the canonical-domain rule T2 also leans on.
//!
//! Control: accept a second `@` (drop the `domain.contains('@')` refusal in `parse_username`)
//! → `a@b@c` parses and `two_at_signs_are_refused` goes red.

mod common;

use citadel_identity::*;
use common::{domain, former_domain, main_domain};

#[test]
fn make_username_joins_handle_and_canonical_domain() {
    assert_eq!(
        make_username("alice", &main_domain()).unwrap(),
        "alice@main.work.avarok.net"
    );
    assert_eq!(
        make_username("john.doe", &main_domain()).unwrap(),
        "john.doe@main.work.avarok.net"
    );
    assert!(make_username("Alice", &main_domain()).is_err());
}

#[test]
fn parse_tells_v2_from_legacy_including_dotted_legacy_names() {
    assert_eq!(
        parse_username("alice@acme.example").unwrap(),
        Username::V2 {
            handle: "alice",
            domain: "acme.example"
        }
    );
    assert_eq!(
        parse_username("john.doe@acme.example").unwrap(),
        Username::V2 {
            handle: "john.doe",
            domain: "acme.example"
        }
    );
    assert_eq!(
        parse_username("john.doe").unwrap(),
        Username::Legacy("john.doe")
    );
    assert_eq!(parse_username("bob").unwrap(), Username::Legacy("bob"));
    assert_eq!(parse_username(""), Err(NameError::Empty));
}

#[test]
fn two_at_signs_are_refused() {
    assert_eq!(parse_username("a@b@c"), Err(NameError::TooManyAt));
    assert_eq!(
        parse_username("alice@acme.example@evil.example"),
        Err(NameError::TooManyAt)
    );
    assert_eq!(parse_username("@@"), Err(NameError::TooManyAt));
}

#[test]
fn a_v2_domain_must_already_be_canonical() {
    for name in [
        "alice@ACME.example",
        "alice@acme.example/",
        "alice@acme.example:443",
        "alice@wss://acme.example",
    ] {
        assert!(parse_username(name).is_err(), "{name} parsed");
    }
    assert!(matches!(
        parse_username("alice@"),
        Err(NameError::Domain(DomainError::Empty))
    ));
}

#[test]
fn handles_follow_the_rev_b_rules() {
    for ok in ["abc", "john.doe", "a-b_c", "x.y.z", &"a".repeat(64)] {
        assert_eq!(validate_handle(ok), Ok(()), "{ok}");
    }
    let refused: [(&str, NameError); 9] = [
        ("ab", NameError::HandleLength { len: 2 }),
        (&"a".repeat(65), NameError::HandleLength { len: 65 }),
        ("al@ce", NameError::HandleChar('@')),
        ("al ce", NameError::HandleChar(' ')),
        ("Alice", NameError::HandleChar('A')),
        ("al..ce", NameError::HandleDots),
        (".alice", NameError::HandleDots),
        ("alice.", NameError::HandleDots),
        ("alicé", NameError::HandleChar('é')),
    ];
    for (handle, why) in refused {
        assert_eq!(validate_handle(handle), Err(why), "{handle}");
    }
    assert!(parse_username("al..ce@acme.example").is_err());
    assert!(parse_username(".alice@acme.example").is_err());
}

#[test]
fn display_handle_strips_only_an_accepted_domain() {
    let accepted = [main_domain(), former_domain()];
    assert_eq!(
        display_handle("alice@main.work.avarok.net", &accepted),
        "alice"
    );
    assert_eq!(
        display_handle("alice@main-old.work.avarok.net", &accepted),
        "alice"
    );
    assert_eq!(
        display_handle("alice@other.example", &accepted),
        "alice@other.example"
    );
    assert_eq!(display_handle("john.doe", &accepted), "john.doe");
    assert_eq!(
        display_handle("a@b@main.work.avarok.net", &accepted),
        "a@b@main.work.avarok.net"
    );
    assert_eq!(
        display_handle("alice@main.work.avarok.net", &[]),
        "alice@main.work.avarok.net"
    );
}

#[test]
fn the_whole_name_is_capped_at_255_bytes() {
    // 63 + 1 + 63 + 1 + 62 = 190 bytes, the domain maximum.
    let long = format!("{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(62));
    let d = domain(&long);
    let name = make_username(&"h".repeat(64), &d).unwrap();
    assert_eq!(name.len(), MAX_USERNAME_BYTES);
    assert!(parse_username(&name).is_ok());
    assert!(matches!(
        canonical_domain(&format!("{long}c")),
        Err(DomainError::TooLong { len: 191 })
    ));
    assert_eq!(
        parse_username(&"x".repeat(256)),
        Err(NameError::TooLong { len: 256 })
    );
    assert_eq!(MAX_HANDLE_BYTES + 1 + MAX_DOMAIN_BYTES, MAX_USERNAME_BYTES);
}

#[test]
fn canonical_domain_strips_scheme_slashes_and_443_and_keeps_a_path() {
    let cases = [
        ("main.work.avarok.net", "main.work.avarok.net"),
        ("wss://Main.Work.Avarok.NET/", "main.work.avarok.net"),
        ("https://main.work.avarok.net:443//", "main.work.avarok.net"),
        ("wss://host.example:443/tenant-a/", "host.example/tenant-a"),
        ("host.example:8443", "host.example:8443"),
        ("localhost", "localhost"),
    ];
    for (dialled, expected) in cases {
        let once = canonical_domain(dialled).unwrap();
        assert_eq!(once.as_str(), expected, "{dialled}");
        assert_eq!(
            canonical_domain(once.as_str()).unwrap(),
            once,
            "not idempotent: {dialled}"
        );
    }
    let refused = [
        "",
        "a b.example",
        "user@host.example",
        "host.example?x=1",
        "host.example#f",
        "-host.example",
        "host..example",
        "host.example:0",
        "host.example:99999",
        "host.example:",
        "host.example/a/../b",
        "1x://host.example",
        "höst.example",
    ];
    for dialled in refused {
        assert!(canonical_domain(dialled).is_err(), "{dialled:?} accepted");
    }
}
