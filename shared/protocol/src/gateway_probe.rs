//! Canonical HTTP liveness/readiness documents. They do not authenticate a process.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub status: HealthStatus,
    pub version: String,
    pub instance_id: String,
    pub started_at_unix: i64,
    pub uptime_secs: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessPhase {
    Booting,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Readiness {
    pub ready: bool,
    pub phase: ReadinessPhase,
    pub version: String,
}

impl Readiness {
    pub fn new(ready: bool, version: impl Into<String>) -> Self {
        Self {
            ready,
            phase: if ready {
                ReadinessPhase::Complete
            } else {
                ReadinessPhase::Booting
            },
            version: version.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Health, Readiness};
    use serde_json::json;

    #[test]
    fn complete_wire_derives_phase_from_ready() {
        assert_eq!(
            serde_json::to_value(Readiness::new(true, "2026.11.22")).unwrap(),
            json!({"ready": true, "phase": "complete", "version": "2026.11.22"})
        );
    }

    #[test]
    fn booting_wire_keeps_existing_field_values() {
        assert_eq!(
            serde_json::to_value(Readiness::new(false, "2026.11.22")).unwrap(),
            json!({"ready": false, "phase": "booting", "version": "2026.11.22"})
        );
    }

    #[test]
    fn unknown_probe_states_and_fields_are_not_compatibility_evidence() {
        assert!(serde_json::from_value::<Readiness>(json!({
            "ready": true, "phase": "maybe", "version": "2026.11.22"
        }))
        .is_err());
        assert!(serde_json::from_value::<Health>(json!({
            "status": "other", "version": "2026.11.22", "instance_id": "instance",
            "started_at_unix": 1, "uptime_secs": 1
        }))
        .is_err());
        assert!(serde_json::from_value::<Readiness>(json!({
            "ready": true, "phase": "complete", "version": "2026.11.22", "launch": true
        }))
        .is_err());
    }
}
