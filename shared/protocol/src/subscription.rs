//! `events.subscribe` / `events.unsubscribe` params, and the stream topic a
//! client carves out of its subscription when it does not render it.

use serde::{Deserialize, Serialize};

/// Topic of the live reasoning frames (`StreamEvent::Reasoning` on its way to
/// a client). The gateway names the frame with this constant, and a client
/// that never renders reasoning carves it out of its `stream.*` subscription
/// with it — one name, so the two cannot drift.
pub const STREAM_REASONING_TOPIC: &str = "stream.reasoning";

/// Patterns carved out of every topic of one `events.subscribe` call, and the
/// carve-out an `events.unsubscribe` call names.
///
/// A sibling of `topics`, not a field on each topic entry: a server that
/// predates it ignores the unknown key and subscribes the plain topics — the
/// frames it would have sent anyway — where a per-entry object would make a
/// `Vec<String>` topic list reject the whole call and leave the client with no
/// stream at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicCarveOut {
    /// Topic patterns not delivered through this call's topics, although they
    /// match them. Another subscription of the same connection that matches a
    /// carved-out topic still delivers it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
}

/// Params of `events.subscribe` and `events.unsubscribe` as a client sends
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicsRequest {
    pub topics: Vec<String>,
    #[serde(flatten)]
    pub carve_out: TopicCarveOut,
}

impl TopicsRequest {
    /// `topics`, minus `except`.
    #[must_use]
    pub fn new(topics: Vec<String>, except: Vec<String>) -> Self {
        Self {
            topics,
            carve_out: TopicCarveOut { except },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_carve_out_is_a_sibling_key_and_absent_when_empty() {
        let plain =
            serde_json::to_value(TopicsRequest::new(vec!["team.*".into()], vec![])).unwrap();
        assert_eq!(plain, serde_json::json!({ "topics": ["team.*"] }));

        let carved = serde_json::to_value(TopicsRequest::new(
            vec!["stream.*".into()],
            vec![STREAM_REASONING_TOPIC.into()],
        ))
        .unwrap();
        assert_eq!(
            carved,
            serde_json::json!({ "topics": ["stream.*"], "except": ["stream.reasoning"] })
        );
    }
}
