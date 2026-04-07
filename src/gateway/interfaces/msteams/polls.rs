//! MS Teams Polls System
//!
//! SQLite-backed poll store with Adaptive Card UI for Microsoft Teams.
//! Polls are stored in SQLite with TTL and size limits.
//!
//! Adaptive Card structure:
//! - Input.ChoiceSet for vote selection
//! - Action.Submit with poll ID encoded in data
//! - Fallback text for clients that don't render cards

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::gateway::channel::ChannelError;

/// Maximum number of polls to store
const MAX_POLLS: usize = 1000;
/// Poll TTL in seconds (30 days)
const POLL_TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// A poll with vote tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Poll {
    pub poll_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub max_selections: u32,
    pub created_at: i64,
    pub updated_at: Option<i64>,
    pub conversation_id: Option<String>,
    pub message_id: Option<String>,
    /// voter_id -> selected option indices
    pub votes: std::collections::HashMap<String, Vec<u32>>,
}

/// Parsed vote from a poll submission.
#[derive(Debug, Clone)]
pub struct PollVote {
    pub poll_id: String,
    pub voter_id: String,
    pub selections: Vec<u32>,
}

/// A rendered Adaptive Card for a poll.
#[derive(Debug, Clone)]
pub struct PollCard {
    pub poll_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub max_selections: u32,
    pub card_json: serde_json::Value,
    pub fallback_text: String,
}

/// SQLite-backed poll store.
pub struct PollDatabase {
    conn: Connection,
}

impl PollDatabase {
    /// Open a poll database at the given path.
    pub fn open(path: &str) -> Result<Self, ChannelError> {
        let conn = Connection::open(path)
            .map_err(|e| ChannelError::Internal(format!("Failed to open poll database: {}", e)))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS polls (
                poll_id TEXT PRIMARY KEY,
                question TEXT NOT NULL,
                options TEXT NOT NULL,
                max_selections INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                updated_at INTEGER,
                conversation_id TEXT,
                message_id TEXT,
                votes TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_polls_created_at ON polls(created_at);
            CREATE INDEX IF NOT EXISTS idx_polls_conversation ON polls(conversation_id);",
        )
        .map_err(|e| ChannelError::Internal(format!("Failed to create poll schema: {}", e)))?;

        Ok(Self { conn })
    }

    /// Create a new poll and store it.
    pub fn create_poll(&mut self, poll: &Poll) -> Result<(), ChannelError> {
        let options_json = serde_json::to_string(&poll.options)
            .map_err(|e| ChannelError::Internal(e.to_string()))?;
        let votes_json = serde_json::to_string(&poll.votes)
            .map_err(|e| ChannelError::Internal(e.to_string()))?;

        self.conn
            .execute(
                "INSERT OR REPLACE INTO polls (poll_id, question, options, max_selections, created_at, updated_at, conversation_id, message_id, votes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    poll.poll_id,
                    poll.question,
                    options_json,
                    poll.max_selections,
                    poll.created_at,
                    poll.updated_at,
                    poll.conversation_id,
                    poll.message_id,
                    votes_json,
                ],
            )
            .map_err(|e| ChannelError::Internal(format!("Failed to create poll: {}", e)))?;

        self.prune_old_polls()?;

