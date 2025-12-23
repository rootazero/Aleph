/// Input simulation trait (stub for Phase 2)
///
/// This module will contain keyboard/mouse simulation using enigo.
/// Phase 1: Trait definition only, no implementation.
use crate::error::Result;

/// Trait for simulating keyboard input
///
/// Phase 2 will implement this using the enigo crate to
/// simulate Cmd+X (cut), Cmd+V (paste), etc.
pub trait InputSimulator: Send + Sync {
    /// Simulate cut operation (Cmd+X on macOS)
    ///
    /// TODO: Implement in Phase 2 using enigo
    fn simulate_cut(&self) -> Result<()>;

    /// Simulate paste operation (Cmd+V on macOS)
    ///
    /// TODO: Implement in Phase 2 using enigo
    fn simulate_paste(&self) -> Result<()>;

    /// Simulate select all operation (Cmd+A on macOS)
    ///
    /// TODO: Implement in Phase 2 using enigo
    fn simulate_select_all(&self) -> Result<()>;
}

/// Placeholder input simulator for Phase 1
///
/// All methods are stubs that return Ok(()).
/// This allows the architecture to be established without
/// implementing the actual functionality yet.
#[allow(dead_code)]
pub struct PlaceholderSimulator;

impl PlaceholderSimulator {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlaceholderSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSimulator for PlaceholderSimulator {
    fn simulate_cut(&self) -> Result<()> {
        // TODO: Phase 2 - implement with enigo
        Ok(())
    }

    fn simulate_paste(&self) -> Result<()> {
        // TODO: Phase 2 - implement with enigo
        Ok(())
    }

    fn simulate_select_all(&self) -> Result<()> {
        // TODO: Phase 2 - implement with enigo
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_simulator_creation() {
        let _sim = PlaceholderSimulator::new();
        let _sim2 = PlaceholderSimulator;
        // Should not panic
    }

    #[test]
    fn test_placeholder_simulator_stubs() {
        let sim = PlaceholderSimulator::new();

        // All methods should return Ok (stubs)
        assert!(sim.simulate_cut().is_ok());
        assert!(sim.simulate_paste().is_ok());
        assert!(sim.simulate_select_all().is_ok());
    }
}
