use aleph_protocol::memory_graph::{NoteAddress, NoteNodeDto};
use serde_json::json;

#[test]
fn missing_partition_is_renderable_but_not_addressable() {
    let mut node = json!({
        "id": "wiki/a",
        "name": "a",
        "path": "wiki/a",
        "category": "wiki",
        "tags": [],
        "link_count": 0,
    });
    let old: NoteNodeDto = serde_json::from_value(node.clone()).unwrap();
    assert_eq!(old.address(), None);
    assert_eq!(serde_json::to_value(old).unwrap(), node);

    node["agent_id"] = json!("main__u-alice");
    let current: NoteNodeDto = serde_json::from_value(node).unwrap();
    assert_eq!(
        current.address(),
        Some(NoteAddress {
            agent_id: "main__u-alice".to_owned(),
            node_id: "wiki/a".to_owned(),
        })
    );
    assert_ne!(
        current.address(),
        Some(NoteAddress {
            agent_id: "main".to_owned(),
            node_id: "wiki/a".to_owned(),
        })
    );
}
