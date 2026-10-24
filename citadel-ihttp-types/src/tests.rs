use crate::*;
use serde::Serialize;

pub(super) fn frame(kind: IhttpFrameKind) -> IhttpFrame {
    IhttpFrame {
        v: IHTTP_VERSION,
        stream: 7,
        kind,
    }
}

pub(super) fn head(path: &str, headers: Vec<IhttpHeader>) -> IhttpFrame {
    frame(IhttpFrameKind::RequestHead {
        method: IhttpMethod::Post,
        path: path.to_string(),
        query: Some("a=1&b=2".to_string()),
        headers,
        body_len: Some(5),
        app_id: "com.acme.kanban".to_string(),
        scope_key: "node:abc".to_string(),
    })
}

pub(super) fn header(name: &str, value: &str) -> IhttpHeader {
    IhttpHeader {
        name: name.to_string(),
        value: value.to_string(),
    }
}

/// CBOR written without `encode`'s checks, as a hostile peer would.
pub(super) fn raw<T: Serialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).unwrap();
    out
}

#[test]
fn every_kind_round_trips() {
    for kind in [
        head("/cards/1", vec![header("content-type", "application/json")]).kind,
        IhttpFrameKind::ResponseHead {
            status: 200,
            headers: vec![],
            body_len: None,
        },
        IhttpFrameKind::Body {
            bytes: vec![1; MAX_BODY_CHUNK_BYTES],
        },
        IhttpFrameKind::End,
        IhttpFrameKind::Cancel {
            reason: CancelReason::Timeout,
        },
        IhttpFrameKind::Credit {
            n: MAX_CREDIT_GRANT,
        },
    ] {
        let original = frame(kind);
        let bytes = encode(&original).unwrap();
        assert!(bytes.len() <= MAX_FRAME_BYTES);
        assert_eq!(decode(&bytes).unwrap(), original);
    }
}

#[test]
fn every_cancel_reason_round_trips_under_its_own_name() {
    use CancelReason::*;
    let all = [
        Caller,
        Timeout,
        TooLarge,
        Busy,
        RateLimited,
        NoRoute,
        Refused,
        Failed,
        Closed,
        Protocol,
        Unavailable,
    ];
    for reason in all {
        let bytes = encode(&frame(IhttpFrameKind::Cancel { reason })).unwrap();
        let name = format!("{reason:?}");
        assert!(
            bytes.windows(name.len()).any(|w| w == name.as_bytes()),
            "{name}"
        );
        assert_eq!(
            decode(&bytes).unwrap().kind,
            IhttpFrameKind::Cancel { reason }
        );
    }
}

#[test]
fn the_citadel_prefix_is_the_hosts_and_its_lookalikes_are_not() {
    for path in ["/_citadel/version", "/_citadel/", "/_citadel"] {
        assert!(is_reserved_path(path), "{path}");
    }
    for path in ["/_citadelx", "/a/_citadel/version", "/", "/citadel/version"] {
        assert!(!is_reserved_path(path), "control: {path}");
    }
    assert!(
        encode(&head("/_citadel/version", vec![])).is_ok(),
        "the host still receives its own path"
    );
}

#[test]
fn a_body_travels_as_a_cbor_byte_string() {
    let bytes = encode(&frame(IhttpFrameKind::Body {
        bytes: vec![0xaa; 3],
    }))
    .unwrap();
    // Major type 2 (byte string) of length 3, then the bytes: not an array of integers.
    assert!(bytes.windows(4).any(|w| w == [0x43, 0xaa, 0xaa, 0xaa]));
}

#[test]
fn an_oversize_path_query_or_header_set_is_refused_with_a_control() {
    let at_cap = format!("/{}", "p".repeat(MAX_PATH_BYTES - 1 - "a=1&b=2".len()));
    assert!(
        encode(&head(&at_cap, vec![])).is_ok(),
        "control: exactly at the cap"
    );
    let over = format!("{at_cap}p");
    assert_eq!(encode(&head(&over, vec![])), Err(FrameError::Path));
    let many = (0..=MAX_HEADERS)
        .map(|i| header(&format!("x-{i}"), "v"))
        .collect();
    assert_eq!(encode(&head("/x", many)), Err(FrameError::Headers));
    let long = header("x-long", &"v".repeat(MAX_HEADER_BYTES));
    assert_eq!(encode(&head("/x", vec![long])), Err(FrameError::Headers));
    let fits = header("x-long", &"v".repeat(MAX_HEADER_BYTES - "x-long".len()));
    assert!(encode(&head("/x", vec![fits])).is_ok());
}

