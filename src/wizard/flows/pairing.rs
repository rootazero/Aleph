//! Same-machine PairingFlow — implemented in Task 6.

use crate::wizard::{RpcPrompter, WizardFlow, WizardSessionError};
use async_trait::async_trait;

/// Same-machine pairing flow — placeholder until Task 6 wires the body.
pub struct PairingFlow;

#[async_trait]
impl WizardFlow for PairingFlow {
    async fn run(&self, _prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        Err(WizardSessionError::FlowError(
            "PairingFlow body not yet implemented".to_string(),
        ))
    }

    fn name(&self) -> &str {
        "pairing"
    }
}
