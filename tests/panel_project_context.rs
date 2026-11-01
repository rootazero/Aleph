//! Focused private-module tests; ingress acceptance uses the production router.
pub use alephcore::{extension, gateway, projects, sync_primitives};

mod panel_project_context {
    include!("../src/gateway/panel_project_context.rs");

    mod tests {
        include!("../src/gateway/panel_project_context_tests.rs");
    }
}