        Ok(())
    }

    /// Get a poll by ID.
    pub fn get_poll(&self, poll_id: &str) -> Result<Option<Poll>, ChannelError> {
        let result = self
            .conn
            .query_row(
                "SELECT poll_id, question, options, max_selections, created_at, updated_at, conversation_id, message_id, votes
                 FROM polls WHERE poll_id = ?1",
                params![poll_id],
                |row| {
                    let options_json: String = row.get(2)?;
                    let votes_json: String = row.get(8)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        options_json,
                        row.get::<_, u32>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        votes_json,
                    ))
                },
            )
            .optional()
            .map_err(|e| ChannelError::Internal(format!("Failed to get poll: {}", e)))?;

        match result {
            Some((
                poll_id,
                question,
                options_json,
                max_selections,
                created_at,
                updated_at,
                conversation_id,
                message_id,
                votes_json,
            )) => {
                let options: Vec<String> = serde_json::from_str(&options_json).unwrap_or_default();
                let votes: std::collections::HashMap<String, Vec<u32>> =
                    serde_json::from_str(&votes_json).unwrap_or_default();

                Ok(Some(Poll {
                    poll_id,
                    question,
                    options,
                    max_selections,
                    created_at,
                    updated_at,
                    conversation_id,
                    message_id,
                    votes,
                }))
            }
            None => Ok(None),
        }
    }

    /// Record a vote on a poll.
    pub fn record_vote(&mut self, vote: &PollVote) -> Result<Option<Poll>, ChannelError> {
        // First get the poll to validate and normalize selections
        let poll = match self.get_poll(&vote.poll_id)? {
            Some(p) => p,
            None => return Ok(None),
        };

        // Normalize selections: keep only valid indices, respect max_selections
        let normalized: Vec<u32> = vote
            .selections
            .iter()
            .filter(|&&s| s < poll.options.len() as u32)
            .copied()
            .take(poll.max_selections as usize)
            .collect();

        let mut updated_poll = poll;
        updated_poll.votes.insert(vote.voter_id.clone(), normalized);
        updated_poll.updated_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        );

        self.create_poll(&updated_poll)?;

        Ok(Some(updated_poll))
    }

    /// Delete expired polls and enforce MAX_POLLS limit.
    fn prune_old_polls(&mut self) -> Result<(), ChannelError> {
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - POLL_TTL_SECS as i64;

        // Delete expired polls
        self.conn
            .execute("DELETE FROM polls WHERE created_at < ?1", params![cutoff])
            .map_err(|e| ChannelError::Internal(format!("Failed to prune expired polls: {}", e)))?;

        // Enforce MAX_POLLS limit
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM polls", [], |row| row.get(0))
            .map_err(|e| ChannelError::Internal(format!("Failed to count polls: {}", e)))?;

        if count > MAX_POLLS as i64 {
            let to_delete = count - MAX_POLLS as i64;
            self.conn
                .execute(
                    "DELETE FROM polls WHERE poll_id IN (
                        SELECT poll_id FROM polls ORDER BY created_at ASC LIMIT ?1
                    )",
                    params![to_delete],
                )
                .map_err(|e| {
                    ChannelError::Internal(format!("Failed to prune poll limit: {}", e))
                })?;
        }

        Ok(())
    }
}

/// Build an Adaptive Card for a poll.
pub fn build_poll_card(question: &str, options: &[String], max_selections: u32) -> PollCard {
    let poll_id = Uuid::new_v4().to_string();
    let max_sel = max_selections.max(1).min(options.len() as u32);
    let hint = if max_sel > 1 {
        format!(
            "Select up to {} option{}.",
            max_sel,
            if max_sel == 1 { "" } else { "s" }
        )
    } else {
        "Select one option.".to_string()
    };

    let choices: Vec<serde_json::Value> = options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            serde_json::json!({
                "title": opt,
                "value": String::from_utf8_lossy(&[b'0' + i as u8]).to_string()
            })
        })
        .collect();

    let card = serde_json::json!({
        "type": "AdaptiveCard",
        "version": "1.5",
        "body": [
            {
                "type": "TextBlock",
                "text": question,
                "wrap": true,
                "weight": "Bolder",
                "size": "Medium"
            },
            {
                "type": "Input.ChoiceSet",
                "id": "choices",
                "isMultiSelect": max_sel > 1,
                "style": "expanded",
                "choices": choices
            },
            {
                "type": "TextBlock",
                "text": hint,
                "wrap": true,
                "isSubtle": true,
                "spacing": "Small"
            }
        ],
        "actions": [
            {
                "type": "Action.Submit",
                "title": "Vote",
                "data": {
                    "openclawPollId": poll_id,
                    "pollId": poll_id
                },
                "msteams": {
                    "type": "messageBack",
                    "text": "openclaw poll vote",
                    "displayText": "Vote recorded",
                    "value": {
                        "openclawPollId": poll_id,
                        "pollId": poll_id
                    }
                }
            }
        ]
    });

    let fallback_text = format!(
        "Poll: {}\n{}",
        question,
        options
            .iter()
            .enumerate()
            .map(|(i, opt)| format!("{}. {}", i + 1, opt))
            .collect::<Vec<_>>()
            .join("\n")
    );

    PollCard {
        poll_id: poll_id.clone(),
        question: question.to_string(),
        options: options.to_vec(),
        max_selections: max_sel,
        card_json: card,
        fallback_text,
    }
}

