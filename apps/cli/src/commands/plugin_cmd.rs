//! Plugin developer commands — init, validate, pack, doctor.
//!
//! These commands operate locally (no server connection needed).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{CliError, CliResult};

// ---------------------------------------------------------------------------
// Plugin Template Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum PluginTemplate {
    NodeJs,
    Wasm,
    Static,
}

impl std::str::FromStr for PluginTemplate {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nodejs" | "node" | "js" | "ts" => Ok(Self::NodeJs),
            "wasm" | "rust" => Ok(Self::Wasm),
            "static" | "markdown" | "md" => Ok(Self::Static),
            _ => Err(format!(
                "Unknown template type: '{}'. Use: nodejs, wasm, or static",
                s
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// `aleph plugin init`
// ---------------------------------------------------------------------------

/// Scaffold a new plugin project.
pub fn init(name: &str, template: PluginTemplate, target_dir: Option<&Path>) -> CliResult<()> {
    let target = target_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(name));

    scaffold_plugin(&target, name, template)?;

    println!("Plugin '{}' created at {}", name, target.display());
    println!();
    match template {
        PluginTemplate::NodeJs => {
            println!("Next steps:");
            println!("  cd {}", target.display());
            println!("  npm install");
            println!("  npm run build");
            println!("  aleph plugin validate .");
        }
        PluginTemplate::Wasm => {
            println!("Next steps:");
            println!("  cd {}", target.display());
            println!("  cargo build --target wasm32-wasi --release");
            println!("  aleph plugin validate .");
        }
        PluginTemplate::Static => {
            println!("Next steps:");
            println!("  cd {}", target.display());
            println!("  # Edit SKILL.md with your skill content");
            println!("  aleph plugin validate .");
        }
    }

    Ok(())
}

/// Create the plugin directory structure and files.
pub fn scaffold_plugin(target: &Path, name: &str, template: PluginTemplate) -> CliResult<()> {
    // Check target directory
    if target.exists() {
        let entries: Vec<_> = std::fs::read_dir(target)?.collect();
        if !entries.is_empty() {
            return Err(CliError::Other(format!(
                "Directory '{}' is not empty. Use an empty or non-existent directory.",
                target.display()
            )));
        }
    }
    std::fs::create_dir_all(target)?;

    // Common manifest
    let kind = match template {
        PluginTemplate::NodeJs => "nodejs",
        PluginTemplate::Wasm => "wasm",
        PluginTemplate::Static => "static",
    };
    let entry = match template {
        PluginTemplate::NodeJs => "dist/index.js",
        PluginTemplate::Wasm => "target/wasm32-wasi/release/plugin.wasm",
        PluginTemplate::Static => "SKILL.md",
    };

    let manifest = format!(
        r#"[plugin]
id = "{name}"
name = "{name}"
version = "0.1.0"
description = "TODO: Describe your plugin"
kind = "{kind}"
entry = "{entry}"

[[tools]]
name = "{name}_hello"
description = "A sample tool — replace with your own"
handler = "hello"
parameters = {{ type = "object", properties = {{ message = {{ type = "string" }} }} }}
"#
    );

    std::fs::write(target.join("aleph.plugin.toml"), &manifest)?;

    match template {
        PluginTemplate::NodeJs => scaffold_nodejs(target, name)?,
        PluginTemplate::Wasm => scaffold_wasm(target, name)?,
        PluginTemplate::Static => scaffold_static(target, name)?,
    }

    Ok(())
}

fn scaffold_nodejs(target: &Path, name: &str) -> CliResult<()> {
    // package.json
    let package_json = format!(
        r#"{{
  "name": "{name}",
  "version": "0.1.0",
  "description": "Aleph plugin",
  "main": "dist/index.js",
  "scripts": {{
    "build": "tsc",
    "dev": "tsc --watch"
  }},
  "devDependencies": {{
    "typescript": "^5.0.0"
  }}
}}
"#
    );
    std::fs::write(target.join("package.json"), package_json)?;

