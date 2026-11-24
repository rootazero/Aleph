//! Startup-only Policy assembly shared by the server and library composition.
use crate::capability::ownership::OwnershipTree;
use crate::capability::policy::{PolicyDenial, PolicyEngine};
use crate::sync_primitives::Arc;
use crate::Config;

/// Assemble startup restrictions using the supplied canonical ownership tree.
/// This is pure assembly, not authorization or a second ownership authority.
pub fn build_capability_policy(
    config: &Config,
    tree: Arc<OwnershipTree>,
) -> Result<Arc<PolicyEngine>, PolicyDenial> {
    PolicyEngine::with_shared_tree(tree, config.policies.capability_constraints.clone())
        .map(Arc::new)
}
