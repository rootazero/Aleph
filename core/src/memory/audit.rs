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

use crate::error::AlephError;
use crate::memory::store::{AuditStore, MemoryBackend, MemoryStore};

/// Logger for memory audit events
pub struct AuditLogger {
    db: MemoryBackend,
}

impl AuditLogger {
    /// Create a new audit logger
    pub fn new(db: MemoryBackend) -> Self {
        Self { db }
    }

    /// Log an audit event
    pub async fn log(&self, entry: AuditEntry) -> Result<(), AlephError> {
        self.db.insert_audit_entry(&entry).await
    }

    /// Log a fact creation event
    pub async fn log_created(
        &self,
        fact_id: &str,
        source: &str,
        extraction_context: Option<&str>,
    ) -> Result<(), AlephError> {
        let entry = AuditEntry::new(
            fact_id.to_string(),
            AuditAction::Created,
            AuditActor::Agent,
            Some("Fact extracted from conversation".to_string()),
            Some(AuditDetails::Created {
                source: source.to_string(),
                extraction_context: extraction_context.map(|s| s.to_string()),
            }),
        );
        self.log(entry).await
    }

    /// Log a fact access event
    pub async fn log_accessed(
        &self,
        fact_id: &str,
        query: Option<&str>,
        relevance_score: Option<f32>,
        used_in_response: bool,
    ) -> Result<(), AlephError> {
        let entry = AuditEntry::new(
            fact_id.to_string(),
            AuditAction::Accessed,
            AuditActor::Agent,
            None,
            Some(AuditDetails::Accessed {
                query: query.map(|s| s.to_string()),
                relevance_score,
                used_in_response,
            }),
        );
        self.log(entry).await
    }

    /// Log a fact invalidation event
    pub async fn log_invalidated(
        &self,
        fact_id: &str,
        reason: &str,
        actor: AuditActor,
        strength_at_invalidation: Option<f32>,
    ) -> Result<(), AlephError> {
        let entry = AuditEntry::new(
            fact_id.to_string(),
            AuditAction::Invalidated,
            actor,
            Some(format!("Fact invalidated: {}", reason)),
            Some(AuditDetails::Invalidated {
                reason: reason.to_string(),
                strength_at_invalidation,
            }),
        );
        self.log(entry).await
    }

    /// Log a fact restoration event
    pub async fn log_restored(&self, fact_id: &str, new_strength: f32) -> Result<(), AlephError> {
        let entry = AuditEntry::new(
            fact_id.to_string(),
            AuditAction::Restored,
            AuditActor::User,
            Some("Fact restored from recycle bin".to_string()),
            Some(AuditDetails::Restored { new_strength }),
        );
        self.log(entry).await
    }

    /// Log a permanent deletion event
    pub async fn log_deleted(
        &self,
        fact_id: &str,
        reason: &str,
        days_in_recycle_bin: Option<u32>,
    ) -> Result<(), AlephError> {
        let entry = AuditEntry::new(
            fact_id.to_string(),
            AuditAction::Deleted,
            AuditActor::System,
            Some(format!("Fact permanently deleted: {}", reason)),
            Some(AuditDetails::Deleted {
                reason: reason.to_string(),
                days_in_recycle_bin,
            }),
        );
        self.log(entry).await
    }

    /// Get audit history for a specific fact
    pub async fn get_fact_history(&self, fact_id: &str) -> Result<Vec<AuditEntry>, AlephError> {
        self.db.get_audit_entries_for_fact(fact_id).await
    }

    /// Get recent audit events
    pub async fn get_recent_events(&self, limit: usize) -> Result<Vec<AuditEntry>, AlephError> {
        self.db.get_recent_audit_entries(limit).await
    }

