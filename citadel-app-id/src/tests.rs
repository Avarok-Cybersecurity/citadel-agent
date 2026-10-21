use super::*;

#[test]
fn well_formed_ids_are_accepted() {
    for id in [
        "com.acme.kanban",
        "a.b",
        "0.9",
        "net.avarok.docs-2",
        "x-1.y--z",
        &format!("a.{}", "b".repeat(MAX_APP_ID_CHARS - 2)),
    ] {
        assert_eq!(validate_app_id(id), Ok(()), "{id}");
    }
}

#[test]
fn every_malformed_id_is_refused() {
    for id in [
        "",
        "kanban",
        "com..kanban",
        ".com.kanban",
        "com.kanban.",
        "Com.acme",
        "com.-acme",
        "com.acme_kanban",
        "com.acme kanban",
        "com.acme/kanban",
        "com.acme.kanban\0",
        "com.acmé",
        "*.acme",
        &format!("a.{}", "b".repeat(MAX_APP_ID_CHARS - 1)),
    ] {
        assert!(
            matches!(validate_app_id(id), Err(IdentityRefusal::AppId(_))),
            "{id:?} was accepted"
        );
    }
}

#[test]
fn versions_parse_and_print_the_same() {
    for text in ["0.0.0", "1.4.0", "10.20.30", "4294967295.0.1"] {
        let version = AppVersion::parse(text).expect(text);
        assert_eq!(version.to_string(), text);
        assert_eq!(text.parse::<AppVersion>(), Ok(version));
    }
}

#[test]
fn every_malformed_version_is_refused() {
    for text in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "01.2.3",
        "1.02.3",
        "1.2.03",
        "1.2.3-rc.1",
        "1.2.3+build",
        "v1.2.3",
        " 1.2.3",
        "1.2.3 ",
        "1..3",
        "1.2.x",
        "4294967296.0.0",
        "-1.0.0",
        "+1.0.0",
    ] {
        assert!(
            matches!(AppVersion::parse(text), Err(IdentityRefusal::Version(_))),
            "{text:?} was accepted"
        );
    }
}

#[test]
fn versions_order_numerically_part_by_part() {
    let v = |t: &str| AppVersion::parse(t).unwrap();
    assert!(v("1.10.0") > v("1.9.9"), "numeric, not lexical");
    assert!(v("2.0.0") > v("1.99.99"));
    assert!(v("1.0.1") > v("1.0.0"));
    assert_eq!(v("1.2.3").cmp(&v("1.2.3")), Ordering::Equal);
    // Control: the lexical order of the same strings is the other way round.
    assert!("1.10.0" < "1.9.9");
}
