//! Wizard system for guided configuration.
//!
//! Provides a session-based wizard framework for onboarding and configuration flows.
//!
//! # Architecture
//!
//! The wizard system uses a session-based state machine with deferred promises:
//!
//! ```text
//! Client                        WizardSession                     WizardFlow
//!    │                               │                                │
//!    │── wizard.start ──────────────▶│                                │
//!    │                               │── run(prompter) ──────────────▶│
//!    │◀── { step: intro } ───────────│◀── prompt(intro) ──────────────│
//!    │                               │                                │
//!    │── wizard.next { null } ──────▶│                                │
//!    │◀── { step: select } ──────────│◀── prompt(select) ─────────────│
//!    │                               │                                │
//!    │── wizard.next { "local" } ───▶│                                │
//!    │                               │── answer("local") ────────────▶│
//!    │◀── { step: text } ────────────│◀── prompt(text) ───────────────│
//!    │                               │                                │
//!    │── wizard.cancel ─────────────▶│                                │
//!    │◀── { done: true } ────────────│                                │
//! ```

pub mod flows;
pub mod prompter;
pub mod session;
pub mod types;

pub use flows::{OnboardingFlow, onboarding::{OnboardingData, ProviderSetupFlow, QuickSetupFlow}};
pub use prompter::{CliPrompter, ProgressHandle, RpcPrompter, WizardPrompter};
pub use session::{WizardFlow, WizardSession, WizardSessionError};
pub use types::{
    StepExecutor, StepType, WizardNextResult, WizardOption, WizardStatus, WizardStep,
};
