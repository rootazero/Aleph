//! Auth profile failure classification and cooldown backoff.
//!
//! Slim helpers consumed by `profile_manager`:
//! - [`AuthProfileFailureReason`] — classify provider failures by HTTP outcome
//! - [`calculate_cooldown_ms`] — base-5 exponential backoff for rate limits

mod cooldown;
mod failure;

pub use cooldown::calculate_cooldown_ms;
pub use failure::AuthProfileFailureReason;
