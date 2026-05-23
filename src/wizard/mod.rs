//! Wizard system for guided configuration.
//!
//! Session-based wizard framework. The flow runs as a background task,
//! exchanges `WizardStep` ↔ answer pairs with the client through an
//! `RpcPrompter`, and surfaces its final payload via
//! `WizardNextResult.data` on the closing `wizard.next` call.

pub mod flows;
pub mod prompter;
pub mod session;
pub mod types;

pub use flows::pairing::PairingFlow;
pub use prompter::{ProgressHandle, RpcPrompter, WizardPrompter};
pub use session::{WizardFlow, WizardSession, WizardSessionError};
pub use types::{
    StepExecutor, StepType, WizardAnswer, WizardNextResult, WizardOption, WizardStatus, WizardStep,
};
