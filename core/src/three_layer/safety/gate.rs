//! Capability Gate - enforces capability restrictions on tool execution

use super::Capability;
use std::collections::HashSet;

/// Error returned when a capability check fails
#[derive(Debug, Clone)]
pub struct CapabilityDenied {
    /// The capability that was required
    pub required: Capability,
    /// The capabilities that were granted
    pub granted: Vec<Capability>,
}

impl std::fmt::Display for CapabilityDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Capability '{}' denied. Granted: {:?}",
            self.required,
            self.granted.iter().map(|c| c.to_string()).collect::<Vec<_>>()
        )
    }
}

impl std::error::Error for CapabilityDenied {}

/// Gate that enforces capability restrictions
///
/// A Skill declares its required capabilities, and the gate ensures
/// only those capabilities can be used during execution.
#[derive(Debug, Clone)]
pub struct CapabilityGate {
    /// Capabilities that have been granted
    granted: HashSet<Capability>,
}

impl CapabilityGate {
    /// Create a new gate with specific granted capabilities
    pub fn new(capabilities: Vec<Capability>) -> Self {
        Self {
            granted: capabilities.into_iter().collect(),
        }
    }

    /// Create an empty gate (denies everything)
    pub fn empty() -> Self {
        Self {
            granted: HashSet::new(),
        }
    }

    /// Create a gate that allows all capabilities (for testing/admin)
    pub fn all() -> Self {
        Self {
            granted: vec![
                Capability::FileRead,
                Capability::FileList,
                Capability::FileWrite,
                Capability::FileDelete,
                Capability::WebSearch,
                Capability::WebFetch,
                Capability::LlmCall,
                Capability::ShellExec,
                Capability::ProcessSpawn,
            ]
            .into_iter()
            .collect(),
        }
    }

    /// Check if a capability is granted
    ///
    /// Returns Ok(()) if granted, Err(CapabilityDenied) if not.
    pub fn check(&self, required: &Capability) -> Result<(), CapabilityDenied> {
        if self.granted.contains(required) {
            Ok(())
        } else {
            Err(CapabilityDenied {
                required: required.clone(),
                granted: self.granted.iter().cloned().collect(),
            })
        }
    }

    /// Get all granted capabilities
    pub fn granted(&self) -> &HashSet<Capability> {
        &self.granted
    }

    /// Add a capability to the gate
    pub fn grant(&mut self, capability: Capability) {
        self.granted.insert(capability);
    }

    /// Remove a capability from the gate
    pub fn revoke(&mut self, capability: &Capability) {
        self.granted.remove(capability);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_check_granted() {
        let gate = CapabilityGate::new(vec![
            Capability::FileRead,
            Capability::WebSearch,
        ]);

        assert!(gate.check(&Capability::FileRead).is_ok());
        assert!(gate.check(&Capability::WebSearch).is_ok());
    }

    #[test]
    fn test_gate_check_denied() {
        let gate = CapabilityGate::new(vec![Capability::FileRead]);

        let result = gate.check(&Capability::FileWrite);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.required, Capability::FileWrite);
    }

    #[test]
    fn test_gate_check_mcp_specific() {
        let gate = CapabilityGate::new(vec![
            Capability::Mcp { server: "github".to_string() },
        ]);

        assert!(gate.check(&Capability::Mcp { server: "github".to_string() }).is_ok());
        assert!(gate.check(&Capability::Mcp { server: "slack".to_string() }).is_err());
    }

    #[test]
    fn test_gate_empty() {
        let gate = CapabilityGate::empty();
        assert!(gate.check(&Capability::FileRead).is_err());
    }

    #[test]
    fn test_gate_all() {
        let gate = CapabilityGate::all();
        assert!(gate.check(&Capability::FileRead).is_ok());
        assert!(gate.check(&Capability::ShellExec).is_ok());
    }
}
