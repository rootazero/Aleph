//! User profile structures

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A consolidated user profile distilled from frequent facts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// Unique identifier for this profile
    pub profile_id: String,

    /// Categories of user information (e.g., "preferences", "habits", "skills")
    pub categories: HashMap<String, ProfileCategory>,

    /// When this profile was created
    pub created_at: i64,

    /// When this profile was last updated
    pub updated_at: i64,
}

/// A category within the user profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileCategory {
    /// Category name (e.g., "preferences", "habits", "technical_skills")
    pub name: String,

    /// Consolidated facts in this category
    pub facts: Vec<ConsolidatedFact>,

    /// Confidence in this category (0.0-1.0)
    pub confidence: f32,
}

/// A consolidated fact derived from multiple source facts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedFact {
    /// The consolidated fact content
    pub content: String,

    /// IDs of source facts that contributed to this consolidation
    pub source_fact_ids: Vec<String>,

    /// Number of times source facts were accessed
    pub access_count: u32,

    /// When this fact was last accessed
    pub last_accessed: i64,

    /// Confidence score (0.0-1.0)
    pub confidence: f32,
}

impl UserProfile {
    /// Create a new empty user profile
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            profile_id: uuid::Uuid::new_v4().to_string(),
            categories: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a category to the profile
    pub fn add_category(&mut self, category: ProfileCategory) {
        self.categories.insert(category.name.clone(), category);
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Get a category by name
    pub fn get_category(&self, name: &str) -> Option<&ProfileCategory> {
        self.categories.get(name)
    }

    /// Get all facts across all categories
    pub fn get_all_facts(&self) -> Vec<&ConsolidatedFact> {
        self.categories
            .values()
            .flat_map(|cat| cat.facts.iter())
            .collect()
    }

    /// Get total number of consolidated facts
    pub fn fact_count(&self) -> usize {
        self.categories.values().map(|cat| cat.facts.len()).sum()
    }
}

impl Default for UserProfile {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileCategory {
    /// Create a new profile category
    pub fn new(name: String) -> Self {
        Self {
            name,
            facts: Vec::new(),
            confidence: 0.0,
        }
    }

    /// Add a consolidated fact to this category
    pub fn add_fact(&mut self, fact: ConsolidatedFact) {
        self.facts.push(fact);
        self.update_confidence();
    }

    /// Update category confidence based on facts
    fn update_confidence(&mut self) {
        if self.facts.is_empty() {
            self.confidence = 0.0;
            return;
        }

        // Average confidence of all facts
        let sum: f32 = self.facts.iter().map(|f| f.confidence).sum();
        self.confidence = sum / self.facts.len() as f32;
    }
}

impl ConsolidatedFact {
    /// Create a new consolidated fact
    pub fn new(
        content: String,
        source_fact_ids: Vec<String>,
        access_count: u32,
        last_accessed: i64,
    ) -> Self {
        // Calculate confidence based on access count
        let confidence = (access_count as f32 / 100.0).min(1.0);

        Self {
            content,
            source_fact_ids,
            access_count,
            last_accessed,
            confidence,
        }
    }

    /// Update access statistics
    pub fn update_access(&mut self, timestamp: i64) {
        self.access_count += 1;
        self.last_accessed = timestamp;
        self.confidence = (self.access_count as f32 / 100.0).min(1.0);
    }
}
