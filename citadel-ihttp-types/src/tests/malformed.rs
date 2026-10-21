//! Frames a hostile peer writes without `encode`: another version, unknown kinds and fields,
//! trailing bytes, oversize input.

use super::*;

#[test]
fn another_version_is_refused() {
    let mut future = frame(IhttpFrameKind::End);
    future.v = 2;
    assert_eq!(decode(&raw(&future)), Err(FrameError::Version(2)));
    future.v = 0;
    assert_eq!(decode(&raw(&future)), Err(FrameError::Version(0)));
    assert_eq!(encode(&future), Err(FrameError::Version(0)));
}

#[test]
fn unknown_kinds_fields_missing_fields_and_trailing_bytes_are_malformed() {
    #[derive(Serialize)]
    enum Fake {
        Push { x: u8 },
    }
    #[derive(Serialize)]
    struct Frame<K: Serialize> {
        v: u8,
        stream: u64,
        kind: K,
    }
    #[derive(Serialize)]
    struct Extra {
        v: u8,
        stream: u64,
        kind: &'static str,
        caller: &'static str,
    }
    #[derive(Serialize)]
    struct Missing {
        v: u8,
        kind: &'static str,
    }
    #[derive(Serialize)]
    enum Credit {
        Credit { n: u32, extra: u8 },
    }
    let unknown_kind = raw(&Frame {
        v: 1,
        stream: 1,
        kind: Fake::Push { x: 1 },
    });
    let unknown_field = raw(&Extra {
        v: 1,
        stream: 1,
        kind: "End",
        caller: "admin",
    });
    let unknown_variant_field = raw(&Frame {
        v: 1,
        stream: 1,
        kind: Credit::Credit { n: 1, extra: 1 },
    });
    let missing = raw(&Missing { v: 1, kind: "End" });
    let mut trailing = encode(&frame(IhttpFrameKind::End)).unwrap();
    trailing.push(0);
    for bytes in [
        unknown_kind,
        unknown_field,
        unknown_variant_field,
        missing,
        trailing,
        vec![0xff],
        vec![],
    ] {
        assert!(
            matches!(decode(&bytes), Err(FrameError::Malformed(_))),
            "{bytes:?}"
        );
    }
    // Control: the same writer produces a frame that decodes when it is well formed.
    let good = raw(&Frame {
        v: 1,
        stream: 1,
        kind: "End",
    });
    assert_eq!(
        decode(&good).unwrap(),
        IhttpFrame {
            v: 1,
            stream: 1,
            kind: IhttpFrameKind::End
        }
    );
}

#[test]
fn input_past_the_frame_cap_is_refused_before_parsing() {
    assert_eq!(
        decode(&vec![0; MAX_FRAME_BYTES + 1]),
        Err(FrameError::TooLarge)
    );
}