    // tsconfig.json
    let tsconfig = r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "node",
    "outDir": "dist",
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "declaration": true
  },
  "include": ["src"]
}
"#;
    std::fs::write(target.join("tsconfig.json"), tsconfig)?;

    // src/index.ts
    std::fs::create_dir_all(target.join("src"))?;
    let index_ts = format!(
        r#"// {name} — Aleph Plugin
//
// This is the entry point for your plugin. Edit the tool registration
// below or add hooks, services, channels, etc.

export default async (api: any) => {{
  api.registerTool({{
    name: '{name}_hello',
    description: 'A sample tool from {name}',
    parameters: {{
      type: 'object',
      properties: {{
        message: {{ type: 'string', description: 'A greeting message' }},
      }},
    }},
    execute: async (_toolCallId: string, params: {{ message?: string }}) => {{
      return {{ result: `Hello from {name}: ${{params.message ?? 'world'}}` }};
    }},
  }});
}};
"#
    );
    std::fs::write(target.join("src/index.ts"), index_ts)?;

    // .gitignore
    std::fs::write(target.join(".gitignore"), "node_modules/\ndist/\n")?;

    Ok(())
}

fn scaffold_wasm(target: &Path, name: &str) -> CliResult<()> {
    let cargo_name = name.replace('-', "_");

    // Cargo.toml
    let cargo_toml = format!(
        r#"[package]
name = "{cargo_name}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
extism-pdk = "1"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#
    );
    std::fs::write(target.join("Cargo.toml"), cargo_toml)?;

    // src/lib.rs
    std::fs::create_dir_all(target.join("src"))?;
    let lib_rs = format!(
        r#"//! {name} — Aleph WASM Plugin

use extism_pdk::*;
use serde::{{Deserialize, Serialize}};

#[derive(Deserialize)]
struct HelloInput {{
    message: Option<String>,
}}

#[derive(Serialize)]
struct HelloOutput {{
    result: String,
}}

#[plugin_fn]
pub fn hello(input: Json<HelloInput>) -> FnResult<Json<HelloOutput>> {{
    let msg = input.0.message.unwrap_or_else(|| "world".to_string());
    Ok(Json(HelloOutput {{
        result: format!("Hello from {name}: {{}}", msg),
    }}))
}}
"#
    );
    std::fs::write(target.join("src/lib.rs"), lib_rs)?;

    // .gitignore
    std::fs::write(target.join(".gitignore"), "target/\n")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// `aleph plugin validate`
// ---------------------------------------------------------------------------

/// Result of validating a plugin directory.
#[derive(Debug, Default)]
pub struct PluginValidation {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub info: Vec<String>,
}

/// Validate a plugin directory for correctness.
pub fn validate(plugin_dir: &Path, json_mode: bool) -> CliResult<()> {
    let result = validate_plugin_dir(plugin_dir)?;

    if json_mode {
        let json = serde_json::json!({
            "valid": result.errors.is_empty(),
            "errors": result.errors,
            "warnings": result.warnings,
            "info": result.info,
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        for msg in &result.info {
            println!("  [info] {}", msg);
        }
        for msg in &result.warnings {
            println!("  [warn] {}", msg);
        }
        for msg in &result.errors {
            println!("  [error] {}", msg);
        }
        if result.errors.is_empty() {
            println!("\nValidation passed.");
        } else {
            println!("\nValidation failed with {} error(s).", result.errors.len());
        }
    }

    Ok(())
}

fn validate_plugin_dir(plugin_dir: &Path) -> CliResult<PluginValidation> {
    let mut result = PluginValidation::default();

    if !plugin_dir.exists() {
        result
            .errors
            .push(format!("Directory does not exist: {}", plugin_dir.display()));
        return Ok(result);
    }

    // Check manifest
    let manifest_path = plugin_dir.join("aleph.plugin.toml");
    if !manifest_path.exists() {
        result
            .errors
            .push("No aleph.plugin.toml found".to_string());
        return Ok(result);
    }

    let content = std::fs::read_to_string(&manifest_path).map_err(CliError::Io)?;
    let toml: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(e) => {
            result.errors.push(format!("Invalid TOML: {}", e));
            return Ok(result);
        }
    };

    // Check [plugin] section
    let plugin = match toml.get("plugin") {
        Some(p) => p,
        None => {
            result.errors.push("Missing [plugin] section".to_string());
            return Ok(result);
        }
    };

    // Required fields
    for field in ["id", "name", "kind", "entry"] {
        match plugin.get(field).and_then(|v| v.as_str()) {
            Some(val) if !val.is_empty() => {}
            _ => result
                .errors
                .push(format!("Missing or empty required field: plugin.{}", field)),
        }
    }

    let id = plugin
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");
    let name = plugin
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");
    result.info.push(format!("Plugin: {} ({})", name, id));

    // Check entry file
    if let Some(entry) = plugin.get("entry").and_then(|v| v.as_str()) {
        let entry_path = plugin_dir.join(entry);
        if !entry_path.exists() {
            result.warnings.push(format!(
                "Entry file not found: {} (run build first?)",
                entry
            ));
        }
    }

    // Check for duplicate tool names
    if let Some(tools) = toml.get("tools").and_then(|v| v.as_array()) {
        let mut names = std::collections::HashSet::new();
        for tool in tools {
            if let Some(tool_name) = tool.get("name").and_then(|v| v.as_str()) {
                if !names.insert(tool_name) {
                    result
                        .errors
                        .push(format!("Duplicate tool name: '{}'", tool_name));
                }
            }
        }
        result.info.push(format!("{} tool(s) defined", tools.len()));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// `aleph plugin doctor`
// ---------------------------------------------------------------------------

/// A single diagnostic check result.
#[derive(Debug)]
pub struct DoctorCheck {
    pub name: String,
    pub description: String,
    pub passed: bool,
    pub required: bool,
    pub message: String,
}

/// Run all plugin doctor checks.
pub fn doctor(json_mode: bool) -> CliResult<()> {
    let checks = run_doctor_checks();

    if json_mode {
        let json_checks: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "description": c.description,
                    "passed": c.passed,
                    "required": c.required,
                    "message": c.message,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_checks).unwrap_or_default()
        );
    } else {
        println!("Plugin Doctor\n");
        for check in &checks {
            let status = if check.passed {
                "OK"
            } else if check.required {
                "FAIL"
            } else {
                "WARN"
            };
            let icon = if check.passed { "+" } else { "-" };
            println!(
                "  [{}] {} — {} ({})",
                icon, check.name, check.description, status
            );
            if !check.passed {
                println!("       {}", check.message);
            }
        }

        let failed = checks.iter().filter(|c| !c.passed && c.required).count();
        let warned = checks.iter().filter(|c| !c.passed && !c.required).count();
        println!();
        if failed == 0 {
            println!("All required checks passed.");
            if warned > 0 {
                println!("{} optional check(s) need attention.", warned);
            }
        } else {
            println!("{} required check(s) failed.", failed);
        }
    }

    Ok(())
}

/// Run all diagnostic checks and return the results.
pub fn run_doctor_checks() -> Vec<DoctorCheck> {
    vec![
        check_node_available(),
        check_npm_available(),
        check_wasm_target(),
        check_plugin_dir_exists(),
    ]
}

fn check_node_available() -> DoctorCheck {
    let result = std::process::Command::new("node")
        .arg("--version")
        .output();
    DoctorCheck {
        name: "node".into(),
        description: "Node.js runtime (for Node.js plugins)".into(),
        passed: result.as_ref().map(|o| o.status.success()).unwrap_or(false),
        required: false,
        message: match result {
            Ok(ref o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => "Not found. Install Node.js for Node.js plugin support.".into(),
        },
    }
}

fn check_npm_available() -> DoctorCheck {
    let result = std::process::Command::new("npm")
        .arg("--version")
        .output();
    DoctorCheck {
        name: "npm".into(),
        description: "npm package manager".into(),
        passed: result.as_ref().map(|o| o.status.success()).unwrap_or(false),
        required: false,
        message: match result {
            Ok(ref o) if o.status.success() => {
                format!("v{}", String::from_utf8_lossy(&o.stdout).trim())
            }
            _ => "Not found. Install npm for Node.js plugin development.".into(),
        },
    }
}

fn check_wasm_target() -> DoctorCheck {
    let result = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    let has_wasi = result
        .as_ref()
        .map(|o| {
            let output = String::from_utf8_lossy(&o.stdout);
            output.contains("wasm32-wasi") || output.contains("wasm32-wasip1")
        })
        .unwrap_or(false);
    DoctorCheck {
        name: "wasm-target".into(),
        description: "WASM compilation target (for WASM plugins)".into(),
        passed: has_wasi,
        required: false,
        message: if has_wasi {
            "wasm32-wasi target installed".into()
        } else {
            "Not found. Run: rustup target add wasm32-wasip1".into()
        },
    }
}

fn check_plugin_dir_exists() -> DoctorCheck {
    let home = dirs::home_dir();
    let plugin_dir = home.as_ref().map(|h| h.join(".aleph/extensions"));
    let exists = plugin_dir.as_ref().map(|p| p.exists()).unwrap_or(false);
    DoctorCheck {
        name: "plugin-dir".into(),
        description: "Global plugin directory (~/.aleph/extensions/)".into(),
        passed: exists,
        required: false,
        message: if exists {
            format!("{} exists", plugin_dir.unwrap().display())
        } else {
            "~/.aleph/extensions/ does not exist. Will be created on first plugin install.".into()
        },
    }
}

// ---------------------------------------------------------------------------
// `aleph plugin init` — Static template scaffold
// ---------------------------------------------------------------------------

fn scaffold_static(target: &Path, name: &str) -> CliResult<()> {
    let skill_md = format!(
        r#"---
name: {name}
description: TODO — describe what this skill does
---

# {name}

Write your skill instructions here. The AI assistant will follow these
instructions when this skill is invoked.

## Usage

Describe when and how to use this skill.
"#
    );
    std::fs::write(target.join("SKILL.md"), skill_md)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// `aleph plugin pack`
// ---------------------------------------------------------------------------

const PACK_EXCLUDE: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    ".DS_Store",
    ".aleph-plugin.zip",
    "__pycache__",
    ".mypy_cache",
];

/// Pack a plugin directory into a distributable archive.
pub fn pack(plugin_dir: &Path, output: Option<&Path>) -> CliResult<()> {
    // 1. Validate first
    let validation = validate_plugin_dir(plugin_dir)?;
    if !validation.errors.is_empty() {
        for err in &validation.errors {
            eprintln!("  [error] {}", err);
        }
        return Err(CliError::Other(
            "Plugin validation failed. Fix errors before packing.".into(),
        ));
    }

    // 2. Determine output path
    let plugin_name = plugin_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin");
    let output_path = output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| plugin_dir.join(format!("{}.aleph-plugin.zip", plugin_name)));

    // 3. Create zip
    let file = std::fs::File::create(&output_path).map_err(CliError::Io)?;
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // 4. Walk directory, add files
    add_dir_to_zip(&mut zip, plugin_dir, plugin_dir, &options)?;

    zip.finish()
        .map_err(|e| CliError::Other(format!("Failed to finalize zip: {}", e)))?;

    println!("Packed plugin to: {}", output_path.display());
    let size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    println!("Archive size: {} bytes", size);

    Ok(())
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
    options: &zip::write::SimpleFileOptions,
) -> CliResult<()> {
    for entry in std::fs::read_dir(dir).map_err(CliError::Io)? {
        let entry = entry.map_err(CliError::Io)?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip excluded patterns
        if PACK_EXCLUDE
            .iter()
            .any(|ex| name == *ex || name.ends_with(ex))
        {
            continue;
        }

        let relative = path.strip_prefix(base).unwrap_or(&path);
        let relative_str = relative.to_string_lossy().replace('\\', "/");

        if path.is_dir() {
            add_dir_to_zip(zip, base, &path, options)?;
        } else {
            zip.start_file(&relative_str, *options)
                .map_err(|e| CliError::Other(format!("Zip error: {}", e)))?;
            let mut f = std::fs::File::open(&path).map_err(CliError::Io)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(CliError::Io)?;
            zip.write_all(&buf).map_err(CliError::Io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scaffold_nodejs_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-plugin");

        scaffold_plugin(&target, "my-plugin", PluginTemplate::NodeJs).unwrap();

        assert!(target.join("aleph.plugin.toml").exists());
        assert!(target.join("package.json").exists());
        assert!(target.join("src/index.ts").exists());
        assert!(target.join("tsconfig.json").exists());
        assert!(target.join(".gitignore").exists());

        let manifest = std::fs::read_to_string(target.join("aleph.plugin.toml")).unwrap();
        assert!(manifest.contains("my-plugin"));
        assert!(manifest.contains(r#"kind = "nodejs""#));
    }

    #[test]
    fn scaffold_wasm_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-wasm");

        scaffold_plugin(&target, "my-wasm", PluginTemplate::Wasm).unwrap();

        assert!(target.join("aleph.plugin.toml").exists());
        assert!(target.join("Cargo.toml").exists());
        assert!(target.join("src/lib.rs").exists());

        let manifest = std::fs::read_to_string(target.join("aleph.plugin.toml")).unwrap();
        assert!(manifest.contains(r#"kind = "wasm""#));
    }

    #[test]
    fn scaffold_static_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-static");

        scaffold_plugin(&target, "my-static", PluginTemplate::Static).unwrap();

        assert!(target.join("aleph.plugin.toml").exists());
        assert!(target.join("SKILL.md").exists());

        let skill = std::fs::read_to_string(target.join("SKILL.md")).unwrap();
        assert!(skill.contains("my-static"));
    }

    #[test]
    fn rejects_existing_non_empty_directory() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("existing");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("file.txt"), "content").unwrap();

        let result = scaffold_plugin(&target, "existing", PluginTemplate::NodeJs);
        assert!(result.is_err());
    }

    #[test]
    fn accepts_existing_empty_directory() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("empty");
        std::fs::create_dir_all(&target).unwrap();

        let result = scaffold_plugin(&target, "empty", PluginTemplate::NodeJs);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_valid_plugin() {
        let dir = tempdir().unwrap();
        scaffold_plugin(dir.path().join("p").as_path(), "test", PluginTemplate::Static).unwrap();

        let result = validate_plugin_dir(dir.path().join("p").as_path()).unwrap();
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert!(!result.info.is_empty());
    }

    #[test]
    fn validate_missing_manifest() {
        let dir = tempdir().unwrap();
        let result = validate_plugin_dir(dir.path()).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("aleph.plugin.toml"));
    }

    #[test]
    fn validate_missing_required_fields() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("aleph.plugin.toml"),
            "[plugin]\nid = \"test\"\n",
        )
        .unwrap();
        let result = validate_plugin_dir(dir.path()).unwrap();
        assert!(result.errors.iter().any(|e| e.contains("plugin.name")));
        assert!(result.errors.iter().any(|e| e.contains("plugin.kind")));
        assert!(result.errors.iter().any(|e| e.contains("plugin.entry")));
    }

    #[test]
    fn validate_duplicate_tool_names() {
        let dir = tempdir().unwrap();
        let manifest = r#"
[plugin]
id = "dup"
name = "dup"
kind = "static"
entry = "SKILL.md"

[[tools]]
name = "foo"
description = "first"

[[tools]]
name = "foo"
description = "duplicate"
"#;
        std::fs::write(dir.path().join("aleph.plugin.toml"), manifest).unwrap();
        let result = validate_plugin_dir(dir.path()).unwrap();
        assert!(result.errors.iter().any(|e| e.contains("Duplicate tool name: 'foo'")));
    }

    #[test]
    fn validate_nonexistent_directory() {
        let result = validate_plugin_dir(Path::new("/tmp/does-not-exist-aleph-test")).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("does not exist"));
    }

    #[test]
    fn template_from_str() {
        assert!(matches!(
            "nodejs".parse::<PluginTemplate>().unwrap(),
            PluginTemplate::NodeJs
        ));
        assert!(matches!(
            "node".parse::<PluginTemplate>().unwrap(),
            PluginTemplate::NodeJs
        ));
        assert!(matches!(
            "wasm".parse::<PluginTemplate>().unwrap(),
            PluginTemplate::Wasm
        ));
        assert!(matches!(
            "rust".parse::<PluginTemplate>().unwrap(),
            PluginTemplate::Wasm
        ));
        assert!(matches!(
            "static".parse::<PluginTemplate>().unwrap(),
            PluginTemplate::Static
        ));
        assert!("unknown".parse::<PluginTemplate>().is_err());
    }

    #[test]
    fn pack_creates_zip() {
        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        scaffold_plugin(&plugin_dir, "my-plugin", PluginTemplate::Static).unwrap();

        let output = dir.path().join("out.aleph-plugin.zip");
        pack(&plugin_dir, Some(&output)).unwrap();

        assert!(output.exists());
        assert!(output.metadata().unwrap().len() > 0);
    }

    #[test]
    fn doctor_checks_run() {
        let checks = run_doctor_checks();
        assert!(!checks.is_empty());
        // At minimum we should have 4 checks
        assert!(checks.len() >= 4);
        // Each check has a name and description
        for check in &checks {
            assert!(!check.name.is_empty());
            assert!(!check.description.is_empty());
        }
    }

    #[test]
    fn pack_excludes_node_modules() {
        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("p");
        scaffold_plugin(&plugin_dir, "p", PluginTemplate::NodeJs).unwrap();

        // Create fake node_modules
        std::fs::create_dir_all(plugin_dir.join("node_modules/dep")).unwrap();
        std::fs::write(plugin_dir.join("node_modules/dep/index.js"), "").unwrap();

        let output = dir.path().join("out.zip");
        pack(&plugin_dir, Some(&output)).unwrap();

        let file = std::fs::File::open(&output).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
        assert!(names.iter().all(|n| !n.contains("node_modules")));
        // But should include other files
        assert!(names.iter().any(|n| n.contains("aleph.plugin.toml")));
    }
}
