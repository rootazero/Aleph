//! Permission conversion from TOML sections to domain types

use super::types::{FilesystemPermission, PermissionsSection};
use crate::extension::manifest::types::PluginPermission;

/// Convert TOML permissions section to PluginPermission list
pub fn convert_permissions(perms: &PermissionsSection) -> Vec<PluginPermission> {
    let mut permissions = Vec::new();

    if perms.network {
        permissions.push(PluginPermission::Network);
    }

    match &perms.filesystem {
        FilesystemPermission::Bool(true) => {
            permissions.push(PluginPermission::Filesystem);
        }
        FilesystemPermission::Bool(false) => {}
        FilesystemPermission::Level(level) => match level.as_str() {
            "read" => permissions.push(PluginPermission::FilesystemRead),
            "write" => permissions.push(PluginPermission::FilesystemWrite),
            "full" => permissions.push(PluginPermission::Filesystem),
            _ => {}
        },
    }

    if perms.env {
        permissions.push(PluginPermission::Env);
    }

    if perms.shell {
        permissions.push(PluginPermission::Custom("shell".to_string()));
    }

    permissions
}
