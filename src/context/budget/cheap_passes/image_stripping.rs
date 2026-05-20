//! `HistoricalImageStrippingStage` — drop images from all messages preceding
//! the newest image-bearing turn (beyond the fresh tail), saving ~1500
//! tokens per image.
//!
//! Hermes-borrowed heuristic: the most recent screenshot/image is "live"
//! context; older ones are historical and can be summarised as a text
//! placeholder. The 1500-token estimate matches Anthropic's image pricing
//! and Hermes' constant.

use crate::context::budget::ContextPressure;
use crate::providers::message::{ContentBlock, UnifiedMessage};
use async_trait::async_trait;

/// Estimated tokens per image (matches Anthropic's pricing model + Hermes).
const IMAGE_TOKENS_ESTIMATE: usize = 1500;

#[derive(Default)]
pub struct HistoricalImageStrippingStage;

#[async_trait]
impl crate::context::budget::preflight::PreflightStage for HistoricalImageStrippingStage {
    fn name(&self) -> &'static str {
        "historical_image_stripping"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        _pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        // Locate the newest image-bearing message — that one stays intact.
        let newest_image_idx = messages
            .iter()
            .enumerate()
            .rev()
            .find(|(_, m)| message_has_image(m))
            .map(|(i, _)| i);
        let Some(newest_image_idx) = newest_image_idx else {
            return 0; // no images in history
        };

        // Strip from older messages only, AND never strip from the fresh tail.
        let cut_end = messages.len().saturating_sub(fresh_tail_count);
        let upper_bound = cut_end.min(newest_image_idx);

        let mut total_freed = 0usize;
        for msg in messages.iter_mut().take(upper_bound) {
            let stripped = strip_images_in_place(msg);
            total_freed += stripped * IMAGE_TOKENS_ESTIMATE;
        }
        total_freed
    }
}

fn message_has_image(msg: &UnifiedMessage) -> bool {
    msg.content_blocks()
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }))
}

/// Replace each `Image` block in-place with a short text placeholder.
/// Returns the number of images replaced.
fn strip_images_in_place(msg: &mut UnifiedMessage) -> usize {
    let blocks = msg.content_blocks_mut();
    let mut count = 0usize;
    for block in blocks.iter_mut() {
        if matches!(block, ContentBlock::Image { .. }) {
            *block = ContentBlock::Text {
                text: "[image stripped from history]".to_string(),
                cache_control: None,
            };
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::preflight::PreflightStage;

    fn pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 8000,
            budget_tokens: 10000,
            ratio: 0.8,
            overhead_tokens: 0,
            available_for_messages: 2000,
        }
    }

    fn user_with_image_and_text(text: &str) -> UnifiedMessage {
        UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "fake_b64".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            },
        ])
    }

    #[tokio::test]
    async fn strips_older_images_keeps_newest() {
        let mut messages = vec![
            user_with_image_and_text("old turn 1"),
            UnifiedMessage::user("middle text"),
            user_with_image_and_text("newest image turn"),
            UnifiedMessage::user("fresh 1"),
            UnifiedMessage::user("fresh 2"),
        ];
        let stage = HistoricalImageStrippingStage;
        let freed = stage.prepare(&mut messages, &pressure(), 2).await;
        assert_eq!(
            freed, IMAGE_TOKENS_ESTIMATE,
            "should strip exactly one image (the oldest), got {freed}"
        );
        // Oldest image gone:
        assert!(!message_has_image(&messages[0]));
        // Newest image still present:
        assert!(message_has_image(&messages[2]));
    }

    #[tokio::test]
    async fn no_op_when_no_images() {
        let mut messages = vec![
            UnifiedMessage::user("a"),
            UnifiedMessage::user("b"),
            UnifiedMessage::user("c"),
        ];
        let stage = HistoricalImageStrippingStage;
        let freed = stage.prepare(&mut messages, &pressure(), 1).await;
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn protects_image_in_fresh_tail() {
        let mut messages = vec![
            UnifiedMessage::user("old text"),
            user_with_image_and_text("fresh image"),
            UnifiedMessage::user("fresh text"),
        ];
        // fresh_tail=2 protects last two — image at index 1 is in the tail
        let stage = HistoricalImageStrippingStage;
        let freed = stage.prepare(&mut messages, &pressure(), 2).await;
        assert_eq!(freed, 0, "fresh-tail image must survive");
        assert!(message_has_image(&messages[1]));
    }

    #[tokio::test]
    async fn strips_multiple_images_in_single_old_turn() {
        let triple_image_msg = UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "a".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Image {
                data: "b".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Image {
                data: "c".to_string(),
                mime_type: "image/png".to_string(),
            },
        ]);
        let mut messages = vec![
            triple_image_msg,
            user_with_image_and_text("newest image"),
            UnifiedMessage::user("fresh tail 1"),
            UnifiedMessage::user("fresh tail 2"),
        ];
        let stage = HistoricalImageStrippingStage;
        let freed = stage.prepare(&mut messages, &pressure(), 2).await;
        assert_eq!(freed, 3 * IMAGE_TOKENS_ESTIMATE);
    }
}
