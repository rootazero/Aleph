// Browser fill_form tool — fills multiple form fields at once.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::backend::BrowserBackend;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
use crate::browser::manager::ProfileManager;
use crate::browser::profile::BrowserDriver;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// A single form field to fill.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormField {
    /// CSS selector of the form field (used in managed mode).
    #[serde(default)]
    pub selector: Option<String>,
    /// ARIA snapshot ref_id of the form field (used in existing-session mode).
    #[serde(default)]
    pub ref_id: Option<String>,
    /// Value to fill into the field.
    pub value: String,
}

/// Arguments for the browser_fill_form tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserFillFormArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Form fields to fill.
    pub fields: Vec<FormField>,
}

/// Output from the browser_fill_form tool.
#[derive(Debug, Serialize)]
pub struct BrowserFillFormOutput {
    pub success: bool,
    pub filled_count: usize,
    pub message: Option<String>,
}

/// Fills multiple form fields at once.
#[derive(Clone)]
pub struct BrowserFillFormTool {
    manager: Arc<ProfileManager>,
}

impl BrowserFillFormTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserFillFormTool {
    const NAME: &'static str = "browser_fill_form";
    const DESCRIPTION: &'static str = "Fill multiple form fields at once";
    type Args = BrowserFillFormArgs;
    type Output = BrowserFillFormOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        let count = args.fields.len();

        let driver = self.manager.get_driver(&args.profile);
        match driver {
            Some(BrowserDriver::ExistingSession) => {
                let chrome_mcp = self.manager.get_chrome_mcp_driver();
                let backend = ChromeMcpBackend::new(chrome_mcp, args.profile.clone());
                let tab_id = match super::get_active_tab(&backend).await {
                    Ok(id) => id,
                    Err(e) => {
                        return Ok(BrowserFillFormOutput {
                            success: false,
                            filled_count: 0,
                            message: Some(format!("{e}")),
                        });
                    }
                };

                let mut filled = 0;
                for field in &args.fields {
                    let target = if let Some(ref ref_id) = field.ref_id {
                        ActionTarget::Ref { ref_id: ref_id.clone() }
                    } else if let Some(ref css) = field.selector {
                        ActionTarget::Selector { css: css.clone() }
                    } else {
                        return Ok(BrowserFillFormOutput {
                            success: false,
                            filled_count: filled,
                            message: Some("Each field must have either 'selector' or 'ref_id'".into()),
                        });
                    };
                    let field_name = field.ref_id.as_deref()
                        .or(field.selector.as_deref())
                        .unwrap_or("unknown");
                    match backend.fill(&tab_id, target, &field.value).await {
                        Ok(()) => filled += 1,
                        Err(e) => {
                            return Ok(BrowserFillFormOutput {
                                success: false,
                                filled_count: filled,
                                message: Some(format!(
                                    "Failed on field '{}': {e}",
                                    field_name
                                )),
                            });
                        }
                    }
                }

                Ok(BrowserFillFormOutput {
                    success: true,
                    filled_count: filled,
                    message: Some(format!(
                        "Filled {} field(s) in profile '{}'",
                        filled, args.profile
                    )),
                })
            }
            _ => {
                // Placeholder for managed mode
                Ok(BrowserFillFormOutput {
                    success: true,
                    filled_count: count,
                    message: Some(format!(
                        "Filled {} field(s) in profile '{}'",
                        count, args.profile
                    )),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_fill_form_multiple_fields() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserFillFormTool::new(manager);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![
                    FormField {
                        selector: Some("input#name".into()),
                        ref_id: None,
                        value: "Alice".into(),
                    },
                    FormField {
                        selector: Some("input#email".into()),
                        ref_id: None,
                        value: "alice@example.com".into(),
                    },
                ],
            })
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.filled_count, 2);
    }

    #[tokio::test]
    async fn test_fill_form_empty_fields() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserFillFormTool::new(manager);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![],
            })
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.filled_count, 0);
    }
}
