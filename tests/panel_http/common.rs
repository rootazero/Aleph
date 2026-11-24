//! Public-API fixtures shared by HTTP acceptance and the opt-in live browser test.
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use alephcore::discovery::DiscoveryConfig;
use alephcore::extension::{ExtensionConfig, ExtensionManager};
use base64::Engine;
use sha2::{Digest, Sha256};

pub struct Home {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl Home {
    // The integration binary has one environment-owning acceptance test. The
    // ignored browser fixture runs alone in a separate process, never in parallel.
    pub fn set(home: &Path, root: &Path) -> Self {
        let previous = ["ALEPH_HOME", "HOME"]
            .map(|key| (key, std::env::var_os(key)))
            .to_vec();
        std::env::set_var("ALEPH_HOME", home);
        std::env::set_var("HOME", root);
        Self { previous }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

pub fn write_panel(home: &Path, version: &str, bytes: &[u8]) {
    let root = home.join("plugins/community-live");
    std::fs::create_dir_all(root.join("ui")).unwrap();
    std::fs::write(root.join("ui/panel.mjs"), bytes).unwrap();
    std::fs::write(root.join("generic.mjs"), b"generic is not a Panel").unwrap();
    std::fs::write(root.join("aleph.plugin.toml"), format!(
        "[plugin]\nid = \"community-live\"\nname = \"Community\"\nversion = \"{version}\"\nkind = \"static\"\nentry = \"generic.mjs\"\n\n[panel]\ntab_id = \"overview\"\nlabel = \"Overview\"\nicon = \"layout\"\norder = 0\nentry = \"ui/panel.mjs\"\napi_major = 1\npermissions = []\nasset_integrity = \"sha256-{}\"\n",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes))
    )).unwrap();
}

pub async fn manager(root: &Path) -> Arc<ExtensionManager> {
    std::fs::create_dir_all(root.join("project")).unwrap();
    let projects = alephcore::projects::ProjectStore::shared();
    projects.create_schema().unwrap();
    projects.add_for(&root.join("project"), None, None).unwrap();
    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: root.to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        },
        plugins_config_path: Some(root.join("plugins.toml")),
    })
    .await
    .unwrap();
    manager.load_all().await.unwrap();
    Arc::new(manager)
}
