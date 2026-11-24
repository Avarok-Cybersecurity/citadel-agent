//! The op table and the capability list are the contract the host implements: their shape is
//! asserted here, against the design's literal list rather than against themselves.

use citadel_app_sdk_types::abi::{AbiError, InitResult, REQUIRED_EXPORTS};
use citadel_app_sdk_types::{Capability, Group, OPS};
use std::collections::BTreeSet;

/// Design 2.2 + R4.1, exactly. A rename or an addition must be made here deliberately.
const DESIGN_CAPABILITIES: [&str; 34] = [
    "canvas",
    "ui",
    "events",
    "navigate",
    "log",
    "nodes.read",
    "nodes.write",
    "md.edit",
    "files.read",
    "files.write",
    "chat.read",
    "chat.send",
    "members.list",
    "storage.user",
    "storage.scope",
    "storage.node",
    "storage.blob",
    "storage.files",
    "storage.local",
    "storage.node.authored",
    "net.host",
    "net.host.private",
    "net.server",
    "ihttp.serve",
    "ihttp.call",
    "ihttp.p2p.serve",
    "ihttp.p2p.call",
    "clipboard.read",
    "clipboard.write",
    "notifications",
    "documents",
    "group.topic.subscribe",
    "group.topic.publish",
    "citadel.session",
];

/// The ABI's import list, in index order, with its index. 17 is reserved for `identity`.
const DESIGN_GROUPS: [(&str, i32); 18] = [
    ("canvas", 0),
    ("ui", 1),
    ("events", 2),
    ("navigate", 3),
    ("nodes", 4),
    ("md", 5),
    ("files", 6),
    ("chat", 7),
    ("members", 8),
    ("storage", 9),
    ("net", 10),
    ("ihttp", 11),
    ("clipboard", 12),
    ("notify", 13),
    ("documents", 14),
    ("log", 15),
    ("topic", 16),
    ("session", 18),
];

#[test]
fn the_capabilities_are_the_design_list_by_name() {
    let names: Vec<&str> = Capability::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(names, DESIGN_CAPABILITIES);
    for c in Capability::ALL {
        assert_eq!(Capability::from_name(c.name()), Some(*c));
        let wire: String =
            citadel_app_sdk_types::decode(&citadel_app_sdk_types::encode(c)).unwrap();
        assert_eq!(wire, c.name(), "the serialized name is the manifest name");
    }
    assert_eq!(
        Capability::from_name("network"),
        None,
        "R1's `network` was split by R4.1"
    );
}

#[test]
fn the_groups_are_the_abi_imports_with_their_stable_indexes() {
    let groups: Vec<(&str, i32)> = Group::ALL
        .iter()
        .map(|g| (g.import_name(), g.index()))
        .collect();
    assert_eq!(groups, DESIGN_GROUPS);
    assert_eq!(Group::from_index(17), None, "reserved");
    for g in Group::ALL {
        assert_eq!(Group::from_index(g.index()), Some(*g));
        assert_eq!(Group::from_import_name(g.import_name()), Some(*g));
        let wire: String =
            citadel_app_sdk_types::decode(&citadel_app_sdk_types::encode(g)).unwrap();
        assert_eq!(wire, g.import_name());
    }
    assert_eq!(Group::from_import_name("memory"), None);
    assert_eq!(Group::from_import_name("reply_read"), None);
}

#[test]
fn every_group_is_opened_by_some_capability_and_reached_by_some_op() {
    for g in Group::ALL {
        assert!(
            Capability::opening(*g).next().is_some(),
            "{g:?} has no capability"
        );
        assert!(OPS.iter().any(|o| o.group == *g), "{g:?} has no op");
    }
}

