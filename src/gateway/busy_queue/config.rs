//! Runtime knobs for the busy wait lane.
//!
//! Mirrors the `[team_broadcast]` → `BroadcastConfig` shape used by §4.5: the
//! `Default` impl is the single source for the values, and the TOML layer
//! (`[execution]`) simply carries operator overrides in. Every field used to be
//! a bare `const` inside the inbound executor, so changing one meant a rebuild.

/// Caps and timings for [`super::deliver_when_free`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusyQueueConfig {
    /// Messages one session's lane may hold before `REJECT_NEWEST` kicks in.
    pub max_per_session: usize,
    /// How long a queued message may wait before the sender is told delivery
    /// failed.
    pub max_wait_secs: u64,
    /// Safety net between wake signals. Delivery latency is governed by the
    /// engine's slot-free signal, not by this — it only bounds the damage of a
    /// missed signal.
    pub wake_fallback_secs: u64,
}

impl Default for BusyQueueConfig {
    fn default() -> Self {
        Self {
            max_per_session: 32,
            max_wait_secs: 1800,
            wake_fallback_secs: 30,
        }
    }
}

impl BusyQueueConfig {
    /// Build from the `[execution]` config block. Zero values fall back to the
    /// default (P7: a zero cap would mean "reject everything" — an operator
    /// typo must not make the lane refuse every message).
    #[must_use]
    pub fn from_execution(cfg: &crate::config::types::execution::ExecutionConfig) -> Self {
        let d = Self::default();
        Self {
            max_per_session: if cfg.busy_queue_max_per_session == 0 {
                d.max_per_session
            } else {
                cfg.busy_queue_max_per_session
            },
            max_wait_secs: if cfg.busy_queue_max_wait_secs == 0 {
                d.max_wait_secs
            } else {
                cfg.busy_queue_max_wait_secs
            },
            wake_fallback_secs: if cfg.busy_queue_wake_fallback_secs == 0 {
                d.wake_fallback_secs
            } else {
                cfg.busy_queue_wake_fallback_secs
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::execution::ExecutionConfig;

    #[test]
    fn default_matches_the_execution_config_defaults() {
        // Single source: the two default sets must not drift.
        let from_cfg = BusyQueueConfig::from_execution(&ExecutionConfig::default());
        assert_eq!(from_cfg, BusyQueueConfig::default());
    }

    #[test]
    fn operator_overrides_are_carried_through() {
        let cfg = ExecutionConfig {
            busy_queue_max_per_session: 4,
            busy_queue_max_wait_secs: 60,
            busy_queue_wake_fallback_secs: 5,
            ..Default::default()
        };
        let bq = BusyQueueConfig::from_execution(&cfg);
        assert_eq!(bq.max_per_session, 4);
        assert_eq!(bq.max_wait_secs, 60);
        assert_eq!(bq.wake_fallback_secs, 5);
    }

    #[test]
    fn zero_values_fall_back_to_defaults() {
        // A zero cap would make the lane reject every message ("born dead"),
        // so a typo degrades to the default instead (P7, same posture as the
        // §4.5 `[team_broadcast]` mapping).
        let cfg = ExecutionConfig {
            busy_queue_max_per_session: 0,
            busy_queue_max_wait_secs: 0,
            busy_queue_wake_fallback_secs: 0,
            ..Default::default()
        };
        assert_eq!(
            BusyQueueConfig::from_execution(&cfg),
            BusyQueueConfig::default()
        );
    }
}
