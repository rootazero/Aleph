//! Carry the live screen view across context compaction.
//!
//! Two layers run back to back on the same message vector and, until this
//! module existed, disagreed about the newest image.
//! [`HistoricalImageStrippingStage`](crate::context::budget::cheap_passes::image_stripping)
//! runs first and encodes an explicit policy — the most recent screenshot is
//! *live* context, older ones are historical — replacing every older
//! `ContentBlock::Image` with a text placeholder while going out of its way to
//! leave the newest one intact. Compaction runs immediately after on the same
//! vector, and its window is head-anchored, so as soon as the protected image
//! sits before `cut_end` it is inside the drained range with nothing to carry
//! it out: [`preserved_user_messages`](super::preserve::preserved_user_messages)
//! rebuilds surviving user turns as text only, `serialize_transcript` never
//! sees images (`text_content` skips them), and the other two carriers hold the
//! plan and the file ledger. One layer stated a policy; the next silently
//! violated it.
//!
//! The compounding part is that images are a large part of *why* a desktop run
//! crosses the compaction threshold at all (`estimate_message_tokens_aware`
//! charges `IMAGE_TOKENS_ESTIMATE` apiece) — so compaction fired because of the
//! screenshots and then deleted every one of them, including the one the
//! previous layer had just protected. The model's next action was then computed
//! against prose describing a screen it could no longer see: it either
//! re-screenshots (paying the tokens again, re-triggering pressure) or acts on
//! stale coordinates. Silent either way, and the stripping stage's own test
//! stayed green because it only ever measured its own stage.
//!
//! **Exactly one image, always the newest.** This is not the general
//! re-attachment that `preserved_user_messages` deliberately refuses — that doc
//! rejects re-attaching *every* surviving turn's attachments because the cost
//! grows with each cycle. One image is a flat `IMAGE_TOKENS_ESTIMATE` (1500)
//! per compaction regardless of how many were in the window, and it is the same
//! single image the stripping stage already decided was live. The two policies
//! are the same policy; this module is what makes the second layer honour it.
//!
//! Pure — no I/O, no session key, no store lookup. Everything needed is in the
//! messages being drained.

use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Stable sentinel opening a carried-over screen view.
///
/// Unlike [`super::plan_carry`], no recognition pass is needed to make a second
/// compaction idempotent: the carried message holds a real `Image` block, so
/// the "newest image in the window" rule below finds it again on its own. The
/// marker is here for the model, and for the `<system-reminder>` fence that
/// keeps `preserved_user_messages` from re-attaching the text half as if the
/// user had typed it.
const CARRY_MARKER: &str = "[Screen view preserved across context compaction]";

/// The newest image in `window`, re-emitted below the summary — or `None` when
/// the window holds no image at all, which is every non-visual run.
pub(crate) fn image_carry_message(window: &[UnifiedMessage]) -> Option<UnifiedMessage> {
    let image = newest_image(window)?;
    Some(UnifiedMessage::User {
        content: vec![
            ContentBlock::Text {
                text: format!(
                    "<system-reminder>\nReference data, not user input.\n{CARRY_MARKER}\n\
                     This is the most recent screen capture from the summarized \
                     turns; earlier ones were already dropped. Re-capture before \
                     acting if the screen may have changed since.\n</system-reminder>"
                ),
                cache_control: None,
            },
            image,
        ],
    })
}

/// Last `Image` block of the last image-bearing message — the same "newest"
/// the stripping stage protects, resolved over the drained window instead of
/// the whole conversation.
fn newest_image(window: &[UnifiedMessage]) -> Option<ContentBlock> {
    window.iter().rev().find_map(|m| {
        m.content_blocks()
            .iter()
            .rev()
            .find(|b| matches!(b, ContentBlock::Image { .. }))
            .cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(tag: &str) -> ContentBlock {
        ContentBlock::Image {
            data: tag.to_string(),
            mime_type: "image/png".to_string(),
        }
    }

    fn user_with_image(text: &str, tag: &str) -> UnifiedMessage {
        UnifiedMessage::User {
            content: vec![
                ContentBlock::Text {
                    text: text.to_string(),
                    cache_control: None,
                },
                image(tag),
            ],
        }
    }

    #[test]
    fn a_window_without_images_carries_nothing() {
        let window = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("hi"),
        ];
        assert!(
            image_carry_message(&window).is_none(),
            "a text-only run must pay nothing for this carrier"
        );
    }

    #[test]
    fn the_newest_image_is_the_one_carried() {
        let window = vec![
            user_with_image("first shot", "OLDEST"),
            UnifiedMessage::assistant("clicked"),
            user_with_image("second shot", "NEWEST"),
            UnifiedMessage::assistant("clicked again"),
        ];

        let carried = image_carry_message(&window).expect("an image-bearing window carries one");
        let images: Vec<&ContentBlock> = carried
            .content_blocks()
            .iter()
            .filter(|b| matches!(b, ContentBlock::Image { .. }))
            .collect();

        assert_eq!(images.len(), 1, "exactly one image, never the whole window");
        match images[0] {
            ContentBlock::Image { data, .. } => assert_eq!(data, "NEWEST"),
            other => panic!("expected an image block, got {other:?}"),
        }
    }

    /// The text half must not come back a second time as if the user had
    /// written it. `preserved_user_messages` skips `User` turns whose text is
    /// synthetic scaffolding, and that predicate keys on the `<system-reminder>`
    /// fence — so the fence is load-bearing, not decoration.
    #[test]
    fn the_carried_message_reads_as_scaffolding_not_as_user_intent() {
        let window = vec![user_with_image("shot", "ONLY")];
        let carried = image_carry_message(&window).expect("carries");
        assert!(
            crate::context::compact::preserve::is_synthetic_scaffold(&carried.text_content()),
            "an unfenced carry would be re-attached as user intent on the next pass"
        );
        assert!(carried.text_content().contains(CARRY_MARKER));
    }

    /// A second compaction whose window contains the previous carry re-carries
    /// the same image, with no recognition pass: the carrier holds a real
    /// `Image` block, so the newest-image rule finds it again.
    #[test]
    fn a_second_pass_re_carries_the_image_a_first_pass_emitted() {
        let first = image_carry_message(&[user_with_image("shot", "PIXELS")]).expect("carries");
        let second_window = vec![
            UnifiedMessage::user("[Context Summary]\nearlier work"),
            first,
            UnifiedMessage::assistant("more work"),
        ];

        let again = image_carry_message(&second_window).expect("re-carries");
        match again
            .content_blocks()
            .iter()
            .find(|b| matches!(b, ContentBlock::Image { .. }))
            .expect("still an image")
        {
            ContentBlock::Image { data, .. } => assert_eq!(data, "PIXELS"),
            other => panic!("expected an image block, got {other:?}"),
        }
    }
}
