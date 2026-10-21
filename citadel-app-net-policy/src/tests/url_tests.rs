//! The URL shape: everything lenient parsers accept that this refuses.

use super::*;

#[test]
fn well_formed_urls_parse() {
    let url = TargetUrl::parse("https://Api.Acme.com:8443/a/b?c=d&e").unwrap();
    assert_eq!(url.scheme, Scheme::Https);
    assert_eq!(url.host, "api.acme.com");
    assert_eq!(url.port, 8443);
    assert_eq!(url.path_and_query, "/a/b?c=d&e");
    assert_eq!(
        TargetUrl::parse("https://a.io").unwrap().path_and_query,
        "/"
    );
    assert_eq!(
        TargetUrl::parse("https://a.io?x").unwrap().path_and_query,
        "/?x"
    );
    assert_eq!(TargetUrl::parse("http://a.io").unwrap().port, 80);
    assert_eq!(
        TargetUrl::parse("https://10.0.0.1/").unwrap().ip_literal(),
        Some("10.0.0.1".parse().unwrap())
    );
}

#[test]
fn every_lenient_form_is_refused() {
    for (text, fault) in [
        ("https://u@a.io/", UrlFault::Credentials),
        ("https://u:p@a.io/", UrlFault::Credentials),
        ("https://a.io/#f", UrlFault::Fragment),
        ("https://a.io/ x", UrlFault::Character),
        ("https://a.io/\\x", UrlFault::Character),
        ("https://a.io/\u{e9}", UrlFault::Character),
        ("https://a.io/\n", UrlFault::Character),
        ("HTTPS://a.io/", UrlFault::Scheme),
        ("ws://a.io/", UrlFault::Scheme),
        ("file:///etc/passwd", UrlFault::Scheme),
        ("https:/a.io/", UrlFault::Scheme),
        ("https://[::1]/", UrlFault::Host),
        ("https://a.io./", UrlFault::Host),
        ("https://.a.io/", UrlFault::Host),
        ("https:///x", UrlFault::Host),
        ("https://-a.io/", UrlFault::Host),
        ("https://a_b.io/", UrlFault::Host),
        ("https://0x7f.0.0.1/", UrlFault::Host),
        ("https://0x7f000001/", UrlFault::Host),
        ("https://2130706433/", UrlFault::Host),
        ("https://0177.0.0.1/", UrlFault::Host),
        ("https://127.1/", UrlFault::Host),
        ("https://127.0.0.01/", UrlFault::Host),
        ("https://256.0.0.1/", UrlFault::Host),
        ("https://a.io:/", UrlFault::Port),
        ("https://a.io:0/", UrlFault::Port),
        ("https://a.io:65536/", UrlFault::Port),
        ("https://a.io:0443/", UrlFault::Port),
        ("https://a.io:+443/", UrlFault::Port),
    ] {
        assert_eq!(TargetUrl::parse(text), Err(fault), "{text:?}");
    }
    let long = format!("https://a.io/{}", "x".repeat(MAX_URL_BYTES));
    assert_eq!(TargetUrl::parse(&long), Err(UrlFault::TooLong));
}
