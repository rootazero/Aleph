//! Wizard system for guided configuration.
//!
//! Session-based wizard framework. The flow runs as a background task,
//! exchanges `WizardStep` ↔ answer pairs with the client through an
//! `RpcPrompter`, and surfaces its terminal status via the closing
//! `wizard.next` call.

pub mod flows;
pub mod prompter;
pub mod session;
pub mod types;

pub use flows::onboarding::{OnboardingData, OnboardingFlow};
pub use prompter::{RpcPrompter, WizardPrompter};
pub use session::{WizardFlow, WizardSession, WizardSessionError};
pub use types::{
    StepExecutor, StepType, WizardNextResult, WizardOption, WizardStatus, WizardStep,
};
