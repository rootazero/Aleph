//! ToolService middleware layers.

pub mod audit;
pub mod context_rule;
pub mod permission;
pub mod timeout;

pub use audit::ExecAuditLayer;
pub use context_rule::ContextRuleLayer;
pub use permission::PermissionLayer;
pub use timeout::TimeoutLayer;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::tools::dispatch::CoreDispatch;
    use crate::tools::registry::ToolRegistry;
    use crate::tools::service::ToolService;

    #[tokio::test]
    async fn empty_chain_compiles_and_lists_empty() {
        let registry = Arc::new(ToolRegistry::new());
        let core = Arc::new(CoreDispatch::new(registry));
        let timeout = Arc::new(TimeoutLayer::new(
            core,
            std::time::Duration::from_secs(60),
            std::collections::HashMap::new(),
        ));
        let ctx = Arc::new(ContextRuleLayer::new(timeout));
        let perm = Arc::new(PermissionLayer::new(ctx));
        let audit = Arc::new(ExecAuditLayer::new(perm));
        let svc: Arc<dyn ToolService> = audit;
        assert!(svc.list().await.is_empty());
    }
}
