//! Lightweight UI capability primitives for the Core layer.
//!
//! The in-memory implementation is an intentional bridge stub. Panel owns the
//! real rendering and input transport; Core only keeps the typed capability
//! contract and a deterministic adapter for tests and future bridges.

use crate::capability::backend::{
    LegacyCapabilityDispatchError, LegacyCapabilityHandler, LegacyCapabilityHandlerRegistry,
    LegacyCapabilityRegistryError,
};
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::sync_primitives::Arc;
use aleph_protocol::capability::UiInputEvent;
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
    #[error("UI surface is unavailable")]
    Unavailable,
    #[error("UI surface attachment is stale")]
    StaleAttachment,
    #[error("UI input queue is full")]
    QueueFull,
    #[error("UI surface already has a pending display")]
    Busy,
    #[error("UI render acknowledgement reported failure")]
    RenderFailed,
    #[error("UI render acknowledgement timed out")]
    Timeout,
    #[error("UI display was cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiElement {
    Text(String),
}

fn input_to_json(event: &UiInputEvent) -> Value {
    match event {
        UiInputEvent::KeyDown { key } => json!({"kind":"key_down","key":key}),
        UiInputEvent::MouseDown { x, y, button } => {
            json!({"kind":"mouse_down","x":x,"y":y,"button":button})
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

    pub fn display_text(&self, text: impl Into<String>) -> Result<(), UiError> {
        let closed = self.closed.lock().map_err(|_| UiError::Poisoned)?;
        if *closed {
            return Err(UiError::Closed);
        }
        self.display
            .lock()
            .map_err(|_| UiError::Poisoned)?
            .push(UiElement::Text(text.into()));
        Ok(())
    }

    pub fn display_snapshot(&self) -> Result<Vec<UiElement>, UiError> {
        let closed = self.closed.lock().map_err(|_| UiError::Poisoned)?;
        if *closed {
            return Err(UiError::Closed);
        }
        Ok(self.display.lock().map_err(|_| UiError::Poisoned)?.clone())
    }

    pub fn push_input(&self, event: UiInputEvent) -> Result<(), UiError> {
        let closed = self.closed.lock().map_err(|_| UiError::Poisoned)?;
        if *closed {
            return Err(UiError::Closed);
        }
        self.input
            .lock()
            .map_err(|_| UiError::Poisoned)?
            .push_back(event);
        Ok(())
    }

    pub fn capture_input(&self) -> Result<Option<UiInputEvent>, UiError> {
        let closed = self.closed.lock().map_err(|_| UiError::Poisoned)?;
        if *closed {
            return Err(UiError::Closed);
        }
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

impl LegacyCapabilityHandler for UiHandler {
    fn invoke(&self, input: Value) -> Result<Value, LegacyCapabilityDispatchError> {
        let input = input.as_object().ok_or_else(|| {
            LegacyCapabilityDispatchError::InvalidInput("UI input must be an object".into())
        })?;
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                LegacyCapabilityDispatchError::InvalidInput("UI operation is required".into())
            })?;
        match operation {
            "display" => {
                let text = input.get("text").and_then(Value::as_str).ok_or_else(|| {
                    LegacyCapabilityDispatchError::InvalidInput("display text is required".into())
                })?;
                self.capability
                    .display_text(text)
                    .map_err(|error| LegacyCapabilityDispatchError::Execution(error.to_string()))?;
                Ok(json!({"displayed":true}))
            }
            "input" => self
                .capability
                .capture_input()
                .map(|event| event.map_or(Value::Null, |event| input_to_json(&event)))
                .map_err(|error| LegacyCapabilityDispatchError::Execution(error.to_string())),
            other => Err(LegacyCapabilityDispatchError::InvalidInput(format!(
                "unsupported UI operation: {other}"
            ))),
        }
    }
}

pub(crate) fn schema() -> Value {
    json!({
        "oneOf":[
            {
                "type":"object",
                "properties":{
                    "operation":{"const":"display"},
                    "text":{"type":"string"}
                },
                "required":["operation","text"],
                "additionalProperties":false
            },
            {
                "type":"object",
                "properties":{
                    "operation":{"const":"input"}
                },
                "required":["operation"],
                "additionalProperties":false
            }
        ]
    })
}

pub(crate) fn descriptor() -> CapabilityDescriptor {
    let schema = schema();
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
    registry: &LegacyCapabilityHandlerRegistry,
    capability: Arc<UiCapability>,
) -> Result<CapabilityId, LegacyCapabilityRegistryError> {
    registry.register(descriptor(), Arc::new(UiHandler::new(capability)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::backend::{CapabilityBackend, LegacyCapabilityHandlerRegistry};
    use crate::sync_primitives::Arc;
    use serde_json::json;
    use std::sync::Barrier;
    use std::thread;

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
    fn ui_close_serializes_with_concurrent_display_and_rejects_post_close_operations() {
        let ui = Arc::new(UiCapability::in_memory());
        let start = Arc::new(Barrier::new(2));
        let finish_close = Arc::new(Barrier::new(2));
        let worker_ui = ui.clone();
        let worker_start = start.clone();
        let worker_finish = finish_close.clone();
        let worker = thread::spawn(move || {
            worker_start.wait();
            let concurrent = worker_ui.display_text("concurrent");
            worker_finish.wait();
            let post_close = worker_ui.push_input(UiInputEvent::KeyDown {
                key: "after-close".into(),
            });
            (concurrent, post_close)
        });

        start.wait();
        ui.close().unwrap();
        finish_close.wait();
        let (_concurrent, post_close) = worker.join().unwrap();

        assert!(post_close.is_err());
        assert!(ui.display_text("after-close").is_err());
        assert!(ui.capture_input().is_err());
    }

    #[test]
    fn ui_handler_dispatches_display_and_input_operations() {
        let ui = Arc::new(UiCapability::in_memory());
        ui.push_input(UiInputEvent::KeyDown {
            key: "Enter".into(),
        })
        .unwrap();
        let registry = LegacyCapabilityHandlerRegistry::new();
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
        let registry = LegacyCapabilityHandlerRegistry::new();
        let id = register_handler(&registry, Arc::new(UiCapability::in_memory())).unwrap();

        assert!(matches!(
            registry.dispatch(&id, json!({"operation":"click"})),
            Err(crate::capability::backend::LegacyCapabilityDispatchError::InvalidInput(_))
        ));
    }

    #[test]
    fn ui_handler_exposes_ui_descriptor() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        let id = register_handler(&registry, Arc::new(UiCapability::in_memory())).unwrap();

        assert_eq!(registry.lookup(&id).unwrap().kind, CapabilityKind::Ui);
    }
}
