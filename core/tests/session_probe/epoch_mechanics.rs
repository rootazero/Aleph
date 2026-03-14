//! P2: Epoch mechanics tests.
//!
//! Covers epoch increment on /new, epoch-aware key generation,
//! and history isolation across epochs.

use alephcore::routing::session_key::{DmScope, SessionKey};

#[test]
fn p2_01_main_default_epoch_is_zero() {
    let key = SessionKey::main("test");
    assert_eq!(key.epoch(), 0);
}

#[test]
fn p2_02_with_next_epoch_increments_main() {
    let k0 = SessionKey::main("test");
    assert_eq!(k0.epoch(), 0);

    let k1 = k0.with_next_epoch();
    assert_eq!(k1.epoch(), 1);

    let k2 = k1.with_next_epoch();
    assert_eq!(k2.epoch(), 2);
}

#[test]
fn p2_03_with_next_epoch_increments_dm() {
    let k0 = SessionKey::dm("test", "telegram", "alice", DmScope::PerPeer);
    assert_eq!(k0.epoch(), 0);

    let k1 = k0.with_next_epoch();
    assert_eq!(k1.epoch(), 1);
}

#[test]
fn p2_04_with_next_epoch_noop_for_non_epoch_types() {
    let task = SessionKey::task("test", "cron", "daily");
    let task_next = task.with_next_epoch();
    assert_eq!(task_next.epoch(), 0);
    assert_eq!(task_next.to_key_string(), task.to_key_string());

    let eph = SessionKey::ephemeral("test");
    let eph_next = eph.with_next_epoch();
    assert_eq!(eph_next.epoch(), 0);
    assert_eq!(eph_next.to_key_string(), eph.to_key_string());
}

#[test]
fn p2_05_to_key_string_no_suffix_for_epoch_zero() {
    let key = SessionKey::main("test");
    let s = key.to_key_string();
    assert_eq!(s, "agent:test:main");
    assert!(!s.contains(":s"));
}

#[test]
fn p2_06_to_key_string_appends_suffix_for_nonzero_epoch() {
    let k1 = SessionKey::main("test").with_next_epoch();
    assert_eq!(k1.to_key_string(), "agent:test:main:s1");

    let k3 = k1.with_next_epoch().with_next_epoch();
    assert_eq!(k3.to_key_string(), "agent:test:main:s3");
}

#[test]
fn p2_07_parse_roundtrip_preserves_epoch() {
    for epoch_val in [0u32, 1, 5, 100] {
        let key = SessionKey::Main {
            agent_id: "test".to_string(),
            main_key: "main".to_string(),
            epoch: epoch_val,
        };
        let serialized = key.to_key_string();
        let parsed = SessionKey::parse(&serialized)
            .unwrap_or_else(|| panic!("Failed to parse: {}", serialized));
        assert_eq!(
            parsed.epoch(),
            epoch_val,
            "Epoch roundtrip failed for epoch={}",
            epoch_val
        );
    }
}
