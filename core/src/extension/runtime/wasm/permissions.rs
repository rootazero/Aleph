//! WASM plugin permission checking

use std::collections::HashSet;
use crate::extension::manifest::PluginPermission;

/// Permission checker for WASM plugins
#[derive(Debug, Clone, Default)]
pub struct PermissionChecker {
    allowed: HashSet<PluginPermission>,
}

impl PermissionChecker {
    /// Create a new permission checker with the given permissions
    pub fn new(permissions: Vec<PluginPermission>) -> Self {
        Self {
            allowed: permissions.into_iter().collect(),
        }
    }

    /// Check if network access is allowed
    pub fn can_network(&self) -> bool {
        self.allowed.contains(&PluginPermission::Network)
    }

    /// Check if filesystem read is allowed
    pub fn can_read_filesystem(&self) -> bool {
        self.allowed.contains(&PluginPermission::FilesystemRead)
            || self.allowed.contains(&PluginPermission::Filesystem)
    }

    /// Check if filesystem write is allowed
    pub fn can_write_filesystem(&self) -> bool {
        self.allowed.contains(&PluginPermission::FilesystemWrite)
            || self.allowed.contains(&PluginPermission::Filesystem)
    }

    /// Check if environment access is allowed
    pub fn can_access_env(&self) -> bool {
        self.allowed.contains(&PluginPermission::Env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_permissions() {
        let checker = PermissionChecker::new(vec![]);
        assert!(!checker.can_network());
        assert!(!checker.can_read_filesystem());
        assert!(!checker.can_write_filesystem());
    }

    #[test]
    fn test_network_permission() {
        let checker = PermissionChecker::new(vec![PluginPermission::Network]);
        assert!(checker.can_network());
        assert!(!checker.can_read_filesystem());
    }

    #[test]
    fn test_filesystem_permission() {
        let checker = PermissionChecker::new(vec![PluginPermission::Filesystem]);
        assert!(checker.can_read_filesystem());
        assert!(checker.can_write_filesystem());
    }

    #[test]
    fn test_filesystem_read_only() {
        let checker = PermissionChecker::new(vec![PluginPermission::FilesystemRead]);
        assert!(checker.can_read_filesystem());
        assert!(!checker.can_write_filesystem());
    }

    #[test]
    fn test_filesystem_write_only() {
        let checker = PermissionChecker::new(vec![PluginPermission::FilesystemWrite]);
        assert!(!checker.can_read_filesystem());
        assert!(checker.can_write_filesystem());
    }

    #[test]
    fn test_env_permission() {
        let checker = PermissionChecker::new(vec![PluginPermission::Env]);
        assert!(checker.can_access_env());
        assert!(!checker.can_network());
    }

    #[test]
    fn test_multiple_permissions() {
        let checker = PermissionChecker::new(vec![
            PluginPermission::Network,
            PluginPermission::FilesystemRead,
            PluginPermission::Env,
        ]);
        assert!(checker.can_network());
        assert!(checker.can_read_filesystem());
        assert!(!checker.can_write_filesystem());
        assert!(checker.can_access_env());
    }
}
