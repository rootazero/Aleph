//! YAML Policy Loader

use crate::daemon::dispatcher::yaml_policy::{YamlRule, YamlPolicy};
use crate::daemon::dispatcher::policy::Policy;
use crate::daemon::worldmodel::WorldModel;
use crate::daemon::error::{DaemonError, Result};
use std::path::Path;
use std::sync::Arc;
use std::fs;

/// Load YAML policies from file
pub fn load_yaml_policies(
    path: impl AsRef<Path>,
    worldmodel: Arc<WorldModel>,
) -> Result<Vec<Box<dyn Policy>>> {
    let path = path.as_ref();

    if !path.exists() {
        log::info!("No YAML policy file at {:?}, skipping", path);
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path)
        .map_err(|e| DaemonError::Config(format!("Failed to read {:?}: {}", path, e)))?;

    let rules: Vec<YamlRule> = serde_yaml::from_str(&content)
        .map_err(|e| DaemonError::Config(format!("Failed to parse YAML: {}", e)))?;

    log::info!("Loaded {} YAML policies from {:?}", rules.len(), path);

    let policies: Vec<Box<dyn Policy>> = rules
        .into_iter()
        .map(|rule| Box::new(YamlPolicy::new(rule, worldmodel.clone())) as Box<dyn Policy>)
        .collect();

    Ok(policies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::WorldModelConfig;
    use crate::daemon::event_bus::DaemonEventBus;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_load_yaml_policies_success() {
        let yaml = r#"
- name: "Rule 1"
  enabled: true
  trigger:
    event: activity_changed
  action:
    type: notify
  risk: low

- name: "Rule 2"
  enabled: true
  trigger:
    event: idle_state_changed
  action:
    type: mute_system_audio
  risk: low
"#;
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml.as_bytes()).unwrap();

        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let policies = load_yaml_policies(temp_file.path(), worldmodel).unwrap();
        assert_eq!(policies.len(), 2);
        assert_eq!(policies[0].name(), "Rule 1");
        assert_eq!(policies[1].name(), "Rule 2");
    }

    #[tokio::test]
    async fn test_load_yaml_policies_missing_file() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let policies = load_yaml_policies("/nonexistent/path.yaml", worldmodel).unwrap();
        assert_eq!(policies.len(), 0);
    }

    #[tokio::test]
    async fn test_load_yaml_policies_invalid_yaml() {
        let yaml = "invalid: yaml: syntax: error:";
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml.as_bytes()).unwrap();

        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let result = load_yaml_policies(temp_file.path(), worldmodel);
        assert!(result.is_err());
    }
}
