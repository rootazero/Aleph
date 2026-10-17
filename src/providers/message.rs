//! Compatibility paths for the API-owned message vocabulary and pure helpers.

pub use aleph_slot_api::ai_message::{
    normalize_tool_pairs, transform_messages, value_as_model_text, CacheControl, ContentBlock,
    EphemeralTtl, UnifiedMessage,
};

#[doc(hidden)]
pub use aleph_slot_api::ai_message::{dispatch_content_block, dispatch_unified_message};
