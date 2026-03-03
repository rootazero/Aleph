//! Plugin management command handlers

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
        println!("{:<25} {:<12} {:<10} {:<40}", "NAME", "VERSION", "STATUS", "DESCRIPTION");
        println!("{}", "-".repeat(90));
        for plugin in &plugins {
            let version = plugin.version.clone().unwrap_or_else(|| "-".to_string());
            let status = if plugin.enabled { "enabled" } else { "disabled" };
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
    use alephcore::extension::{default_plugins_dir, ComponentLoader};

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

            // Try to load the plugin to verify it's valid
            let loader = ComponentLoader::new();

            match loader.load_plugin(&dest_path).await {
                Ok(plugin) => {
                    let info = plugin.info();
                    println!();
                    println!("Plugin installed successfully!");
                    println!("  Name:        {}", info.name);
                    println!("  Version:     {}", info.version.unwrap_or_else(|| "-".to_string()));
                    println!("  Description: {}", info.description.unwrap_or_else(|| "-".to_string()));
                    println!("  Path:        {}", dest_path.display());
                    println!("  Skills:      {}", info.skills_count);
                    println!("  Agents:      {}", info.agents_count);
                    println!("  Hooks:       {}", info.hooks_count);
                }
                Err(e) => {
                    // Cleanup on failure
                    eprintln!("Warning: Plugin cloned but failed to load: {}", e);
                    eprintln!("The plugin directory has been kept at: {}", dest_path.display());
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
