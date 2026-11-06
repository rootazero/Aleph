//! Lightweight UI capability primitives for the Core layer.
//!
//! The in-memory implementation is an intentional bridge stub. Panel owns the
//! real rendering and input transport; Core only keeps the typed capability
//! contract and a deterministic adapter for tests and future bridges.

use crate::capability::backend::{
    CapabilityDispatchError, CapabilityHandler, CapabilityHandlerRegistry, CapabilityRegistryError,
};
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::sync_primitives::Arc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const UI_NAMESPACE: &str = "aleph/ui";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UiError {
    #[error("UI capability is closed")]
    Closed,
    #[error("UI input is invalid: {0}")]
    InvalidInput(String),
    #[error("UI state lock is poisoned")]
    Poisoned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiElement {
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiInputEvent {
    KeyDown { key: String },
    MouseDown { x: i32, y: i32, button: u8 },
}

impl UiInputEvent {
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::KeyDown { key } => json!({"kind":"key_down","key":key}),
            Self::MouseDown { x, y, button } => {
                json!({"kind":"mouse_down","x":x,"y":y,"button":button})
            }
        }
    }
}

/// An in-memory UI bridge stub with immutable read snapshots.
pub struct UiCapability {
    display: Mutex<Vec<UiElement>>,
    input: Mutex<VecDeque<UiInputEvent>>,
    closed: Mutex<bool>,
}

impl UiCapability {
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            display: Mutex::new(Vec::new()),
            input: Mutex::new(VecDeque::new()),
            closed: Mutex::new(false),
        }
    }

    fn ensure_open(&self) -> Result<(), UiError> {
        let closed = self.closed.lock().map_err(|_| UiError::Poisoned)?;
        if *closed {
            return Err(UiError::Closed);
        }
        Ok(())
    }

    pub fn display_text(&self, text: impl Into<String>) -> Result<(), UiError> {
        self.ensure_open()?;
        self.display
            .lock()
            .map_err(|_| UiError::Poisoned)?
            .push(UiElement::Text(text.into()));
        Ok(())
    }

    pub fn display_snapshot(&self) -> Result<Vec<UiElement>, UiError> {
        self.ensure_open()?;
        Ok(self.display.lock().map_err(|_| UiError::Poisoned)?.clone())
    }

    pub fn push_input(&self, event: UiInputEvent) -> Result<(), UiError> {
        self.ensure_open()?;
        self.input
            .lock()
            .map_err(|_| UiError::Poisoned)?
            .push_back(event);
        Ok(())
    }

    pub fn capture_input(&self) -> Result<Option<UiInputEvent>, UiError> {
        self.ensure_open()?;
        Ok(self
            .input
            .lock()
            .map_err(|_| UiError::Poisoned)?
            .pop_front())
    }

    pub fn close(&self) -> Result<(), UiError> {
        *self.closed.lock().map_err(|_| UiError::Poisoned)? = true;
        Ok(())
    }
}

pub struct UiHandler {
    capability: Arc<UiCapability>,
}

impl UiHandler {
    #[must_use]
    pub fn new(capability: Arc<UiCapability>) -> Self {
        Self { capability }
    }
}

impl CapabilityHandler for UiHandler {
    fn invoke(&self, input: Value) -> Result<Value, CapabilityDispatchError> {
        let input = input.as_object().ok_or_else(|| {
            CapabilityDispatchError::InvalidInput("UI input must be an object".into())
        })?;
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CapabilityDispatchError::InvalidInput("UI operation is required".into())
            })?;
        match operation {
            "display" => {
                let text = input.get("text").and_then(Value::as_str).ok_or_else(|| {
                    CapabilityDispatchError::InvalidInput("display text is required".into())
                })?;
                self.capability
                    .display_text(text)
                    .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))?;
                Ok(json!({"displayed":true}))
            }
            "input" => self
                .capability
                .capture_input()
                .map(|event| event.map_or(Value::Null, |event| event.to_json()))
                .map_err(|error| CapabilityDispatchError::Execution(error.to_string())),
            other => Err(CapabilityDispatchError::InvalidInput(format!(
                "unsupported UI operation: {other}"
            ))),
        }
    }
}

