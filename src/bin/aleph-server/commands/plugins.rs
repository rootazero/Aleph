//! Plugin management command handlers

use crate::cli::MarketplaceAction;

/// Handle plugins list command
pub async fn handle_plugins_list() -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::ExtensionManager;

    let manager = ExtensionManager::with_defaults().await?;

    // Load all plugins
    if let Err(e) = manager.load_all().await {
        eprintln!("Warning: Some plugins failed to load: {}", e);
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
            let status = if plugin.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let description = plugin.description.clone().unwrap_or_default();
            // Truncate description if too long
            let description = if description.chars().count() > 38 {
                let truncated: String = description.chars().take(35).collect();
                format!("{}...", truncated)
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

    println!("Installing plugin from {}...", url);

    let plugins_dir = default_plugins_dir();

    // Ensure plugins directory exists
    if !plugins_dir.exists() {
        std::fs::create_dir_all(&plugins_dir)?;
    }

    // Extract repo name from URL
    let repo_name = url
        .split('/')
        .next_back()
        .unwrap_or("plugin")
        .trim_end_matches(".git");
    let dest_path = plugins_dir.join(repo_name);

    if dest_path.exists() {
        eprintln!("Error: Plugin already exists at: {}", dest_path.display());
        std::process::exit(1);
    }

    // Clone the repository
    println!("Cloning repository...");
    match git2::Repository::clone(url, &dest_path) {
        Ok(_) => {
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
                    println!("  Name:        {}", name);
                    println!("  Version:     {}", version);
                    println!("  Description: {}", description);
                    println!("  Path:        {}", dest_path.display());
                    println!("  Capabilities: {}", capabilities_count);
                }
                Err(e) => {
                    // Cleanup on failure
                    eprintln!("Warning: Plugin cloned but failed to load: {}", e);
                    eprintln!(
                        "The plugin directory has been kept at: {}",
                        dest_path.display()
                    );
                    eprintln!("You may need to check the plugin's manifest file.");
                }
            }
        }
        Err(e) => {
            eprintln!("Error: Failed to clone repository: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Handle plugins uninstall command
pub fn handle_plugins_uninstall(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::default_plugins_dir;

    let plugins_dir = default_plugins_dir();
    let plugin_path = plugins_dir.join(name);

    if !plugin_path.exists() {
        eprintln!("Error: Plugin not found: {}", name);
        eprintln!("Plugin directory: {}", plugin_path.display());
        std::process::exit(1);
    }

    // Confirm uninstall
    println!("Uninstalling plugin: {}", name);
    println!("Path: {}", plugin_path.display());

    match std::fs::remove_dir_all(&plugin_path) {
        Ok(()) => {
            println!("Plugin uninstalled successfully.");
        }
        Err(e) => {
            eprintln!("Error: Failed to remove plugin: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Handle plugins enable command
pub fn handle_plugins_enable(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::default_plugins_dir;

    let plugins_dir = default_plugins_dir();
    let plugin_path = plugins_dir.join(name);

    if !plugin_path.exists() {
        eprintln!("Error: Plugin not found: {}", name);
        std::process::exit(1);
    }

    // Check for disabled marker file
    let disabled_marker = plugin_path.join(".disabled");
    if disabled_marker.exists() {
        std::fs::remove_file(&disabled_marker)?;
        println!("Plugin enabled: {}", name);
    } else {
        println!("Plugin is already enabled: {}", name);
    }

    Ok(())
}

/// Handle plugins disable command
pub fn handle_plugins_disable(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::default_plugins_dir;

    let plugins_dir = default_plugins_dir();
    let plugin_path = plugins_dir.join(name);

    if !plugin_path.exists() {
        eprintln!("Error: Plugin not found: {}", name);
        std::process::exit(1);
    }

    // Create disabled marker file
    let disabled_marker = plugin_path.join(".disabled");
    if !disabled_marker.exists() {
        std::fs::write(&disabled_marker, "")?;
        println!("Plugin disabled: {}", name);
    } else {
        println!("Plugin is already disabled: {}", name);
    }

    Ok(())
}

/// Install plugin from marketplace (by name) or URL (legacy path)
pub async fn handle_plugin_install(
    source: &str,
    scope: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::marketplace::types::{MarketplaceConfig, MarketplaceSourceType};
    use alephcore::extension::marketplace::MarketplaceManager;
    use alephcore::extension::scope::parse_scope;
    use alephcore::Config;

    // If source looks like a plugin name (no /, ., or :), use marketplace install locally
    let is_plugin_name = !source.contains('/') && !source.contains('.') && !source.contains(':');

    if is_plugin_name {
        let config = Config::load()?;
        let marketplace_configs: std::collections::HashMap<String, MarketplaceConfig> = config
            .plugin_marketplaces
            .iter()
            .map(
                |(name, entry): (&String, &alephcore::PluginMarketplaceEntry)| {
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
                },
            )
            .collect();

        let manager = MarketplaceManager::new(marketplace_configs, None);
        let plugin_scope = parse_scope(scope)?;

        println!("Searching for plugin '{}'...", source);
        match manager.install_to_scope(source, None, plugin_scope, None) {
            Ok(path) => {
                println!("Plugin '{}' installed to {:?}", source, path);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Direct URL install (legacy path)
        handle_plugins_install(source).await?;
    }
    Ok(())
}

/// Handle marketplace subcommands
pub async fn handle_marketplace_command(
    action: MarketplaceAction,
) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::extension::marketplace::types::{MarketplaceConfig, MarketplaceSourceType};
    use alephcore::extension::marketplace::MarketplaceManager;
    use alephcore::{Config, PluginMarketplaceEntry};

    // Load marketplace config from config.toml
    let config = Config::load()?;
    let marketplace_configs: std::collections::HashMap<String, MarketplaceConfig> = config
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
        .collect();

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

            println!("Added marketplace '{}' ({})", name, source);

            // Sync cache
            print!("Syncing cache...");
            match manager.update(&name) {
                Ok(_) => println!(" done."),
                Err(e) => println!(" failed: {}", e),
            }
        }
        MarketplaceAction::Update { name } => match name {
            Some(n) => {
                println!("Updating marketplace '{}'...", n);
                match manager.update(&n) {
                    Ok(path) => println!("Updated: {:?}", path),
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            None => {
                println!("Updating all marketplaces...");
                match manager.update_all() {
                    Ok(()) => println!("All marketplaces updated."),
                    Err(e) => eprintln!("Some updates failed: {}", e),
                }
            }
        },
        MarketplaceAction::Remove { name } => {
            manager.remove(&name)?;

            // Save to config
            let mut config = Config::load()?;
            config.plugin_marketplaces.remove(&name);
            config.save_incremental(&["plugin_marketplaces"])?;

            println!("Removed marketplace '{}'", name);
        }
    }
    Ok(())
}
