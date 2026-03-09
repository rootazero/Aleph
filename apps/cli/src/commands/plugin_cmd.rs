//! Plugin developer commands — init, validate, pack, doctor.
//!
//! These commands operate locally (no server connection needed).

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
}
