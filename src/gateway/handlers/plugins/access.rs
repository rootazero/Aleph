use std::future::Future;

use crate::extension::plugin_target::PluginAccess;
use crate::gateway::caller_identity::{
    current_caller_is_loopback, current_caller_role, current_caller_verified_user,
};
use crate::gateway::panel_project_context::PanelRequestContext;

// Propagate authenticated dispatch context into the existing plugin and Hub
// handlers, including closures registered through the plain handler API.
tokio::task_local! {
    static PLUGIN_ACCESS: PluginAccess;
}

pub(crate) fn is_management_method(method: &str) -> bool {
    method.starts_with("plugin.")
        || method.starts_with("plugins.")
        || method.starts_with("extensions.")
}

pub(crate) fn current_plugin_access() -> PluginAccess {
    PLUGIN_ACCESS
        .try_with(Clone::clone)
        .unwrap_or_else(|_| PluginAccess::global())
}

pub(crate) async fn with_plugin_access<T>(
    context: Option<&PanelRequestContext>,
    handle: impl Future<Output = T>,
) -> T {
    let legacy_local = current_caller_is_loopback()
        && current_caller_role().as_deref() == Some("operator")
        && current_caller_verified_user().is_none();
    let access = match context {
        Some(context) => match context.selector.resolve(&context.caller) {
            Ok(_) if legacy_local && !context.caller.has_verified_principal() => {
                PluginAccess::LocalOperator
            }
            Ok(resolved) => PluginAccess::Scoped(resolved.visibility),
            Err(_) => PluginAccess::Denied,
        },
        None if legacy_local => PluginAccess::LocalOperator,
        None => PluginAccess::global(),
    };
    PLUGIN_ACCESS.scope(access, handle).await
}
