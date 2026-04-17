//! User profile layer: USER.md dialectic synthesis.

pub mod store;
pub mod types;

pub use store::{parse_user_md, render_user_md, ProfileStore, PROFILE_FILENAME};
pub use types::{ProfileDiff, ProfileSection, SessionSignal, UpdateOutcome, UserProfile};
