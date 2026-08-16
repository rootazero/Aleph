//! Plugin management command handlers
//!
//! Spec C policy: **`NoLock`**. All plugin operations target
//! `~/.aleph/plugins/` (extension dir) or read `~/.aleph/config.toml`
//! for marketplace metadata; none of them write to `~/.aleph/data/`.
//! Each handler enters via a marker `run_no_lock` call to satisfy
//! the reverse-regression check (Task 25).

use crate::cli::MarketplaceAction;
use alephcore::extension::marketplace::types::{MarketplaceConfig, MarketplaceSourceType};
use alephcore::Config;
use alephcore::PluginMarketplaceEntry;

/// Handle plugins list command
pub async fn handle_plugins_list() -> Result<(), Box<dyn std::error::Error>> {
    alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(()))?;
    use alephcore::extension::ExtensionManager;

    let manager = ExtensionManager::with_defaults().await?;

    // Load all plugins
    if let Err(e) = manager.load_all().await {
        eprintln!("Warning: Some plugins failed to load: {e}");
    }

    let plugins = manager.get_plugin_info().await;

    if plugins.is_empty() {
        println!("No plugins installed");
    } else {
        println!("Installed plugins:");
        println!(
            "{:<25} {:<12} {:<10} {:<40}",
            "NAME", "VERSION", "STATUS", "DESCRIPTION"
        );
        println!("{}", "-".repeat(90));
        for plugin in &plugins {
            let version = plugin.version.clone().unwrap_or_else(|| "-".to_string());
            // Surface the real runtime status (loaded/disabled/overridden/error)
            // instead of collapsing everything to enabled/disabled. Older
            // records without a status fall back to the enabled flag.
            let status = if plugin.status.is_empty() {
                if plugin.enabled {
                    "loaded"
                } else {
                    "disabled"
                }
            } else {
                plugin.status.as_str()
            };
            let description = plugin.description.clone().unwrap_or_default();
            // Truncate description if too long
            let description = if description.chars().count() > 38 {
                let truncated: String = description.chars().take(35).collect();
                format!("{truncated}...")
            } else {
                description
            };
            println!(
                "{:<25} {:<12} {:<10} {:<40}",
                plugin.name, version, status, description
            );
        }
        println!();
        println!("Total: {} plugin(s)", plugins.len());
    }
    Ok(())
}