    /// Explain a fact's complete lifecycle
    ///
    /// Returns a human-readable explanation of how a fact was created,
    /// accessed, and potentially invalidated.
    pub async fn explain_fact(&self, fact_id: &str) -> Result<FactExplanation, AlephError> {
        let history = self.get_fact_history(fact_id).await?;
        let fact = self.db.get_fact(fact_id).await?;

        if history.is_empty() && fact.is_none() {
            return Err(AlephError::other(format!("Fact not found: {}", fact_id)));
        }

        // Build explanation from history
        let mut events = Vec::new();
        let mut creation_source = None;
        let mut invalidation_reason = None;
        let mut access_count = 0;

        for entry in &history {
            match &entry.action {
                AuditAction::Created => {
                    if let Some(AuditDetails::Created { source, extraction_context }) = &entry.details {
                        creation_source = Some(source.clone());
                        events.push(ExplainedEvent {
                            timestamp: entry.created_at,
                            action: "Created".to_string(),
                            description: format!(
                                "Extracted from {} {}",
                                source,
                                extraction_context.as_ref().map(|c| format!("({})", c)).unwrap_or_default()
                            ),
                            actor: entry.actor.to_string(),
                        });
                    }
                }
                AuditAction::Accessed => {
                    access_count += 1;
                    if let Some(AuditDetails::Accessed { query, relevance_score, used_in_response }) = &entry.details {
                        events.push(ExplainedEvent {
                            timestamp: entry.created_at,
                            action: "Accessed".to_string(),
                            description: format!(
                                "Retrieved{} (score: {:.2}, used: {})",
                                query.as_ref().map(|q| format!(" for query '{}'", q)).unwrap_or_default(),
                                relevance_score.unwrap_or(0.0),
                                used_in_response
                            ),
                            actor: entry.actor.to_string(),
                        });
                    }
                }
                AuditAction::Invalidated => {
                    if let Some(AuditDetails::Invalidated { reason, strength_at_invalidation }) = &entry.details {
                        invalidation_reason = Some(reason.clone());
                        events.push(ExplainedEvent {
                            timestamp: entry.created_at,
                            action: "Invalidated".to_string(),
                            description: format!(
                                "{} (strength: {:.2})",
                                reason,
                                strength_at_invalidation.unwrap_or(0.0)
                            ),
                            actor: entry.actor.to_string(),
                        });
                    }
                }
                AuditAction::Restored => {
                    if let Some(AuditDetails::Restored { new_strength }) = &entry.details {
                        events.push(ExplainedEvent {
                            timestamp: entry.created_at,
                            action: "Restored".to_string(),
                            description: format!("Restored with strength {:.2}", new_strength),
                            actor: entry.actor.to_string(),
                        });
                    }
                }
                AuditAction::Updated => {
                    if let Some(AuditDetails::Updated { reason, .. }) = &entry.details {
                        events.push(ExplainedEvent {
                            timestamp: entry.created_at,
                            action: "Updated".to_string(),
                            description: reason.clone(),
                            actor: entry.actor.to_string(),
                        });
                    }
                }
                AuditAction::Deleted => {
                    if let Some(AuditDetails::Deleted { reason, days_in_recycle_bin }) = &entry.details {
                        events.push(ExplainedEvent {
                            timestamp: entry.created_at,
                            action: "Deleted".to_string(),
                            description: format!(
                                "{} (in recycle bin for {} days)",
                                reason,
                                days_in_recycle_bin.unwrap_or(0)
                            ),
                            actor: entry.actor.to_string(),
                        });
                    }
                }
            }
        }

        let (is_valid, content) = if let Some(f) = fact {
            (f.is_valid, Some(f.content))
        } else {
            (false, None)
        };

        Ok(FactExplanation {
            fact_id: fact_id.to_string(),
            content,
            is_valid,
            creation_source,
            access_count,
            invalidation_reason,
            events,
        })
    }

    /// Explain why a fact was forgotten/invalidated
    ///
    /// Returns a focused explanation specifically about invalidation.
    pub async fn explain_forgetting(&self, fact_id: &str) -> Result<ForgettingExplanation, AlephError> {
        let history = self.get_fact_history(fact_id).await?;
        let fact = self.db.get_fact(fact_id).await?;

        // Find invalidation event
        let invalidation = history.iter().find(|e| e.action == AuditAction::Invalidated);

        let (reason, actor, strength_at_invalidation, timestamp) = match invalidation {
            Some(entry) => {
                if let Some(AuditDetails::Invalidated { reason, strength_at_invalidation }) = &entry.details {
                    (
                        reason.clone(),
                        entry.actor.clone(),
                        *strength_at_invalidation,
                        Some(entry.created_at),
                    )
                } else {
                    let r = fact.as_ref()
                        .and_then(|f| f.invalidation_reason.clone())
                        .unwrap_or_else(|| "Unknown reason".to_string());
                    (r, entry.actor.clone(), None, Some(entry.created_at))
                }
            }
            None => {
                if let Some(f) = &fact {
                    if !f.is_valid {
                        (
                            f.invalidation_reason.clone().unwrap_or_else(|| "Unknown reason".to_string()),
                            AuditActor::System,
                            None,
                            f.decay_invalidated_at,
                        )
                    } else {
                        return Err(AlephError::other(format!(
                            "Fact {} is still valid, not forgotten",
                            fact_id
                        )));
                    }
                } else {
                    return Err(AlephError::other(format!("Fact not found: {}", fact_id)));
                }
            }
        };

        // Calculate days since creation
        let created_at = fact.map(|f| f.created_at).unwrap_or(0);
        let days_since_creation = timestamp
            .map(|t| (t - created_at) as f32 / 86400.0)
            .unwrap_or(0.0);

        // Generate human-readable explanation
        let explanation = match actor {
            AuditActor::Decay => format!(
                "This fact was automatically forgotten by the decay mechanism after {:.1} days. {}",
                days_since_creation,
                if let Some(s) = strength_at_invalidation {
                    format!("Memory strength had dropped to {:.2}, below the threshold.", s)
                } else {
                    "Memory strength fell below the minimum threshold.".to_string()
                }
            ),
            AuditActor::User => format!(
                "This fact was manually forgotten by the user. Reason: {}",
                reason
            ),
            AuditActor::Agent => format!(
                "This fact was invalidated by an agent due to: {}",
                reason
            ),
            AuditActor::System => format!(
                "This fact was invalidated by the system. Reason: {}",
                reason
            ),
        };

        Ok(ForgettingExplanation {
            fact_id: fact_id.to_string(),
            reason,
            actor,
            strength_at_invalidation,
            timestamp,
            days_since_creation,
            explanation,
        })
    }
}

