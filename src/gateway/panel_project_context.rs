// Server-owned Panel selection. ProjectStore and SecurityStore remain authoritative.

use std::collections::HashMap;

use aleph_protocol::panel::PanelProjectSelection;

use crate::extension::visibility::{canonical_root, VisibilityCtx};
use crate::gateway::security::store::{DeviceRow, SecurityStore, UserStatus};
use crate::gateway::security::PANEL_DEVICE_TYPE;
use crate::projects::{authz, ProjectStatus, ProjectStore};
use crate::sync_primitives::{Arc, Mutex};

/// Only authenticated ingress constructs this; strings from RPC params are not principals.
#[derive(Clone, Debug, Default)]
pub(crate) struct PanelCallerContext {
    verified_user: Option<String>,
    device_id: Option<String>,
}

impl PanelCallerContext {
    pub(crate) fn authenticated(verified_user: Option<String>, device_id: Option<String>) -> Self {
        Self {
            verified_user,
            device_id,
        }
    }
}

/// Explicit dispatch state; it cannot be populated from JSON-RPC params.
#[derive(Clone)]
pub(crate) struct PanelRequestContext {
    pub(crate) caller: PanelCallerContext,
    pub(crate) selector: Arc<PanelProjectSelector>,
}

pub(crate) struct PanelResolvedContext {
    pub(crate) selection: PanelProjectSelection,
    pub(crate) visibility: VisibilityCtx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelProjectContextError;

#[derive(Clone)]
struct DeviceSelection {
    // Binding provenance prevents a re-paired device inheriting another user's selection.
    user: String,
    selection: PanelProjectSelection,
}

pub(crate) struct PanelProjectSelector {
    projects: Arc<ProjectStore>,
    security: Arc<SecurityStore>,
    selections: Mutex<HashMap<String, DeviceSelection>>,
}

impl PanelProjectSelector {
    pub(crate) fn new(projects: Arc<ProjectStore>, security: Arc<SecurityStore>) -> Self {
        Self {
            projects,
            security,
            selections: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn select(
        &self,
        caller: &PanelCallerContext,
        project_id: Option<String>,
    ) -> Result<PanelProjectSelection, PanelProjectContextError> {
        let (device, user) = self.principal(caller)?;
        if let Some(id) = project_id.as_deref() {
            self.project_visibility(id, user)?;
        }
        let mut selections = self
            .selections
            .lock()
            .map_err(|_| PanelProjectContextError)?;
        let generation = selections
            .get(device)
            .map_or(0, |entry| entry.selection.generation)
            .checked_add(1)
            .ok_or(PanelProjectContextError)?;
        let selection = PanelProjectSelection {
            project_id,
            generation,
        };
        selections.insert(
            device.to_string(),
            DeviceSelection {
                user: user.to_string(),
                selection: selection.clone(),
            },
        );
        Ok(selection)
    }

    pub(crate) fn resolve(
        &self,
        caller: &PanelCallerContext,
    ) -> Result<PanelResolvedContext, PanelProjectContextError> {
        // A paired identity remains current even before its first project selection.
        let principal = if caller.verified_user.is_some() {
            Some(self.principal(caller)?)
        } else {
            if let Some(device) = caller.device_id.as_deref() {
                // Compatibility is only for a currently unbound, active device.
                if self.device(device)?.user_id.is_some() {
                    return Err(PanelProjectContextError);
                }
            }
            None
        };
        let selected = caller
            .device_id
            .as_deref()
            .map(|device| {
                self.selections
                    .lock()
                    .map(|map| map.get(device).cloned())
                    .map_err(|_| PanelProjectContextError)
            })
            .transpose()?
            .flatten();
        let Some(selected) = selected else {
            return Ok(PanelResolvedContext {
                selection: PanelProjectSelection::default(),
                visibility: VisibilityCtx::default(),
            });
        };
        let (_, user) = principal.ok_or(PanelProjectContextError)?;
        if selected.user != user {
            return Err(PanelProjectContextError);
        }
        let visibility = match selected.selection.project_id.as_deref() {
            Some(id) => self.project_visibility(id, user)?,
            None => VisibilityCtx::default(),
        };
        Ok(PanelResolvedContext {
            selection: selected.selection,
            visibility,
        })
    }

    /// Re-read both authorities on each use; a prior handshake is not a durable grant.
    fn principal<'a>(
        &self,
        caller: &'a PanelCallerContext,
    ) -> Result<(&'a str, &'a str), PanelProjectContextError> {
        let device_id = caller
            .device_id
            .as_deref()
            .ok_or(PanelProjectContextError)?;
        let user_id = caller
            .verified_user
            .as_deref()
            .ok_or(PanelProjectContextError)?;
        let device = self.device(device_id)?;
        if device.user_id.as_deref() != Some(user_id) {
            return Err(PanelProjectContextError);
        }
        let user = self
            .security
            .get_user(user_id)
            .ok()
            .flatten()
            .ok_or(PanelProjectContextError)?;
        if user.status != UserStatus::Active {
            return Err(PanelProjectContextError);
        }
        Ok((device_id, user_id))
    }

    fn device(&self, id: &str) -> Result<DeviceRow, PanelProjectContextError> {
        let device = self
            .security
            .get_device(id)
            .ok()
            .flatten()
            .ok_or(PanelProjectContextError)?;
        if device.revoked_at.is_some() || device.device_type.as_deref() != Some(PANEL_DEVICE_TYPE) {
            return Err(PanelProjectContextError);
        }
        Ok(device)
    }

    fn project_visibility(
        &self,
        id: &str,
        user: &str,
    ) -> Result<VisibilityCtx, PanelProjectContextError> {
        let project =
            authz::project_for(&self.projects, id, Some(user)).ok_or(PanelProjectContextError)?;
        if project.status != ProjectStatus::Active {
            return Err(PanelProjectContextError);
        }
        let workspace = project.workspace_path.ok_or(PanelProjectContextError)?;
        Ok(VisibilityCtx {
            project_root: Some(canonical_root(&workspace)),
        })
    }
}
