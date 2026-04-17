//! User profile layer: USER.md dialectic synthesis.

pub mod prompts;
pub mod store;
pub mod types;

pub use prompts::{build_merge_user_prompt, PROMPT_PROFILE_BOOTSTRAP, PROMPT_PROFILE_MERGE};
pub use store::{parse_user_md, render_user_md, ProfileStore, PROFILE_FILENAME};
pub use types::{ProfileDiff, ProfileSection, SessionSignal, UpdateOutcome, UserProfile};