#[test]
fn ops_are_unique_nonzero_and_need_a_capability_of_their_own_group() {
    let mut seen = BTreeSet::new();
    let mut names = BTreeSet::new();
    for op in OPS {
        assert!(op.op > 0, "{} has op 0, which is reserved", op.name);
        assert!(
            seen.insert((op.group, op.op)),
            "duplicate op {} in {:?}",
            op.op,
            op.group
        );
        assert!(names.insert(op.name), "duplicate op name {}", op.name);
        if let Some(caps) = op.granted_by {
            assert!(!caps.is_empty(), "{} can never be granted", op.name);
            for cap in caps {
                assert_eq!(
                    cap.group(),
                    op.group,
                    "{} is granted by {cap:?} from another group",
                    op.name
                );
            }
        }
    }
    let derived: Vec<&str> = OPS
        .iter()
        .filter(|o| o.granted_by.is_none())
        .map(|o| o.name)
        .collect();
    assert_eq!(
        derived,
        ["KvGet", "KvSet", "KvSetMany", "KvDelete", "KvList"]
    );
}

#[test]
fn every_granted_capability_but_the_private_network_extension_has_an_op() {
    let used: BTreeSet<Capability> = OPS
        .iter()
        .flat_map(|o| o.granted_by.unwrap_or_default())
        .copied()
        .collect();
    let kv = [
        Capability::StorageUser,
        Capability::StorageScope,
        Capability::StorageNode,
        Capability::StorageLocal,
        Capability::StorageNodeAuthored,
    ];
    for c in Capability::ALL {
        let reached = used.contains(c) || kv.contains(c);
        assert_eq!(reached, *c != Capability::NetHostPrivate, "{c:?}");
    }
}

#[test]
fn abi_error_codes_are_negative_and_distinct_and_init_ok_is_zero() {
    let codes: BTreeSet<i32> = AbiError::ALL.iter().map(|e| e.code()).collect();
    assert_eq!(codes.len(), AbiError::ALL.len());
    for e in AbiError::ALL {
        assert!(e.code() < 0);
        assert_eq!(AbiError::from_code(e.code()), Some(e));
    }
    assert_eq!(AbiError::from_code(0), None);
    assert_eq!(InitResult::Ok.code(), 0);
    assert_eq!(
        REQUIRED_EXPORTS,
        [
            "app_alloc",
            "app_free",
            "app_init",
            "app_event",
            "app_close"
        ]
    );
}

#[test]
fn the_intrinsic_capabilities_are_the_ones_that_reach_only_the_apps_own_surface() {
    let names: Vec<&str> = Capability::INTRINSIC.iter().map(|c| c.name()).collect();
    assert_eq!(names, ["canvas", "ui", "events", "navigate", "log"]);
    assert!(!Capability::NetHost.is_intrinsic());
}

/// Any-of rows: the ihttp ops admit either plane (the host then checks the plane the call
/// names), and the topic ops that a subscriber and a publisher both make.
#[test]
fn only_the_ihttp_and_topic_ops_are_granted_by_more_than_one_capability() {
    use Capability::*;
    let any_of: Vec<(&str, &[Capability])> = OPS
        .iter()
        .filter_map(|o| o.granted_by.map(|caps| (o.name, caps)))
        .filter(|(_, caps)| caps.len() > 1)
        .collect();
    let topic: &[Capability] = &[GroupTopicSubscribe, GroupTopicPublish];
    assert_eq!(
        any_of,
        [
            ("IhttpServe", &[IhttpServe, IhttpP2pServe][..]),
            ("IhttpRespond", &[IhttpServe, IhttpP2pServe][..]),
            ("IhttpCall", &[IhttpCall, IhttpP2pCall][..]),
            (
                "IhttpConsumed",
                &[IhttpServe, IhttpCall, IhttpP2pServe, IhttpP2pCall][..]
            ),
            ("TopicOpen", topic),
            ("TopicClose", topic),
        ]
    );
}

#[test]
fn alone_is_the_one_capability_as_a_static_slice() {
    for c in Capability::ALL {
        assert_eq!(c.alone(), &[*c]);
    }
}
