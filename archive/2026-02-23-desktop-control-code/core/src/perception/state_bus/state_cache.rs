//! State cache for real-time coordinate mapping.

use super::types::{AppState, Element};
use std::collections::HashMap;

/// In-memory cache of current application states.
pub struct StateCache {
    /// Map: app_id -> AppState
    states: HashMap<String, AppState>,

    /// Map: element_id -> (app_id, element_index)
    element_index: HashMap<String, (String, usize)>,
}

impl StateCache {
    /// Create a new empty state cache.
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            element_index: HashMap::new(),
        }
    }

    /// Update state for an application.
    pub fn update(&mut self, state: AppState) {
        let app_id = state.app_id.clone();

        // Rebuild element index for this app
        for (idx, element) in state.elements.iter().enumerate() {
            self.element_index.insert(
                element.id.clone(),
                (app_id.clone(), idx),
            );
        }

        // Store state
        self.states.insert(app_id, state);
    }

    /// Get state for an application.
    pub fn get(&self, app_id: &str) -> Option<&AppState> {
        self.states.get(app_id)
    }

    /// Get element by ID (across all apps).
    pub fn get_element(&self, element_id: &str) -> Option<&Element> {
        let (app_id, idx) = self.element_index.get(element_id)?;
        let state = self.states.get(app_id)?;
        state.elements.get(*idx)
    }

    /// Remove state for an application.
    pub fn remove(&mut self, app_id: &str) -> Option<AppState> {
        // Remove from element index
        if let Some(state) = self.states.get(app_id) {
            for element in &state.elements {
                self.element_index.remove(&element.id);
            }
        }

        // Remove state
        self.states.remove(app_id)
    }

    /// Get all application IDs.
    pub fn app_ids(&self) -> Vec<String> {
        self.states.keys().cloned().collect()
    }

    /// Clear all cached states.
    pub fn clear(&mut self) {
        self.states.clear();
        self.element_index.clear();
    }
}

impl Default for StateCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::state_bus::types::{ElementSource, ElementState, StateSource};

    #[test]
    fn test_state_cache_basic() {
        let mut cache = StateCache::new();

        let state = AppState {
            app_id: "com.apple.mail".to_string(),
            elements: vec![
                Element {
                    id: "btn_001".to_string(),
                    role: "button".to_string(),
                    label: Some("Send".to_string()),
                    current_value: None,
                    rect: None,
                    state: ElementState::default(),
                    source: ElementSource::Ax,
                    confidence: 1.0,
                },
            ],
            app_context: None,
            source: StateSource::Accessibility,
            confidence: 1.0,
        };

        cache.update(state);

        assert!(cache.get("com.apple.mail").is_some());
        assert!(cache.get_element("btn_001").is_some());
        assert_eq!(cache.app_ids().len(), 1);
    }

    #[test]
    fn test_state_cache_element_lookup() {
        let mut cache = StateCache::new();

        let state = AppState {
            app_id: "com.apple.mail".to_string(),
            elements: vec![
                Element {
                    id: "btn_send".to_string(),
                    role: "button".to_string(),
                    label: Some("Send".to_string()),
                    current_value: None,
                    rect: None,
                    state: ElementState::default(),
                    source: ElementSource::Ax,
                    confidence: 1.0,
                },
                Element {
                    id: "input_subject".to_string(),
                    role: "textfield".to_string(),
                    label: Some("Subject".to_string()),
                    current_value: Some("Hello".to_string()),
                    rect: None,
                    state: ElementState::default(),
                    source: ElementSource::Ax,
                    confidence: 1.0,
                },
            ],
            app_context: None,
            source: StateSource::Accessibility,
            confidence: 1.0,
        };

        cache.update(state);

        let element = cache.get_element("input_subject").unwrap();
        assert_eq!(element.role, "textfield");
        assert_eq!(element.current_value.as_ref().unwrap(), "Hello");
    }

    #[test]
    fn test_state_cache_remove() {
        let mut cache = StateCache::new();

        let state = AppState {
            app_id: "com.apple.mail".to_string(),
            elements: vec![],
            app_context: None,
            source: StateSource::Accessibility,
            confidence: 1.0,
        };

        cache.update(state);
        assert!(cache.get("com.apple.mail").is_some());

        cache.remove("com.apple.mail");
        assert!(cache.get("com.apple.mail").is_none());
    }
}
