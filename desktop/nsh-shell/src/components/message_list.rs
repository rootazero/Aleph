// Scrollable native message history for NSH Desktop.
//
// Web fallback strategy: a keyed list in a scrollable container, with the
// active assistant response rendered as one stable row while it streams.

use nsh_core::prelude::*;

use crate::session::{Message, Role};

/// Whether a growing transcript should animate to its end.
fn should_scroll_to_end(previous_count: usize, next_count: usize) -> bool {
    next_count > previous_count
}

/// Append a streamed delta to the stable, temporary assistant message.
pub(crate) fn append_stream_delta(
    messages: &[Message],
    streaming_id: &str,
    timestamp: &str,
    delta: &str,
) -> Vec<Message> {
    let mut next = messages.to_vec();
    if let Some(message) = next
        .iter_mut()
        .find(|message| message.message_id == streaming_id)
    {
        message.content.push_str(delta);
    } else {
        next.push(Message {
            message_id: streaming_id.to_owned(),
            role: Role::Assistant,
            content: delta.to_owned(),
            timestamp: timestamp.to_owned(),
            tool_calls: Vec::new(),
        });
    }
    next
}

/// Replace the temporary streamed row with the completed backend message.
pub(crate) fn finish_stream(
    messages: &[Message],
    streaming_id: &str,
    completed: Message,
) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| message.message_id != streaming_id)
        .cloned()
        .chain(std::iter::once(completed))
        .collect()
}

/// Format one transcript row, including a visible progress state before the
/// first token arrives.
fn display_message(message: &Message) -> String {
    let role = match message.role {
        Role::User => "You",
        Role::Assistant => "Assistant",
    };

    if message.message_id.starts_with("streaming-") && message.content.is_empty() {
        format!("[{role}] {} — Assistant is responding…", message.timestamp)
    } else {
        format!("[{role}] {}\n{}", message.timestamp, message.content)
    }
}

/// Render a keyed, vertically scrollable transcript.
pub fn message_list(messages: impl IntoReactive<Vec<Message>>) -> impl IntoElement {
    let messages = messages.into_reactive();
    let scroll_ref = element_ref();
    let previous_count = signal(0_usize);
    let messages_for_layout = messages.clone();
    let previous_count_for_layout = previous_count;
    let scroll_ref_for_layout = scroll_ref;

    let rows = each(
        messages,
        |message| message.message_id.clone(),
        |message, _| text(move || display_message(&message.get())),
    );

    let content = view().child(rows).on_layout(move |_: &Bounds<Points>| {
        let next_count = messages_for_layout.get().len();
        let previous = previous_count_for_layout.get();
        if should_scroll_to_end(previous, next_count) {
            scroll_ref_for_layout.scroll_to_end(true);
        }
        if previous != next_count {
            previous_count_for_layout.set(next_count);
        }
    });

    scroll().element_ref(&scroll_ref).child(content)
}

#[cfg(test)]
mod tests {
    use super::{append_stream_delta, display_message, finish_stream, should_scroll_to_end};
    use crate::session::{Message, Role};

    fn message(message_id: &str, role: Role, content: &str) -> Message {
        Message {
            message_id: message_id.to_string(),
            role,
            content: content.to_string(),
            timestamp: "2026-11-05T12:34:56Z".to_string(),
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn display_includes_role_timestamp_and_content() {
        let rendered = display_message(&message("user-1", Role::User, "Hello"));

        assert!(rendered.contains("You"));
        assert!(rendered.contains("2026-11-05T12:34:56Z"));
        assert!(rendered.contains("Hello"));
    }

    #[test]
    fn empty_stream_message_shows_progress_indicator() {
        let rendered = display_message(&message("streaming-1", Role::Assistant, ""));

        assert!(rendered.contains("Assistant is responding"));
    }

    #[test]
    fn streamed_deltas_update_one_stable_message() {
        let messages = append_stream_delta(&[], "streaming-1", "timestamp", "Hello");
        let messages = append_stream_delta(&messages, "streaming-1", "ignored", " world");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "streaming-1");
        assert_eq!(messages[0].content, "Hello world");
    }

    #[test]
    fn final_message_replaces_streaming_placeholder() {
        let streaming = append_stream_delta(&[], "streaming-1", "timestamp", "partial");
        let final_message = message("assistant-1", Role::Assistant, "complete");
        let messages = finish_stream(&streaming, "streaming-1", final_message);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "assistant-1");
        assert_eq!(messages[0].content, "complete");
    }

    #[test]
    fn auto_scroll_runs_only_when_a_row_is_added() {
        assert!(should_scroll_to_end(1, 2));
        assert!(!should_scroll_to_end(2, 2));
        assert!(!should_scroll_to_end(2, 1));
    }
}
