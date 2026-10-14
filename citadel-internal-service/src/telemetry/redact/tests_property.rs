//! A generated corpus: no known username, path or IP survives, and every peer cid does.

use super::*;

/// xorshift64*: deterministic, so a failure reproduces from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn word(&mut self, len: usize) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..len)
            .map(|_| ALPHABET[(self.next() % ALPHABET.len() as u64) as usize] as char)
            .collect()
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

struct Case {
    line: String,
    must_vanish: Vec<String>,
    peer_cid: u64,
}

fn case(rng: &mut Rng, users: &[String]) -> Case {
    let user = users[(rng.next() % users.len() as u64) as usize].clone();
    let folder = format!("d{}", rng.word(6));
    let path = format!("/home/{user}/{folder}/f{}.txt", rng.word(4));
    let v4 = format!(
        "{}.{}.{}.{}",
        1 + rng.next() % 223,
        rng.next() % 256,
        rng.next() % 256,
        1 + rng.next() % 254
    );
    let v6 = format!(
        "2001:db8:{:x}::{:x}",
        rng.next() % 0xffff,
        1 + rng.next() % 0xffff
    );
    let peer_cid = rng.next();
    let verb = rng.pick(&["failed", "refused", "timed out", "dropped"]);
    let line = match rng.next() % 4 {
        0 => format!(
            "peer {peer_cid} ({user}) {verb} at {v4}:{}",
            rng.next() % 65535
        ),
        1 => format!("upload {path} to {peer_cid} {verb}; via [{v6}]"),
        2 => format!("{user}: P2P to {peer_cid} {verb}, route {v4} -> {v6}"),
        _ => format!("cid {peer_cid} {verb} reading {path} for user={user}"),
    };
    Case {
        line,
        must_vanish: vec![user, folder, v4, v6],
        peer_cid,
    }
}

#[test]
fn no_known_value_path_or_ip_survives_and_peer_cids_do() {
    let mut rng = Rng(0x5eed_cafe_f00d_0001);
    let users: Vec<String> = (0..12).map(|_| format!("u{}", rng.word(7))).collect();
    let known: Vec<KnownValue> = users
        .iter()
        .map(|u| KnownValue {
            kind: RedactionKind::Username,
            value: u.clone(),
        })
        .collect();
    let salt = Salt::from_bytes([9; 32]);
    for _ in 0..2_000 {
        let case = case(&mut rng, &users);
        let out = redact(&case.line, &known, &salt);
        for gone in &case.must_vanish {
            assert!(
                !out.contains(gone.as_str()),
                "{gone:?} survived: {} -> {out}",
                case.line
            );
        }
        assert!(
            out.contains(&case.peer_cid.to_string()),
            "peer cid lost: {} -> {out}",
            case.line
        );
    }
}
