use crate::tabs::TabModel;
use futures::future::LocalBoxFuture;
use leptos::prelude::{RwSignal, Signal};
use serde_json::Value;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalConnection {
    pub connected: bool,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalEvent {
    pub topic: String,
    pub data: Value,
}

pub struct TerminalEventStream {
    pub coverage_epoch: u64,
    pub drain: Rc<dyn Fn() -> Result<Vec<TerminalEvent>, String>>,
    pub dispose: Rc<dyn Fn()>,
}

impl TerminalEventStream {
    #[must_use]
    pub fn new(
        coverage_epoch: u64,
        drain: Rc<dyn Fn() -> Result<Vec<TerminalEvent>, String>>,
        dispose: Rc<dyn Fn()>,
    ) -> Self {
        let disposed = Rc::new(Cell::new(false));
        let once = Rc::clone(&disposed);
        let dispose = Rc::new(move || {
            if !once.replace(true) {
                dispose();
            }
        });
        Self {
            coverage_epoch,
            drain,
            dispose,
        }
    }
}

impl Clone for TerminalEventStream {
    fn clone(&self) -> Self {
        Self {
            coverage_epoch: self.coverage_epoch,
            drain: Rc::clone(&self.drain),
            dispose: Rc::clone(&self.dispose),
        }
    }
}

pub type TerminalInvoke =
    Rc<dyn Fn(String, Value) -> LocalBoxFuture<'static, Result<Value, String>>>;
pub type TerminalSubscribe =
    Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<TerminalEventStream, String>>>;

#[derive(Clone)]
pub struct TerminalServices {
    pub invoke: TerminalInvoke,
    pub subscribe: TerminalSubscribe,
    pub connection: Signal<TerminalConnection>,
    pub text: TerminalText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalText {
    pub new_tab: String,
    pub close_tab: String,
    pub read_refusal: String,
    pub write_refusal: String,
}

#[derive(Clone)]
pub struct TerminalState {
    pub(crate) services: TerminalServices,
    pub(crate) tabs: RwSignal<TabModel>,
    pub(crate) selection_request: RwSignal<Option<String>>,
    pub(crate) new_request: RwSignal<u64>,
    pub(crate) close_request: RwSignal<Option<String>>,
}

#[must_use]
pub fn terminal_state(services: TerminalServices) -> TerminalState {
    TerminalState {
        services,
        tabs: RwSignal::new(TabModel::default()),
        selection_request: RwSignal::new(None),
        new_request: RwSignal::new(0),
        close_request: RwSignal::new(None),
    }
}
