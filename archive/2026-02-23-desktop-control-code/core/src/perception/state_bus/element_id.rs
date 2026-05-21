//! Stable element ID generation with three-level fallback strategy.
//!
//! Element IDs must remain stable across UI changes to enable reliable
//! action dispatching. This module implements a hierarchical ID generation
//! strategy that gracefully degrades when higher-quality identifiers are
//! unavailable.
//!
//! # Strategy Levels
//!
//! 1. **AXIdentifier** (most stable): Developer-set accessibility identifier
//! 2. **Semantic Hash** (stable): Role + label + relative position
//! 3. **Path Hash** (least stable): UI hierarchy path without indices
//!
//! # Example
//!
//! ```ignore
//! let id = StableElementId::generate(&ax_element);
//! // Later, when UI changes...
//! let element = id.resolve(&state_cache)?;  // Still finds the element
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Stable element identifier with fallback resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableElementId {
    /// Primary ID (highest quality available)
    primary: String,

    /// Fallback ID (for resolution when primary fails)
    fallback: String,

    /// ID generation strategy version
    version: u32,
}

impl StableElementId {
    /// Generate a stable ID for an element.
    ///
    /// This is a stub implementation. In the real implementation, this would:
    /// 1. Try to get AXIdentifier from the element
    /// 2. Fall back to semantic hash (role + label + relative position)
    /// 3. Fall back to path hash as last resort
    pub fn generate(element_info: &ElementInfo) -> Self {
        // Level 1: AXIdentifier (if available)
        if let Some(ref ax_id) = element_info.ax_identifier {
            return Self {
                primary: format!("ax_id:{}", ax_id),
                fallback: Self::path_hash(&element_info.path),
                version: 1,
            };
        }

        // Level 2: Semantic Hash (role + label + relative position)
        if let Some(ref label) = element_info.label {
            let semantic = format!(
                "{}:{}:{}",
                element_info.role,
                label,
                element_info.relative_position
            );
            return Self {
                primary: format!("sem:{}", Self::hash_string(&semantic)),
                fallback: Self::path_hash(&element_info.path),
                version: 2,
            };
        }

        // Level 3: Path Hash (always available)
        Self {
            primary: Self::path_hash(&element_info.path),
            fallback: String::new(),
            version: 3,
        }
    }

    /// Generate path-based hash (without indices).
    ///
    /// Example: "Window/VStack[0]/Button[2]" -> "Window/VStack/Button"
    fn path_hash(path: &str) -> String {
        let path_without_indices = Self::strip_indices(path);
        format!("path:{}", Self::hash_string(&path_without_indices))
    }

    /// Strip array indices from path.
    fn strip_indices(path: &str) -> String {
        path.split('/')
            .map(|segment| {
                // Remove [N] suffix
                if let Some(bracket_pos) = segment.find('[') {
                    &segment[..bracket_pos]
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Hash a string to a short hex string.
    fn hash_string(s: &str) -> String {
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Get the primary ID.
    pub fn primary(&self) -> &str {
        &self.primary
    }

    /// Get the fallback ID.
    pub fn fallback(&self) -> &str {
        &self.fallback
    }

    /// Get the version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Resolve this ID to an element, trying fallback if primary fails.
    pub fn resolve<'a>(&self, cache: &'a StateCache) -> Option<&'a Element> {
        cache.get_element(&self.primary)
            .or_else(|| {
                if !self.fallback.is_empty() {
                    cache.get_element(&self.fallback)
                } else {
                    None
                }
            })
    }
}

/// Element information for ID generation.
#[derive(Debug, Clone)]
pub struct ElementInfo {
    /// AX identifier (if set by developer)
    pub ax_identifier: Option<String>,

    /// Element role (button, textfield, etc.)
    pub role: String,

    /// Element label/title
    pub label: Option<String>,

    /// UI hierarchy path (e.g., "Window/VStack[0]/Button[2]")
    pub path: String,

    /// Relative position within parent (0-based)
    pub relative_position: usize,
}

// Re-export types from state_cache for convenience
use super::state_cache::StateCache;
use super::types::Element;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ax_identifier_priority() {
        let info = ElementInfo {
            ax_identifier: Some("my-button".to_string()),
            role: "button".to_string(),
            label: Some("Send".to_string()),
            path: "Window/VStack[0]/Button[2]".to_string(),
            relative_position: 2,
        };

        let id = StableElementId::generate(&info);
        assert_eq!(id.version(), 1);
        assert!(id.primary().starts_with("ax_id:"));
    }

    #[test]
    fn test_semantic_hash_fallback() {
        let info = ElementInfo {
            ax_identifier: None,
            role: "button".to_string(),
            label: Some("Send".to_string()),
            path: "Window/VStack[0]/Button[2]".to_string(),
            relative_position: 2,
        };

        let id = StableElementId::generate(&info);
        assert_eq!(id.version(), 2);
        assert!(id.primary().starts_with("sem:"));
    }

    #[test]
    fn test_path_hash_last_resort() {
        let info = ElementInfo {
            ax_identifier: None,
            role: "button".to_string(),
            label: None,
            path: "Window/VStack[0]/Button[2]".to_string(),
            relative_position: 2,
        };

        let id = StableElementId::generate(&info);
        assert_eq!(id.version(), 3);
        assert!(id.primary().starts_with("path:"));
    }

    #[test]
    fn test_strip_indices() {
        assert_eq!(
            StableElementId::strip_indices("Window/VStack[0]/Button[2]"),
            "Window/VStack/Button"
        );
        assert_eq!(
            StableElementId::strip_indices("Window/Button"),
            "Window/Button"
        );
    }

    #[test]
    fn test_same_semantic_same_hash() {
        let info1 = ElementInfo {
            ax_identifier: None,
            role: "button".to_string(),
            label: Some("Send".to_string()),
            path: "Window/VStack[0]/Button[2]".to_string(),
            relative_position: 2,
        };

        let info2 = ElementInfo {
            ax_identifier: None,
            role: "button".to_string(),
            label: Some("Send".to_string()),
            path: "Window/VStack[1]/Button[3]".to_string(),  // Different path
            relative_position: 2,
        };

        let id1 = StableElementId::generate(&info1);
        let id2 = StableElementId::generate(&info2);

        // Same semantic hash (role + label + position)
        assert_eq!(id1.primary(), id2.primary());
    }
}
