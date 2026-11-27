//! Resolve plugin management targets against the caller's authorized scope.

use super::visibility::{PluginInstanceKey, ScopeKey, VisibilityCtx, visible_to};
use super::{ExtensionManager, PluginRecord, PluginRegistry};

pub(crate) enum PluginTarget {
    Bare(String),
    Exact(String),
}

impl PluginTarget {
    pub(crate) fn from_fields(name: String, selector: Option<String>) -> Self {
        match selector {
            Some(selector) => Self::Exact(selector),
            None => Self::Bare(name),
        }
    }
}

#[derive(Clone)]
pub(crate) enum PluginAccess {
    Scoped(VisibilityCtx),
    LocalOperator,
    Denied,
}

impl PluginAccess {
    pub(crate) fn global() -> Self {
        Self::Scoped(VisibilityCtx::default())
    }

    pub(crate) fn for_run() -> Self {
        crate::projects::current_project_root().map_or_else(Self::global, |root| {
            Self::Scoped(VisibilityCtx::from_project_root(Some(root)))
        })
    }

    pub(crate) fn allows(&self, scope: &ScopeKey) -> bool {
        match self {
            Self::Scoped(visibility) => visible_to(scope, visibility),
            Self::LocalOperator => true,
            Self::Denied => false,
        }
    }

    pub(crate) fn project_root(&self) -> Option<&std::path::Path> {
        match self {
            Self::Scoped(visibility) => visibility.project_root.as_deref(),
            Self::LocalOperator | Self::Denied => None,
        }
    }

    pub(crate) fn install_scope(
        &self,
        scope: crate::extension::PluginScope,
    ) -> Result<ScopeKey, PluginTargetError> {
        match scope {
            crate::extension::PluginScope::User if self.allows(&ScopeKey::Global) => {
                Ok(ScopeKey::Global)
            }
            crate::extension::PluginScope::User => Err(PluginTargetError::NotFound),
            _ => self
                .project_root()
                .map(ScopeKey::project)
                .ok_or(PluginTargetError::ProjectRequired),
        }
    }

    fn selects_project(&self) -> bool {
        self.project_root().is_some()
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PluginTargetError {
    #[error("Plugin not found")]
    NotFound,
    #[error("Multiple plugin instances match; specify a selector")]
    Ambiguous { selectors: Vec<String> },
    #[error("Plugin instance selector is ambiguous")]
    SelectorCollision,
    #[error("Select an authorized project for this install scope")]
    ProjectRequired,
    #[error(
        "'{0}' was installed by Claude Code (origin claude_cache). Enabling it runs code another tool installed, so the operator turns it on: `aleph plugin enable {0}`, or the Panel's plugin settings. Tell them that; you may still disable it."
    )]
    OperatorRequired(String),
}

fn resolve_target(
    registry: &PluginRegistry,
    target: &PluginTarget,
    access: &PluginAccess,
) -> Result<PluginInstanceKey, PluginTargetError> {
    match target {
        PluginTarget::Bare(id) => {
            let authorized = registry
                .plugin_instances(id)
                .filter(|row| access.allows(&row.scope_key));
            let mut candidates = authorized.collect::<Vec<_>>();
            if access.selects_project() && candidates.iter().any(is_project) {
                candidates.retain(is_project);
            }
            match candidates.as_slice() {
                [] => Err(PluginTargetError::NotFound),
                [row] => Ok(row.instance_key()),
                _ => {
                    let mut selectors: Vec<_> = candidates
                        .into_iter()
                        .map(|row| row.instance_key().selector())
                        .collect();
                    selectors.sort_unstable();
                    if selectors.windows(2).any(|pair| pair[0] == pair[1]) {
                        return Err(PluginTargetError::SelectorCollision);
                    }
                    Err(PluginTargetError::Ambiguous { selectors })
                }
            }
        }
        PluginTarget::Exact(selector) => {
            let mut candidates = registry
                .list_plugins()
                .into_iter()
                .filter(|row| access.allows(&row.scope_key))
                .map(PluginRecord::instance_key)
                .filter(|key| key.selector() == *selector);
            let instance = candidates.next().ok_or(PluginTargetError::NotFound)?;
            if candidates.next().is_some() {
                return Err(PluginTargetError::SelectorCollision);
            }
            Ok(instance)
        }
    }
}

fn is_project(record: &&PluginRecord) -> bool {
    matches!(record.scope_key, ScopeKey::Project(_))
}

impl ExtensionManager {
    pub(crate) async fn resolve_plugin_target(
        &self,
        target: &PluginTarget,
        access: &PluginAccess,
    ) -> Result<PluginInstanceKey, PluginTargetError> {
        let registry = self.plugin_registry.read().await;
        resolve_target(&registry, target, access)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::extension::{PluginKind, PluginOrigin, PluginStatus};

    #[test]
    fn project_shadows_bare_global_before_state_check() {
        let a = PathBuf::from("project-a");
        let b = PathBuf::from("project-b");
        let mut registry = PluginRegistry::new();
        let make = |scope: ScopeKey| {
            let mut record = PluginRecord::new(
                "shared".into(),
                "Shared".into(),
                PluginKind::Static,
                PluginOrigin::Global,
            );
            record.scope_key = scope;
            record
        };
        let global = make(ScopeKey::Global);
        let mut selected = make(ScopeKey::Project(a.clone()));
        selected.status = PluginStatus::Disabled;
        let foreign = make(ScopeKey::Project(b));
        let global_key = global.instance_key();
        let selected_key = selected.instance_key();
        let foreign_key = foreign.instance_key();
        for row in [global, selected, foreign] {
            registry.register_plugin(row);
        }
        let access = PluginAccess::Scoped(VisibilityCtx {
            project_root: Some(a),
        });
        let bare = PluginTarget::Bare("shared".into());
        let global = PluginTarget::Exact(global_key.selector());
        let foreign = PluginTarget::Exact(foreign_key.selector());
        assert_eq!(resolve_target(&registry, &bare, &access), Ok(selected_key));
        assert_eq!(resolve_target(&registry, &global, &access), Ok(global_key));
        assert_eq!(
            resolve_target(&registry, &foreign, &access),
            Err(PluginTargetError::NotFound)
        );
        assert_eq!(
            resolve_target(&registry, &foreign, &PluginAccess::global()),
            Err(PluginTargetError::NotFound)
        );
    }
}
