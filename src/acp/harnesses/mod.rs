//! Concrete ACP harness adapters for supported CLI tools.

mod custom;
mod generic;

pub use custom::CustomHarness;
pub use generic::GenericAcpHarness;
