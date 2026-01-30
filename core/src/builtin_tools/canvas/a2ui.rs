//! A2UI Protocol Types
//!
//! Agent-to-UI protocol for dynamic component rendering.
//! Based on A2UI v0.8 specification.
//!
//! # Overview
//!
//! A2UI uses JSONL (JSON Lines) for streaming updates from agent to UI.
//! Each line is a complete JSON object representing an update command.
//!
//! # Message Types
//!
//! - `surfaceUpdate` - Define or update components on a surface
//! - `beginRendering` - Start rendering a surface with a root component
//! - `dataModelUpdate` - Update data bindings
//! - `userAction` - User interaction callback (UI → Agent)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Server → Client Messages
// ============================================================================

/// A2UI message from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum A2uiMessage {
    /// Update components on a surface
    SurfaceUpdate(SurfaceUpdate),
    /// Begin rendering a surface
    BeginRendering(BeginRendering),
    /// Update data model
    DataModelUpdate(DataModelUpdate),
}

/// Surface update command
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceUpdate {
    /// Surface identifier
    pub surface_id: String,
    /// Components to add/update
    pub components: Vec<Component>,
}

/// Begin rendering command
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginRendering {
    /// Surface identifier
    pub surface_id: String,
    /// Root component ID to render
    pub root: String,
}

/// Data model update command
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataModelUpdate {
    /// Surface identifier
    pub surface_id: String,
    /// Data updates (path → value)
    pub updates: Vec<DataUpdate>,
}

/// Single data update
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataUpdate {
    /// JSON path to update
    pub path: String,
    /// New value
    pub value: serde_json::Value,
}

// ============================================================================
// Components
// ============================================================================

/// A2UI component definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    /// Unique component identifier
    pub id: String,
    /// Component type
    #[serde(rename = "type")]
    pub component_type: ComponentType,
    /// Component properties
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub props: HashMap<String, serde_json::Value>,
    /// Child component IDs
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    /// Event handlers
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handlers: Vec<EventHandler>,
}

/// Component types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentType {
    // Layout
    Container,
    Row,
    Column,
    Stack,
    Scroll,
    // Content
    Text,
    Image,
    Icon,
    Markdown,
    // Interactive
    Button,
    Input,
    TextArea,
    Select,
    Checkbox,
    Radio,
    Slider,
    Toggle,
    // Data Display
    Table,
    List,
    Card,
    Badge,
    Progress,
    // Feedback
    Alert,
    Toast,
    Modal,
    Tooltip,
    // Custom
    #[serde(other)]
    Custom,
}

/// Event handler definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventHandler {
    /// Event name (e.g., "click", "change", "submit")
    pub event: String,
    /// Action name to send back
    pub action: String,
}

// ============================================================================
// Client → Server Messages
// ============================================================================

/// User action from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAction {
    /// Action name
    pub name: String,
    /// Surface identifier
    pub surface_id: String,
    /// Source component ID
    pub source_component_id: String,
    /// Action context/payload
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

// ============================================================================
// JSONL Parsing
// ============================================================================

/// Parse a single A2UI message from JSON
pub fn parse_message(json: &str) -> Result<A2uiMessage, A2uiParseError> {
    // Try to parse as a wrapper object first
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| A2uiParseError::InvalidJson(e.to_string()))?;

    let obj = value
        .as_object()
        .ok_or_else(|| A2uiParseError::InvalidFormat("Expected JSON object".to_string()))?;

    if let Some(surface_update) = obj.get("surfaceUpdate") {
        let update: SurfaceUpdate = serde_json::from_value(surface_update.clone())
            .map_err(|e| A2uiParseError::InvalidMessage(e.to_string()))?;
        return Ok(A2uiMessage::SurfaceUpdate(update));
    }

    if let Some(begin_rendering) = obj.get("beginRendering") {
        let rendering: BeginRendering = serde_json::from_value(begin_rendering.clone())
            .map_err(|e| A2uiParseError::InvalidMessage(e.to_string()))?;
        return Ok(A2uiMessage::BeginRendering(rendering));
    }

    if let Some(data_update) = obj.get("dataModelUpdate") {
        let update: DataModelUpdate = serde_json::from_value(data_update.clone())
            .map_err(|e| A2uiParseError::InvalidMessage(e.to_string()))?;
        return Ok(A2uiMessage::DataModelUpdate(update));
    }

    Err(A2uiParseError::UnknownMessageType)
}

/// Parse multiple A2UI messages from JSONL
pub fn parse_jsonl(jsonl: &str) -> Result<Vec<A2uiMessage>, A2uiParseError> {
    let mut messages = Vec::new();

    for (line_num, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match parse_message(line) {
            Ok(msg) => messages.push(msg),
            Err(e) => {
                return Err(A2uiParseError::LineError {
                    line: line_num + 1,
                    error: Box::new(e),
                })
            }
        }
    }

    Ok(messages)
}

/// Validate JSONL content without fully parsing
pub fn validate_jsonl(jsonl: &str) -> Result<usize, A2uiParseError> {
    let messages = parse_jsonl(jsonl)?;
    Ok(messages.len())
}