/// Extract poll vote from an activity value payload.
///
/// Looks for poll ID in various nested locations to handle different
/// Teams client formats.
pub fn extract_poll_vote(value: &serde_json::Value) -> Option<PollVote> {
    // Try to find poll_id in various nested locations
    let poll_id = value
        .get("openclawPollId")
        .or_else(|| value.get("pollId"))
        .or_else(|| value.get("openclaw").and_then(|o| o.get("pollId")))
        .or_else(|| {
            value
                .get("openclaw")
                .and_then(|o| o.get("poll").and_then(|p| p.get("id")))
        })
        .or_else(|| value.get("data").and_then(|d| d.get("openclawPollId")))
        .or_else(|| value.get("data").and_then(|d| d.get("pollId")))
        .or_else(|| {
            value
                .get("data")
                .and_then(|d| d.get("openclaw"))
                .and_then(|o| o.get("pollId"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;

    // Extract selections from "choices" field
    let raw_selections: Vec<String> = value
        .get("choices")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .or_else(|| {
            value
                .get("data")
                .and_then(|d| d.get("choices"))
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
        })
        .unwrap_or_default();

    // Parse selection indices from string values (e.g., "0", "1", "2")
    let selections: Vec<u32> = raw_selections
        .iter()
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();

    if selections.is_empty() {
        return None;
    }

    // For voter_id, we can't reliably get it from the payload alone
    // The caller should provide it based on the activity's from field
    Some(PollVote {
        poll_id,
        voter_id: String::new(), // Caller fills this
        selections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_poll() -> Poll {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Poll {
            poll_id: "test-poll-1".to_string(),
            question: "What is your favorite color?".to_string(),
            options: vec!["Red".to_string(), "Blue".to_string(), "Green".to_string()],
            max_selections: 2,
            created_at: now,
            updated_at: None,
            conversation_id: Some("conv-1".to_string()),
            message_id: Some("msg-1".to_string()),
            votes: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn test_build_poll_card() {
        let card = build_poll_card(
            "What's for lunch?",
            &[
                "Pizza".to_string(),
                "Sushi".to_string(),
                "Tacos".to_string(),
            ],
            2,
        );

        assert!(!card.poll_id.is_empty());
        assert_eq!(card.question, "What's for lunch?");
        assert_eq!(card.options.len(), 3);
        assert_eq!(card.max_selections, 2);
        assert_eq!(card.card_json["type"], "AdaptiveCard");
        assert_eq!(card.card_json["body"][0]["text"], "What's for lunch?");
        assert!(card.fallback_text.contains("What's for lunch?"));
    }

    #[test]
    fn test_build_poll_card_single_selection() {
        let card = build_poll_card("Yes or No?", &["Yes".to_string(), "No".to_string()], 1);

        assert_eq!(card.max_selections, 1);
        assert_eq!(card.card_json["body"][1]["isMultiSelect"], false);
    }

    #[test]
    fn test_extract_poll_vote_direct() {
        let value = serde_json::json!({
            "openclawPollId": "poll-123",
            "choices": ["0", "2"]
        });

        let vote = extract_poll_vote(&value);
        assert!(vote.is_some());
        let vote = vote.unwrap();
        assert_eq!(vote.poll_id, "poll-123");
        assert_eq!(vote.selections, vec![0, 2]);
    }

    #[test]
    fn test_extract_poll_vote_nested() {
        let value = serde_json::json!({
            "data": {
                "openclawPollId": "poll-456",
                "choices": ["1"]
            }
        });

        let vote = extract_poll_vote(&value);
        assert!(vote.is_some());
        let vote = vote.unwrap();
        assert_eq!(vote.poll_id, "poll-456");
        assert_eq!(vote.selections, vec![1]);
    }

    #[test]
    fn test_extract_poll_vote_missing() {
        let value = serde_json::json!({
            "someOtherField": "value"
        });

        let vote = extract_poll_vote(&value);
        assert!(vote.is_none());
    }

    #[test]
    fn test_poll_database_crud() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test-polls-{}.db", Uuid::new_v4()));

        {
            let mut db = PollDatabase::open(db_path.to_str().unwrap()).unwrap();

            let poll = create_test_poll();
            db.create_poll(&poll).unwrap();

            let retrieved = db.get_poll("test-poll-1").unwrap().unwrap();
            assert_eq!(retrieved.question, "What is your favorite color?");
            assert_eq!(retrieved.options.len(), 3);

            let mut vote = PollVote {
                poll_id: "test-poll-1".to_string(),
                voter_id: "user-1".to_string(),
                selections: vec![0, 2],
            };
            let updated = db.record_vote(&mut vote).unwrap().unwrap();
            assert_eq!(updated.votes.get("user-1"), Some(&vec![0, 2]));
        }

        std::fs::remove_file(db_path).ok();
    }
}