#[test]
fn framing_headers_and_unprintable_values_are_refused() {
    for bad in [
        header("content-length", "5"),
        header("transfer-encoding", "chunked"),
        header("host", "evil"),
        header("connection", "close"),
        header("Content-Type", "x"),
        header("", "x"),
        header("x-a", "1\r\nhost: evil"),
        header("x-a", "\0"),
    ] {
        assert_eq!(
            encode(&head("/x", vec![bad.clone()])),
            Err(FrameError::Headers),
            "{bad:?}"
        );
    }
}

#[test]
fn paths_that_could_confuse_a_router_are_refused() {
    for path in [
        "", "x", "/a?b", "/a#b", "/a\\b", "/a//b", "/./a", "/a/..", "/a/../b", "/a b",
    ] {
        assert_eq!(
            encode(&head(path, vec![])),
            Err(FrameError::Path),
            "{path:?}"
        );
    }
    assert!(
        encode(&head("/a/.b/c..d/", vec![])).is_ok(),
        "control: dots inside segments"
    );
}

#[test]
fn body_lengths_and_chunks_are_bounded() {
    let mut big = head("/x", vec![]);
    if let IhttpFrameKind::RequestHead { body_len, .. } = &mut big.kind {
        *body_len = Some(MAX_REQUEST_BODY_BYTES + 1);
    }
    assert_eq!(encode(&big), Err(FrameError::Body));
    let response = |len| {
        frame(IhttpFrameKind::ResponseHead {
            status: 200,
            headers: vec![],
            body_len: Some(len),
        })
    };
    assert!(encode(&response(MAX_RESPONSE_BODY_BYTES)).is_ok());
    assert_eq!(
        encode(&response(MAX_RESPONSE_BODY_BYTES + 1)),
        Err(FrameError::Body)
    );
    let chunk = |n| frame(IhttpFrameKind::Body { bytes: vec![0; n] });
    assert_eq!(
        encode(&chunk(MAX_BODY_CHUNK_BYTES + 1)),
        Err(FrameError::Body)
    );
    assert_eq!(encode(&chunk(0)), Err(FrameError::Body));
    // A hostile peer skipping `encode` is refused by `decode` all the same.
    assert_eq!(
        decode(&raw(&chunk(MAX_BODY_CHUNK_BYTES + 1))),
        Err(FrameError::Body)
    );
    assert_eq!(decode(&raw(&chunk(0))), Err(FrameError::Body));
}

#[test]
fn ids_status_and_credit_are_checked() {
    let mut bad_app = head("/x", vec![]);
    if let IhttpFrameKind::RequestHead {
        app_id, scope_key, ..
    } = &mut bad_app.kind
    {
        *app_id = "Not An Id".to_string();
        assert_eq!(scope_key, "node:abc");
    }
    assert_eq!(decode(&raw(&bad_app)), Err(FrameError::AppId));
    let mut bad_scope = head("/x", vec![]);
    if let IhttpFrameKind::RequestHead { scope_key, .. } = &mut bad_scope.kind {
        *scope_key = "has space".to_string();
    }
    assert_eq!(decode(&raw(&bad_scope)), Err(FrameError::ScopeKey));
    for status in [0, 99, 600, u16::MAX] {
        let f = frame(IhttpFrameKind::ResponseHead {
            status,
            headers: vec![],
            body_len: None,
        });
        assert_eq!(decode(&raw(&f)), Err(FrameError::Status), "{status}");
    }
    for n in [0, MAX_CREDIT_GRANT + 1] {
        assert_eq!(
            decode(&raw(&frame(IhttpFrameKind::Credit { n }))),
            Err(FrameError::Credit)
        );
    }
}

mod malformed;

#[cfg(feature = "typescript")]
mod bindings;
