//! RSC (Resource-Specific Consent) Permissions for Microsoft Teams
//!
//! RSC allows bot apps to access Teams data with granular permissions
//! without requiring user consent for each operation.

use crate::gateway::channel::ChannelError;
use crate::gateway::interfaces::msteams::graph::GraphClient;
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};

/// RSC permissions for Teams
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RscPermissions {
    #[serde(rename = "ChannelMessage.Read")]
    pub channel_message_read: bool,
    #[serde(rename = "ChannelMessage.Edit")]
    pub channel_message_edit: bool,
    #[serde(rename = "ChannelMessage.Delete")]
    pub channel_message_delete: bool,
    #[serde(rename = "Member.Read")]
    pub member_access: bool,
    #[serde(rename = "FileAccess")]
    pub file_access: bool,
}

impl RscPermissions {
    /// Create permissions from individual flags
    pub fn new(
        channel_message_read: bool,
        channel_message_edit: bool,
        channel_message_delete: bool,
        member_access: bool,
        file_access: bool,
    ) -> Self {
        Self {
            channel_message_read,
            channel_message_edit,
            channel_message_delete,
            member_access,
            file_access,
        }
    }

    /// Check if any permission is enabled
    pub fn has_any(&self) -> bool {
        self.channel_message_read
            || self.channel_message_edit
            || self.channel_message_delete
            || self.member_access
            || self.file_access
    }
}

/// RSC Permission Manager
pub struct RscPermissionManager {
    graph_client: Arc<GraphClient>,
    permissions: RscPermissions,
}

impl RscPermissionManager {
    /// Create a new RscPermissionManager
    pub fn new(graph_client: Arc<GraphClient>, permissions: RscPermissions) -> Self {
        Self {
            graph_client,
            permissions,
        }
    }

    /// Declare permissions to Graph API
    /// POST /teams/{teamId}/appPermissions
    pub async fn declare_permissions(&self, team_id: &str) -> Result<(), ChannelError> {
        if !self.permissions.has_any() {
            return Ok(());
        }

        let resource_access = self.build_resource_access_list();

        let payload = serde_json::json!({
            "requiredResourceSpecificPermissions": resource_access
        });

        let url = format!("/teams/{}/appPermissions", team_id);
        let _resp = self.graph_client.post(&url, payload).await?;

        Ok(())
    }

    /// Build the resource access list for the permissions payload
    fn build_resource_access_list(&self) -> Vec<serde_json::Value> {
        let mut access = Vec::new();

        if self.permissions.channel_message_read {
            access.push(serde_json::json!({
                "resource": "ChannelMessage.Read.Group",
                "delegated": ["ChannelMessage.Read.Group"],
                "application": true
            }));
            access.push(serde_json::json!({
                "resource": "ChannelMessage.Read",
                "delegated": ["ChannelMessage.Read"],
                "application": true
            }));
        }

        if self.permissions.channel_message_edit {
            access.push(serde_json::json!({
                "resource": "ChannelMessage.Edit",
                "delegated": ["ChannelMessage.Edit"],
                "application": true
            }));
        }

        if self.permissions.channel_message_delete {
            access.push(serde_json::json!({
                "resource": "ChannelMessage.Delete",
                "delegated": ["ChannelMessage.Delete"],
                "application": true
            }));
        }

        if self.permissions.member_access {
            access.push(serde_json::json!({
                "resource": "Member.Read",
                "delegated": ["Member.Read"],
                "application": true
            }));
        }

        if self.permissions.file_access {
            access.push(serde_json::json!({
                "resource": "FileAccess",
                "delegated": ["FileAccess"],
                "application": true
            }));
        }

        access
    }

    /// Get the configured permissions
    pub fn permissions(&self) -> &RscPermissions {
        &self.permissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rsc_permissions_default() {
        let perms = RscPermissions::default();
        assert!(!perms.has_any());
    }

    #[test]
    fn test_rsc_permissions_has_any() {
        let perms = RscPermissions::new(true, false, false, false, false);
        assert!(perms.has_any());
    }

    #[test]
    fn test_rsc_permissions_all_false_by_default() {
        let perms = RscPermissions::default();
        assert!(!perms.channel_message_read);
        assert!(!perms.channel_message_edit);
        assert!(!perms.channel_message_delete);
        assert!(!perms.member_access);
        assert!(!perms.file_access);
    }

    #[test]
    fn test_rsc_permissions_new() {
        let perms = RscPermissions::new(true, true, true, true, true);
        assert!(perms.channel_message_read);
        assert!(perms.channel_message_edit);
        assert!(perms.channel_message_delete);
        assert!(perms.member_access);
        assert!(perms.file_access);
        assert!(perms.has_any());
    }

    #[test]
    fn test_build_resource_access_list_read() {
        let perms = RscPermissions::new(true, false, false, false, false);
        let manager = RscPermissionManager::new(
            Arc::new(GraphClient::new(Arc::new(
                crate::gateway::interfaces::msteams::graph::GraphTokenCache::new(
                    "app".to_string(),
                    "secret".to_string(),
                    "tenant".to_string(),
                ),
            ))),
            perms,
        );

        let access = manager.build_resource_access_list();
        assert_eq!(access.len(), 2); // ChannelMessage.Read.Group and ChannelMessage.Read
    }

    #[test]
    fn test_build_resource_access_list_all() {
        let perms = RscPermissions::new(true, true, true, true, true);
        let manager = RscPermissionManager::new(
            Arc::new(GraphClient::new(Arc::new(
                crate::gateway::interfaces::msteams::graph::GraphTokenCache::new(
                    "app".to_string(),
                    "secret".to_string(),
                    "tenant".to_string(),
                ),
            ))),
            perms,
        );

        let access = manager.build_resource_access_list();
        // 2 for read + 1 for edit + 1 for delete + 1 for member + 1 for file = 6
        assert_eq!(access.len(), 6);
    }

    #[test]
    fn test_build_resource_access_list_empty() {
        let perms = RscPermissions::default();
        let manager = RscPermissionManager::new(
            Arc::new(GraphClient::new(Arc::new(
                crate::gateway::interfaces::msteams::graph::GraphTokenCache::new(
                    "app".to_string(),
                    "secret".to_string(),
                    "tenant".to_string(),
                ),
            ))),
            perms,
        );

        let access = manager.build_resource_access_list();
        assert!(access.is_empty());
    }
}