fn descriptor() -> CapabilityDescriptor {
    let schema = json!({
        "type":"object",
        "required":["operation"],
        "additionalProperties":true
    });
    let mut bytes = Vec::new();
    crate::tools::descriptor::canonicalize_json(&schema, &mut bytes);
    CapabilityDescriptor {
        id: CapabilityId {
            namespace: UI_NAMESPACE.into(),
            name: "surface".into(),
        },
        kind: CapabilityKind::Ui,
        schema: SchemaRef {
            version: 1,
            fingerprint: Sha256::digest(bytes).into(),
        },
        revision: CapabilityRevision(0),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: Vec::new(),
        metadata: ProjectionMetadata::default(),
        facets: CapabilityFacets::new().with(CapabilityFacet::Callable),
    }
}

pub fn register_handler(
    registry: &CapabilityHandlerRegistry,
    capability: Arc<UiCapability>,
) -> Result<CapabilityId, CapabilityRegistryError> {
    registry.register(descriptor(), Arc::new(UiHandler::new(capability)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::backend::{CapabilityBackend, CapabilityHandlerRegistry};
    use crate::sync_primitives::Arc;
    use serde_json::json;

    #[test]
    fn ui_displays_text_in_an_immutable_snapshot() {
        let ui = UiCapability::in_memory();
        ui.display_text("hello").unwrap();

        assert_eq!(
            ui.display_snapshot().unwrap(),
            vec![UiElement::Text("hello".into())]
        );
    }

    #[test]
    fn ui_captures_keyboard_and_mouse_input() {
        let ui = UiCapability::in_memory();
        ui.push_input(UiInputEvent::KeyDown { key: "a".into() })
            .unwrap();
        ui.push_input(UiInputEvent::MouseDown {
            x: 10,
            y: 20,
            button: 1,
        })
        .unwrap();

        assert_eq!(
            ui.capture_input().unwrap(),
            Some(UiInputEvent::KeyDown { key: "a".into() })
        );
        assert_eq!(
            ui.capture_input().unwrap(),
            Some(UiInputEvent::MouseDown {
                x: 10,
                y: 20,
                button: 1,
            })
        );
        assert_eq!(ui.capture_input().unwrap(), None);
    }

    #[test]
    fn ui_closed_surface_rejects_display_and_input() {
        let ui = UiCapability::in_memory();
        ui.close().unwrap();

        assert!(matches!(ui.display_text("hidden"), Err(UiError::Closed)));
        assert!(matches!(ui.capture_input(), Err(UiError::Closed)));
        assert!(matches!(
            ui.push_input(UiInputEvent::KeyDown { key: "x".into() }),
            Err(UiError::Closed)
        ));
    }

    #[test]
    fn ui_handler_dispatches_display_and_input_operations() {
        let ui = Arc::new(UiCapability::in_memory());
        ui.push_input(UiInputEvent::KeyDown {
            key: "Enter".into(),
        })
        .unwrap();
        let registry = CapabilityHandlerRegistry::new();
        let id = register_handler(&registry, ui.clone()).unwrap();

        assert_eq!(
            registry
                .dispatch(&id, json!({"operation":"display","text":"ready"}))
                .unwrap(),
            json!({"displayed":true})
        );
        assert_eq!(
            registry
                .dispatch(&id, json!({"operation":"input"}))
                .unwrap(),
            json!({"kind":"key_down","key":"Enter"})
        );
        assert_eq!(
            ui.display_snapshot().unwrap(),
            vec![UiElement::Text("ready".into())]
        );
    }

    #[test]
    fn ui_handler_rejects_invalid_operations() {
        let registry = CapabilityHandlerRegistry::new();
        let id = register_handler(&registry, Arc::new(UiCapability::in_memory())).unwrap();

        assert!(matches!(
            registry.dispatch(&id, json!({"operation":"click"})),
            Err(crate::capability::backend::CapabilityDispatchError::InvalidInput(_))
        ));
    }

    #[test]
    fn ui_handler_exposes_ui_descriptor() {
        let registry = CapabilityHandlerRegistry::new();
        let id = register_handler(&registry, Arc::new(UiCapability::in_memory())).unwrap();

        assert_eq!(registry.lookup(&id).unwrap().kind, CapabilityKind::Ui);
    }
}