/// Handle plugins install command
pub async fn handle_plugins_install(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::default_plugins_dir;
    use alephcore::extension::manifest::adapter::AdapterRegistry;

    println!("Installing plugin from {url}...");

    let plugins_dir = default_plugins_dir();

    // Ensure plugins directory exists
    if !tokio::fs::try_exists(&plugins_dir).await.unwrap_or(false) {
        tokio::fs::create_dir_all(&plugins_dir).await?;
    }

    let repo_name = url
        .split_once('?')
        .or_else(|| url.split_once('#'))
        .map_or(url, |(path, _)| path)
        .trim_end_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("plugin")
        .trim_end_matches(".git");
    // The repo name is derived from external (URL) input — reject anything that
    // could escape the plugins dir or otherwise resolve outside it.
    if repo_name.is_empty()
        || repo_name == "."
        || repo_name == ".."
        || repo_name.contains('/')
        || repo_name.contains('\\')
    {
        eprintln!("Error: cannot derive a safe plugin directory name from URL: {url}");
        std::process::exit(1);
    }
    let dest_path = plugins_dir.join(repo_name);

    if let Err(reason) =
        alephcore::extension::ensure_plugin_destination_is_safe(&plugins_dir, &dest_path)
    {
        eprintln!("Error: {reason}");
        std::process::exit(1);
    }

    if tokio::fs::try_exists(&dest_path).await.unwrap_or(false) {
        eprintln!("Error: Plugin already exists at: {}", dest_path.display());
        std::process::exit(1);
    }

    // Clone the repository off the async executor; git2 is synchronous.
    println!("Cloning repository...");
    let url = url.to_string();
    let dest_path_clone = dest_path.clone();
    let clone_result =
        tokio::task::spawn_blocking(move || git2::Repository::clone(&url, &dest_path_clone))
            .await?;
    match clone_result {
        Ok(_) => {
            if let Err(reason) = alephcore::extension::ensure_plugin_root_within_authoritative(
                &plugins_dir,
                &dest_path,
            ) {
                eprintln!("Error: {reason}");
                let _ = std::fs::remove_dir_all(&dest_path);
                std::process::exit(1);
            }
            println!("Repository cloned successfully.");

            // Validate the installed plugin via AdapterRegistry
            let registry = AdapterRegistry::with_defaults();
            match registry.parse_dir(&dest_path) {
                Ok(output) => {
                    let name = output.name.unwrap_or_else(|| output.plugin_id.clone());
                    let version = output.version.unwrap_or_else(|| "-".to_string());
                    let description = output.description.unwrap_or_else(|| "-".to_string());
                    let capabilities_count = output.capabilities.len();
                    println!();
                    println!("Plugin installed successfully!");
                    println!("  Name:        {name}");
                    println!("  Version:     {version}");
                    println!("  Description: {description}");
                    println!("  Path:        {}", dest_path.display());
                    println!("  Capabilities: {capabilities_count}");
                }
                Err(e) => {
                    // Cleanup on failure
                    eprintln!("Warning: Plugin cloned but failed to load: {e}");
                    eprintln!(
                        "The plugin directory has been kept at: {}",
                        dest_path.display()
                    );
                    eprintln!("You may need to check the plugin's manifest file.");
                }
            }
        }
        Err(e) => {
            eprintln!("Error: Failed to clone repository: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Handle plugins uninstall command
pub async fn handle_plugins_uninstall(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::default_plugins_dir;

    let plugins_dir = default_plugins_dir();
    let plugin_path = plugins_dir.join(name);

    if !tokio::fs::try_exists(&plugin_path).await.unwrap_or(false) {
        eprintln!("Error: Plugin not found: {name}");
        eprintln!("Plugin directory: {}", plugin_path.display());
        std::process::exit(1);
    }

    // Confirm uninstall
    println!("Uninstalling plugin: {name}");
    println!("Path: {}", plugin_path.display());

    match tokio::fs::remove_dir_all(&plugin_path).await {
        Ok(()) => {
            alephcore::extension::plugin_state::forget_plugin_sidecars(name).await;
            println!("Plugin uninstalled successfully.");
        }
        Err(e) => {
            eprintln!("Error: Failed to remove plugin: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Handle plugins enable command
pub async fn handle_plugins_enable(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    set_enabled_locally(name, true).await
}

/// Handle plugins disable command
pub async fn handle_plugins_disable(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    set_enabled_locally(name, false).await
}

/// Write the operator's activation preference for a locally-installed plugin.
///
/// This used to create / delete a `<plugin_dir>/.disabled` marker. That marker
/// had four write sites and **zero readers** — neither `has_plugin_manifest`
/// nor `scan_plugin_parent` consulted it — so `aleph plugin disable` printed
/// success and changed nothing that outlived the process. The durable answer
/// now lives in `<data_dir>/plugins.toml`; see
/// `alephcore::extension::plugin_state`.
///
/// No-lock path (Spec C): this only touches the plugin config document, and
/// [`PluginsConfig::save`] is an atomic temp+rename. A running daemon re-reads
/// the file on its next `load_all`.
async fn set_enabled_locally(name: &str, enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::default_plugins_dir;
    use alephcore::extension::plugin_state::PluginsConfig;

    alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(()))?;

    let plugin_path = default_plugins_dir().join(name);
    if !tokio::fs::try_exists(&plugin_path).await.unwrap_or(false) {
        eprintln!("Error: Plugin not found: {name}");
        std::process::exit(1);
    }

    let path = PluginsConfig::default_path()?;
    let mut config = PluginsConfig::load(&path);
    let verb = if enabled { "enabled" } else { "disabled" };
    if config.set_enabled(name, enabled) {
        config.save(&path).await?;
        println!("Plugin {verb}: {name}");
    } else {
        println!("Plugin is already {verb}: {name}");
    }

    // A stale marker from an older build would be migrated (and removed) by the
    // daemon's next load; say so rather than leaving the operator to wonder why
    // a file they can see is being ignored.
    if tokio::fs::try_exists(plugin_path.join(".disabled"))
        .await
        .unwrap_or(false)
    {
        println!(
            "  note: a legacy .disabled marker is still present; \
             it is ignored and will be removed on the next server start."
        );
    }

    Ok(())
}

/// Load marketplace entries from `config.toml` into the manager type.
fn load_marketplace_configs(
) -> Result<std::collections::HashMap<String, MarketplaceConfig>, Box<dyn std::error::Error>> {
    let config = Config::load()?;
    Ok(config
        .plugin_marketplaces
        .iter()
        .map(|(name, entry): (&String, &PluginMarketplaceEntry)| {
            let source_type = match entry.source_type.as_str() {
                "local" => MarketplaceSourceType::Local,
                _ => MarketplaceSourceType::Github,
            };
            (
                name.clone(),
                MarketplaceConfig {
                    source: entry.source.clone(),
                    source_type,
                },
            )
        })
        .collect())
}

/// Install plugin from marketplace (by name) or URL (legacy path)
pub async fn handle_plugin_install(
    source: &str,
    scope: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::marketplace::MarketplaceManager;
    use alephcore::extension::scope::parse_scope;

    // If source looks like a plugin name (no /, ., or :), use marketplace install locally
    let is_plugin_name = !source.contains('/') && !source.contains('.') && !source.contains(':');

    if is_plugin_name {
        let marketplace_configs = load_marketplace_configs()?;

        let manager = MarketplaceManager::new(marketplace_configs, None);
        let plugin_scope = parse_scope(scope)?;

        println!("Searching for plugin '{source}'...");
        match manager.install_to_scope(source, None, plugin_scope, None) {
            Ok(path) => {
                println!("Plugin '{source}' installed to {path:?}");
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        // Direct URL install (legacy path)
        handle_plugins_install(source).await?;
    }
    Ok(())
}

/// Build a [`MarketplaceManager`] from the marketplace entries in `config.toml`.
fn build_marketplace_manager(
) -> Result<alephcore::extension::marketplace::MarketplaceManager, Box<dyn std::error::Error>> {
    use alephcore::extension::marketplace::MarketplaceManager;

    let marketplace_configs = load_marketplace_configs()?;
    Ok(MarketplaceManager::new(marketplace_configs, None))
}

/// Update an installed plugin (or all installed plugins) to the latest
/// marketplace version. Skips plugins whose version is already current unless
/// `force` is set.
pub async fn handle_plugin_update(
    name: Option<String>,
    force: bool,
    scope: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::marketplace::UpdateOutcome;
    use alephcore::extension::scope::{parse_scope, scope_install_dir};

    alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(()))?;

    let manager = build_marketplace_manager()?;
    let plugin_scope = parse_scope(scope)?;

    // Resolve the set of plugin names to update.
    let names: Vec<String> = if let Some(n) = name {
        vec![n]
    } else {
        let dir = scope_install_dir(plugin_scope, None)?;
        let mut found = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await.is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if let Some(n) = entry.file_name().to_str() {
                // Skip staging/backup scratch dirs.
                if n.starts_with('.') {
                    continue;
                }
                found.push(n.to_string());
            }
        }
        if found.is_empty() {
            println!("No installed plugins to update in scope '{scope}'.");
            return Ok(());
        }
        found.sort();
        found
    };

    let mut updated = 0usize;
    let mut unchanged = 0usize;
    let mut failed = 0usize;
    for plugin_name in &names {
        match manager.update_to_scope(plugin_name, None, plugin_scope, None, force) {
            Ok(UpdateOutcome::Updated { from, to }) => {
                let from = from.unwrap_or_else(|| "-".to_string());
                let to = to.unwrap_or_else(|| "-".to_string());
                println!("Updated '{plugin_name}': {from} → {to}");
                updated += 1;
            }
            Ok(UpdateOutcome::AlreadyLatest { version }) => {
                let version = version.unwrap_or_else(|| "-".to_string());
                println!("'{plugin_name}' already up to date ({version}).");
                unchanged += 1;
            }
            Err(e) => {
                eprintln!("Failed to update '{plugin_name}': {e}");
                failed += 1;
            }
        }
    }

    println!("\nDone: {updated} updated, {unchanged} unchanged, {failed} failed.");
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Handle marketplace subcommands
pub async fn handle_marketplace_command(
    action: MarketplaceAction,
) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::marketplace::types::MarketplaceSourceType;
    use alephcore::extension::marketplace::MarketplaceManager;

    // Load marketplace config from config.toml
    let marketplace_configs = load_marketplace_configs()?;

    let mut manager = MarketplaceManager::new(marketplace_configs, None);

    match action {
        MarketplaceAction::List => {
            let marketplaces = manager.list();
            println!("Registered marketplaces:");
            println!("{:<25} {:<15} SOURCE", "NAME", "TYPE");
            println!("{}", "-".repeat(70));
            for (name, cfg) in &marketplaces {
                let type_str = match cfg.source_type {
                    MarketplaceSourceType::Github => "github",
                    MarketplaceSourceType::Local => "local",
                };
                println!("{:<25} {:<15} {}", name, type_str, cfg.source);
            }
        }
        MarketplaceAction::Add { source } => {
            // Derive name and type from source
            let (name, source_type) = if source.contains('/') && !source.starts_with('/') {
                // GitHub: "owner/repo" -> name from repo part
                let repo = source
                    .split('/')
                    .next_back()
                    .unwrap_or(&source)
                    .to_lowercase();
                (repo, MarketplaceSourceType::Github)
            } else {
                // Local path
                let name = std::path::Path::new(&source)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("local")
                    .to_string();
                (name, MarketplaceSourceType::Local)
            };

            manager.add(
                name.clone(),
                MarketplaceConfig {
                    source: source.clone(),
                    source_type: source_type.clone(),
                },
            );

            // Save to config
            let mut config = Config::load()?;
            config.plugin_marketplaces.insert(
                name.clone(),
                PluginMarketplaceEntry {
                    source: source.clone(),
                    source_type: match source_type {
                        MarketplaceSourceType::Github => "github".to_string(),
                        MarketplaceSourceType::Local => "local".to_string(),
                    },
                },
            );
            config.save_incremental(&["plugin_marketplaces"])?;

            println!("Added marketplace '{name}' ({source})");

            // Sync cache
            print!("Syncing cache...");
            match manager.update(&name) {
                Ok(_) => println!(" done."),
                Err(e) => println!(" failed: {e}"),
            }
        }
        MarketplaceAction::Update { name } => {
            if let Some(n) = name {
                println!("Updating marketplace '{n}'...");
                match manager.update(&n) {
                    Ok(path) => println!("Updated: {path:?}"),
                    Err(e) => eprintln!("Error: {e}"),
                }
            } else {
                println!("Updating all marketplaces...");
                match manager.update_all() {
                    Ok(()) => println!("All marketplaces updated."),
                    Err(e) => eprintln!("Some updates failed: {e}"),
                }
            }
        }
        MarketplaceAction::Remove { name } => {
            manager.remove(&name)?;

            // Save to config
            let mut config = Config::load()?;
            config.plugin_marketplaces.remove(&name);
            config.save_incremental(&["plugin_marketplaces"])?;

            println!("Removed marketplace '{name}'");
        }
    }
    Ok(())
}
