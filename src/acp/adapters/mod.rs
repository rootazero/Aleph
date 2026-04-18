//! Concrete ACP harness adapters for supported CLI tools.

mod custom;
mod generic;

pub use custom::CustomAcpAdapter;
pub use generic::GenericAcpAdapter;
