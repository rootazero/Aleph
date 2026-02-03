//! Memory Audit System
//!
//! Provides audit logging for memory operations, enabling explainability
//! of why facts were created, accessed, updated, or invalidated.

use serde::{Deserialize, Serialize};

/// Actor performing the memory operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditActor {
    /// AI agent performing automatic operations
    Agent,
    /// User performing manual operations
    User,
    /// System processes (compression, decay, etc.)
    System,
    /// Decay mechanism invalidating old facts
    Decay,
}

impl std::fmt::Display for AuditActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditActor::Agent => write!(f, "agent"),
            AuditActor::User => write!(f, "user"),
            AuditActor::System => write!(f, "system"),
            AuditActor::Decay => write!(f, "decay"),
        }
    }
}

impl std::str::FromStr for AuditActor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent" => Ok(AuditActor::Agent),
            "user" => Ok(AuditActor::User),
            "system" => Ok(AuditActor::System),
            "decay" => Ok(AuditActor::Decay),
            _ => Err(format!("Unknown actor: {}", s)),
        }
    }
}

/// Type of audit action
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditAction {
    /// Fact was created
    Created,
    /// Fact was accessed/retrieved
    Accessed,
    /// Fact content was updated
    Updated,
    /// Fact was invalidated (soft deleted)
    Invalidated,
    /// Fact was restored from recycle bin
    Restored,
    /// Fact was permanently deleted
    Deleted,
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditAction::Created => write!(f, "created"),
            AuditAction::Accessed => write!(f, "accessed"),
            AuditAction::Updated => write!(f, "updated"),
            AuditAction::Invalidated => write!(f, "invalidated"),
            AuditAction::Restored => write!(f, "restored"),
            AuditAction::Deleted => write!(f, "deleted"),
        }
    }
}

impl std::str::FromStr for AuditAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "created" => Ok(AuditAction::Created),
            "accessed" => Ok(AuditAction::Accessed),
            "updated" => Ok(AuditAction::Updated),
            "invalidated" => Ok(AuditAction::Invalidated),
            "restored" => Ok(AuditAction::Restored),
            "deleted" => Ok(AuditAction::Deleted),
            _ => Err(format!("Unknown action: {}", s)),
        }
    }
}

/// Details for specific audit actions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuditDetails {
    /// Details for fact creation
    Created {
        source: String,
        extraction_context: Option<String>,
    },
    /// Details for fact access
    Accessed {
        query: Option<String>,
        relevance_score: Option<f32>,
        used_in_response: bool,
    },
    /// Details for fact update
    Updated {
        old_content: String,
        new_content: String,
        reason: String,
    },
    /// Details for fact invalidation
    Invalidated {
        reason: String,
        strength_at_invalidation: Option<f32>,
    },
    /// Details for fact restoration
    Restored { new_strength: f32 },
    /// Details for permanent deletion
    Deleted {
        reason: String,
        days_in_recycle_bin: Option<u32>,
    },
}

/// A single audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry ID
    pub id: String,
    /// ID of the fact this entry relates to
    pub fact_id: String,
    /// The action performed
    pub action: AuditAction,
    /// Human-readable reason
    pub reason: Option<String>,
    /// Who performed the action
    pub actor: AuditActor,
    /// Detailed information (JSON)
    pub details: Option<AuditDetails>,
    /// When the action occurred (Unix timestamp)
    pub created_at: i64,
}

impl AuditEntry {
    /// Create a new audit entry
    pub fn new(
        fact_id: String,
        action: AuditAction,
        actor: AuditActor,
        reason: Option<String>,
        details: Option<AuditDetails>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            fact_id,
            action,
            reason,
            actor,
            details,
            created_at: now,
        }
    }

    /// Serialize details to JSON string
    pub fn details_json(&self) -> Option<String> {
        self.details
            .as_ref()
            .map(|d| serde_json::to_string(d).unwrap_or_default())
    }

    /// Parse details from JSON string
    pub fn parse_details(json: &str) -> Option<AuditDetails> {
        serde_json::from_str(json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_actor_display() {
        assert_eq!(AuditActor::Agent.to_string(), "agent");
        assert_eq!(AuditActor::User.to_string(), "user");
        assert_eq!(AuditActor::System.to_string(), "system");
        assert_eq!(AuditActor::Decay.to_string(), "decay");
    }

    #[test]
    fn test_audit_actor_parse() {
        assert_eq!("agent".parse::<AuditActor>().unwrap(), AuditActor::Agent);
        assert_eq!("USER".parse::<AuditActor>().unwrap(), AuditActor::User);
    }

    #[test]
    fn test_audit_action_display() {
        assert_eq!(AuditAction::Created.to_string(), "created");
        assert_eq!(AuditAction::Invalidated.to_string(), "invalidated");
    }

    #[test]
    fn test_audit_entry_creation() {
        let entry = AuditEntry::new(
            "fact-123".to_string(),
            AuditAction::Created,
            AuditActor::Agent,
            Some("Extracted from conversation".to_string()),
            Some(AuditDetails::Created {
                source: "session".to_string(),
                extraction_context: Some("User mentioned preference".to_string()),
            }),
        );

        assert_eq!(entry.fact_id, "fact-123");
        assert_eq!(entry.action, AuditAction::Created);
        assert!(entry.created_at > 0);
    }

    #[test]
    fn test_details_serialization() {
        let details = AuditDetails::Invalidated {
            reason: "decay".to_string(),
            strength_at_invalidation: Some(0.08),
        };

        let json = serde_json::to_string(&details).unwrap();
        let parsed: AuditDetails = serde_json::from_str(&json).unwrap();

        if let AuditDetails::Invalidated {
            reason,
            strength_at_invalidation,
        } = parsed
        {
            assert_eq!(reason, "decay");
            assert_eq!(strength_at_invalidation, Some(0.08));
        } else {
            panic!("Wrong variant");
        }
    }
}