/// Explanation of a fact's lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactExplanation {
    /// The fact ID
    pub fact_id: String,
    /// The fact content (if still available)
    pub content: Option<String>,
    /// Whether the fact is currently valid
    pub is_valid: bool,
    /// Source of creation (e.g., "session", "user")
    pub creation_source: Option<String>,
    /// Number of times accessed
    pub access_count: usize,
    /// Reason for invalidation (if invalidated)
    pub invalidation_reason: Option<String>,
    /// Timeline of events
    pub events: Vec<ExplainedEvent>,
}

/// A single explained event in a fact's lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainedEvent {
    /// Unix timestamp
    pub timestamp: i64,
    /// Action type
    pub action: String,
    /// Human-readable description
    pub description: String,
    /// Who performed the action
    pub actor: String,
}

/// Explanation of why a fact was forgotten
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgettingExplanation {
    /// The fact ID
    pub fact_id: String,
    /// The reason for forgetting
    pub reason: String,
    /// Who/what caused the forgetting
    pub actor: AuditActor,
    /// Memory strength at invalidation
    pub strength_at_invalidation: Option<f32>,
    /// When it was forgotten
    pub timestamp: Option<i64>,
    /// Days since creation
    pub days_since_creation: f32,
    /// Human-readable explanation
    pub explanation: String,
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

    #[test]
    fn test_fact_explanation_structure() {
        let explanation = FactExplanation {
            fact_id: "fact-123".to_string(),
            content: Some("User prefers Rust".to_string()),
            is_valid: true,
            creation_source: Some("session".to_string()),
            access_count: 5,
            invalidation_reason: None,
            events: vec![
                ExplainedEvent {
                    timestamp: 1234567890,
                    action: "Created".to_string(),
                    description: "Extracted from session".to_string(),
                    actor: "agent".to_string(),
                },
            ],
        };

        assert_eq!(explanation.fact_id, "fact-123");
        assert_eq!(explanation.access_count, 5);
        assert!(explanation.is_valid);
        assert_eq!(explanation.events.len(), 1);
    }

    #[test]
    fn test_fact_explanation_serialization() {
        let explanation = FactExplanation {
            fact_id: "test-id".to_string(),
            content: Some("Test content".to_string()),
            is_valid: false,
            creation_source: Some("user".to_string()),
            access_count: 3,
            invalidation_reason: Some("Outdated".to_string()),
            events: vec![],
        };

        let json = serde_json::to_string(&explanation).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("Outdated"));

        let parsed: FactExplanation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fact_id, "test-id");
        assert!(!parsed.is_valid);
    }

    #[test]
    fn test_forgetting_explanation_structure() {
        let explanation = ForgettingExplanation {
            fact_id: "fact-456".to_string(),
            reason: "Memory strength below threshold".to_string(),
            actor: AuditActor::Decay,
            strength_at_invalidation: Some(0.08),
            timestamp: Some(1234567890),
            days_since_creation: 45.5,
            explanation: "This fact was automatically forgotten after 45.5 days.".to_string(),
        };

        assert_eq!(explanation.fact_id, "fact-456");
        assert_eq!(explanation.actor, AuditActor::Decay);
        assert_eq!(explanation.strength_at_invalidation, Some(0.08));
    }

    #[test]
    fn test_forgetting_explanation_serialization() {
        let explanation = ForgettingExplanation {
            fact_id: "test-fact".to_string(),
            reason: "User requested deletion".to_string(),
            actor: AuditActor::User,
            strength_at_invalidation: None,
            timestamp: Some(1234567890),
            days_since_creation: 7.0,
            explanation: "Manually forgotten by user.".to_string(),
        };

        let json = serde_json::to_string(&explanation).unwrap();
        assert!(json.contains("test-fact"));
        assert!(json.contains("user"));

        let parsed: ForgettingExplanation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.actor, AuditActor::User);
    }

    #[test]
    fn test_explained_event() {
        let event = ExplainedEvent {
            timestamp: 1234567890,
            action: "Accessed".to_string(),
            description: "Retrieved for query 'rust preferences'".to_string(),
            actor: "agent".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Accessed"));
        assert!(json.contains("rust preferences"));
    }
}