/// A2UI parsing errors
#[derive(Debug, thiserror::Error)]
pub enum A2uiParseError {
    #[error("Invalid JSON: {0}")]
    InvalidJson(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Unknown message type")]
    UnknownMessageType,

    #[error("Error on line {line}: {error}")]
    LineError {
        line: usize,
        error: Box<A2uiParseError>,
    },
}

// ============================================================================
// Surface Manager
// ============================================================================

/// Manages A2UI surfaces and their state
#[derive(Debug, Default)]
pub struct SurfaceManager {
    /// Active surfaces
    surfaces: HashMap<String, Surface>,
}

/// A single A2UI surface
#[derive(Debug, Default)]
pub struct Surface {
    /// Surface identifier
    pub id: String,
    /// Components by ID
    pub components: HashMap<String, Component>,
    /// Root component ID
    pub root: Option<String>,
    /// Data model
    pub data: HashMap<String, serde_json::Value>,
}

impl SurfaceManager {
    /// Create a new surface manager
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a message to the surfaces
    pub fn apply(&mut self, message: &A2uiMessage) {
        match message {
            A2uiMessage::SurfaceUpdate(update) => {
                let surface = self
                    .surfaces
                    .entry(update.surface_id.clone())
                    .or_insert_with(|| Surface {
                        id: update.surface_id.clone(),
                        ..Default::default()
                    });

                for component in &update.components {
                    surface
                        .components
                        .insert(component.id.clone(), component.clone());
                }
            }
            A2uiMessage::BeginRendering(rendering) => {
                if let Some(surface) = self.surfaces.get_mut(&rendering.surface_id) {
                    surface.root = Some(rendering.root.clone());
                }
            }
            A2uiMessage::DataModelUpdate(update) => {
                if let Some(surface) = self.surfaces.get_mut(&update.surface_id) {
                    for data_update in &update.updates {
                        surface
                            .data
                            .insert(data_update.path.clone(), data_update.value.clone());
                    }
                }
            }
        }
    }

    /// Get a surface by ID
    pub fn get(&self, surface_id: &str) -> Option<&Surface> {
        self.surfaces.get(surface_id)
    }

    /// List all surface IDs
    pub fn list(&self) -> Vec<&str> {
        self.surfaces.keys().map(|s| s.as_str()).collect()
    }

    /// Clear all surfaces
    pub fn clear(&mut self) {
        self.surfaces.clear();
    }

    /// Clear a specific surface
    pub fn clear_surface(&mut self, surface_id: &str) {
        self.surfaces.remove(surface_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_surface_update() {
        let json = r#"{"surfaceUpdate":{"surfaceId":"main","components":[{"id":"root","type":"container","children":["text1"]}]}}"#;
        let msg = parse_message(json).unwrap();

        if let A2uiMessage::SurfaceUpdate(update) = msg {
            assert_eq!(update.surface_id, "main");
            assert_eq!(update.components.len(), 1);
            assert_eq!(update.components[0].id, "root");
        } else {
            panic!("Expected SurfaceUpdate");
        }
    }

    #[test]
    fn test_parse_begin_rendering() {
        let json = r#"{"beginRendering":{"surfaceId":"main","root":"root"}}"#;
        let msg = parse_message(json).unwrap();

        if let A2uiMessage::BeginRendering(rendering) = msg {
            assert_eq!(rendering.surface_id, "main");
            assert_eq!(rendering.root, "root");
        } else {
            panic!("Expected BeginRendering");
        }
    }

    #[test]
    fn test_parse_data_model_update() {
        let json = r#"{"dataModelUpdate":{"surfaceId":"main","updates":[{"path":"user.name","value":"Alice"}]}}"#;
        let msg = parse_message(json).unwrap();

        if let A2uiMessage::DataModelUpdate(update) = msg {
            assert_eq!(update.surface_id, "main");
            assert_eq!(update.updates.len(), 1);
            assert_eq!(update.updates[0].path, "user.name");
        } else {
            panic!("Expected DataModelUpdate");
        }
    }

    #[test]
    fn test_parse_jsonl() {
        let jsonl = r#"
{"surfaceUpdate":{"surfaceId":"main","components":[{"id":"root","type":"container"}]}}
{"beginRendering":{"surfaceId":"main","root":"root"}}
"#;
        let messages = parse_jsonl(jsonl).unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_validate_jsonl() {
        let jsonl = r#"{"surfaceUpdate":{"surfaceId":"main","components":[]}}"#;
        let count = validate_jsonl(jsonl).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_invalid_json() {
        let result = parse_message("not valid json");
        assert!(matches!(result, Err(A2uiParseError::InvalidJson(_))));
    }

    #[test]
    fn test_unknown_message_type() {
        let result = parse_message(r#"{"unknownType":{}}"#);
        assert!(matches!(result, Err(A2uiParseError::UnknownMessageType)));
    }

    #[test]
    fn test_surface_manager() {
        let mut manager = SurfaceManager::new();

        // Apply surface update
        let update = A2uiMessage::SurfaceUpdate(SurfaceUpdate {
            surface_id: "main".to_string(),
            components: vec![Component {
                id: "root".to_string(),
                component_type: ComponentType::Container,
                props: HashMap::new(),
                children: vec![],
                handlers: vec![],
            }],
        });
        manager.apply(&update);

        // Apply begin rendering
        let rendering = A2uiMessage::BeginRendering(BeginRendering {
            surface_id: "main".to_string(),
            root: "root".to_string(),
        });
        manager.apply(&rendering);

        // Check state
        let surface = manager.get("main").unwrap();
        assert_eq!(surface.root, Some("root".to_string()));
        assert!(surface.components.contains_key("root"));
    }

    #[test]
    fn test_user_action() {
        let action = UserAction {
            name: "submit".to_string(),
            surface_id: "main".to_string(),
            source_component_id: "btn-1".to_string(),
            context: HashMap::new(),
        };

        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("submit"));
    }

    #[test]
    fn test_component_types() {
        let json = r#""button""#;
        let ct: ComponentType = serde_json::from_str(json).unwrap();
        assert_eq!(ct, ComponentType::Button);

        // Unknown types should parse as Custom
        let json = r#""myCustomType""#;
        let ct: ComponentType = serde_json::from_str(json).unwrap();
        assert_eq!(ct, ComponentType::Custom);
    }
}
