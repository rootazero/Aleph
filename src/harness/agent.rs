//! AgentHarness — the concrete Think→Act implementation.
//!
//! The Think phase (Task 8) and Act phase (Task 9) will replace the `todo!`
//! below. The struct and trait impl are established here so that the module
//! hierarchy and object-safety doc-test compile cleanly from the outset.

use async_trait::async_trait;

use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState};
use crate::session::service::SessionId;

pub struct AgentHarness {
    deps: HarnessDeps,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(&self, _session_id: &SessionId) -> Result<TurnState, HarnessError> {
        let _ = &self.deps; // suppress unused-field lint until Tasks 8–9 fill this in
        todo!("Task 8: Think phase")
    }
}
