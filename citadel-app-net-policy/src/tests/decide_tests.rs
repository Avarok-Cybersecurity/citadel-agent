//! The request checks: scheme, method, body, headers, listed host and port. Each refusal is
//! beside its control.

use super::*;

const PUBLIC: &[&str] = &["93.184.216.34"];

fn with(method: &str, body_len: u64, headers: &[(String, String)]) -> Verdict {
    let grant = host_grant(false, &["api.acme.com"]);
    let request = EgressRequest {
        method,
        url: "https://api.acme.com/v1/items?q=1",
        headers,
        body_len,
    };
    decide(&request, &[ip(PUBLIC[0])], &grant, &deployment())
}

fn header(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

#[test]
fn a_listed_https_request_is_allowed_and_pinned() {
    let Verdict::Allow(allowed) = with("GET", 0, &[]) else {
        panic!("refused")
    };
    assert_eq!(allowed.target.host, "api.acme.com");
    assert_eq!(allowed.target.path_and_query, "/v1/items?q=1");
    assert_eq!(allowed.connect, vec!["93.184.216.34:443".parse().unwrap()]);
}

#[test]
fn http_needs_private() {
    let public = host_grant(false, &["api.acme.com", "api.acme.com:80"]);
    assert_eq!(
        reason(verdict(&public, "http://api.acme.com/", PUBLIC)),
        DenyReason::Scheme
    );
    let private = host_grant(true, &["api.acme.com:80"]);
    assert!(verdict(&private, "http://api.acme.com/", PUBLIC).is_allowed());
}

#[test]
fn only_the_six_methods_are_sent() {
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD"] {
        assert!(with(method, 0, &[]).is_allowed(), "{method}");
    }
    for method in ["CONNECT", "TRACE", "OPTIONS", "get", "PROPFIND", ""] {
        assert_eq!(reason(with(method, 0, &[])), DenyReason::Method, "{method}");
    }
}

#[test]
fn the_body_is_bounded_and_absent_on_get_and_head() {
    assert!(with("POST", MAX_REQUEST_BODY_BYTES, &[]).is_allowed());
    assert_eq!(
        reason(with("POST", MAX_REQUEST_BODY_BYTES + 1, &[])),
        DenyReason::Body
    );
    assert_eq!(reason(with("GET", 1, &[])), DenyReason::Body);
    assert_eq!(reason(with("HEAD", 1, &[])), DenyReason::Body);
}

#[test]
fn only_allow_listed_headers_are_sent() {
    let allowed = [
        header("accept", "application/json"),
        header("authorization", "Bearer abc"),
        header("cookie", "a=b"),
        header("content-type", "application/json"),
        header("x-acme-trace", "1"),
    ];
    assert!(with("POST", 2, &allowed).is_allowed());
    for refused in [
        header("host", "evil.com"),
        header("content-length", "5"),
        header("transfer-encoding", "chunked"),
        header("connection", "upgrade"),
        header("upgrade", "websocket"),
        header("forwarded", "for=1.2.3.4"),
        header("x-forwarded-for", "1.2.3.4"),
        header("x-real-ip", "1.2.3.4"),
        header("proxy-authorization", "x"),
        header("origin", "https://acme.work.avarok.net"),
        header("sec-fetch-site", "none"),
        header("accept-encoding", "gzip"),
        header("Accept", "text/html"),
        header("x-", "1"),
        header("accept", "a\r\nhost: evil.com"),
        header("accept", "a\0"),
        header("accept", &"a".repeat(MAX_HEADER_BYTES)),
    ] {
        let verdict = with("POST", 2, std::slice::from_ref(&refused));
        assert_eq!(reason(verdict), DenyReason::Headers, "{refused:?}");
    }
    let many: Vec<_> = (0..=MAX_HEADERS)
        .map(|i| header(&format!("x-h{i}"), "1"))
        .collect();
    assert_eq!(reason(with("POST", 2, &many)), DenyReason::Headers);
    assert!(with("POST", 2, &many[..MAX_HEADERS]).is_allowed());
}

#[test]
fn the_host_must_be_listed_exactly_with_no_wildcard_or_suffix_match() {
    let grant = host_grant(false, &["api.acme.com"]);
    for url in [
        "https://evil.com/",
        "https://x.api.acme.com/",
        "https://acme.com/",
        "https://api.acme.com.evil.com/",
        "https://apiacme.com/",
    ] {
        assert_eq!(
            reason(verdict(&grant, url, PUBLIC)),
            DenyReason::NotListed,
            "{url}"
        );
    }
    assert!(
        verdict(&grant, "https://API.ACME.COM/", PUBLIC).is_allowed(),
        "case-insensitive"
    );
}

#[test]
fn ports_are_443_and_8443_unless_the_entry_names_one() {
    let grant = host_grant(false, &["api.acme.com", "data.acme.com:9443"]);
    assert!(verdict(&grant, "https://api.acme.com:443/", PUBLIC).is_allowed());
    assert!(verdict(&grant, "https://api.acme.com:8443/", PUBLIC).is_allowed());
    assert!(verdict(&grant, "https://data.acme.com:9443/", PUBLIC).is_allowed());
    for url in [
        "https://api.acme.com:444/",
        "https://data.acme.com/",
        "https://data.acme.com:8443/",
    ] {
        assert_eq!(
            reason(verdict(&grant, url, PUBLIC)),
            DenyReason::NotListed,
            "{url}"
        );
    }
}

#[test]
fn a_name_that_resolved_to_nothing_is_refused() {
    let grant = host_grant(false, &["api.acme.com"]);
    assert_eq!(
        reason(verdict(&grant, "https://api.acme.com/", &[])),
        DenyReason::NoAddress
    );
}

#[test]
fn a_malformed_url_is_refused_before_the_grant_is_read() {
    let grant = host_grant(true, &["api.acme.com"]);
    for url in [
        "https://user:pw@api.acme.com/",
        "https://api.acme.com@evil.com/",
        "https://api.acme.com/#x",
        "https://api.acme.com\\@evil.com/",
        "ftp://api.acme.com/",
        "//api.acme.com/",
        "https://[::1]/",
    ] {
        assert!(
            matches!(reason(verdict(&grant, url, PUBLIC)), DenyReason::Url(_)),
            "{url}"
        );
    }
}
