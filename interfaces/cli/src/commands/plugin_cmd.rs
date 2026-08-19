//! Plugin developer commands — init, validate, pack, doctor.
//!
//! These commands operate locally (no server connection needed).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use aleph_client::{CliError, CliResult};

// ---------------------------------------------------------------------------
// Plugin Template Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTemplate {
    /// An MCP stdio server. This is how a Node/TypeScript plugin actually runs
    /// in Aleph.
    ///
    /// There is no Node.js plugin *runtime*: `src/extension/runtime/` contains
    /// `wasm` and nothing else. The `nodejs` template that lived here until
    /// 2026-08-19 wrote `kind = "nodejs"` — which `PluginKind` rejects with
    /// `unknown variant`, so the scaffolded plugin could never load — and an
    /// `api.registerTool(...)` entry point against an API with exactly one
    /// occurrence in the tree: the template that wrote it. The aliases are
    /// kept because they are what an author types; they now produce something
    /// that runs.
    Mcp,
    Wasm,
    Static,
}

impl PluginTemplate {
    /// The `[aleph] runtime` value this template declares.
    #[must_use]
    pub const fn runtime(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Wasm => "wasm",
            Self::Static => "static",
        }
    }
}

impl std::str::FromStr for PluginTemplate {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mcp" | "nodejs" | "node" | "js" | "ts" => Ok(Self::Mcp),
            "wasm" | "rust" => Ok(Self::Wasm),
            "static" | "markdown" | "md" => Ok(Self::Static),
            _ => Err(format!(
                "Unknown template type: '{s}'. Use: mcp (aliases: nodejs/node/js/ts), wasm, or static"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// `aleph plugin init`
// ---------------------------------------------------------------------------

/// Scaffold a new plugin project.
pub fn init(name: &str, template: PluginTemplate, target_dir: Option<&Path>) -> CliResult<()> {
    let target = target_dir.map_or_else(|| PathBuf::from(name), std::path::Path::to_path_buf);

    scaffold_plugin(&target, name, template)?;

    println!("Plugin '{}' created at {}", name, target.display());
    println!();
    match template {
        PluginTemplate::Mcp => {
            println!("Next steps:");
            println!("  cd {}", target.display());
            println!("  npm install");
            println!("  aleph plugin validate .");
            println!("  aleph plugin install .");
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
    // Plugin names are interpolated unescaped into TOML/Cargo/JSON/TS
    // template bodies; without a character whitelist a name like
    // `a"foo=b" \n key = "injected"` would break TOML parsing or smuggle
    // extra fields. npm-style names (lowercase + hyphens, optional scope
    // prefix) cover the common cases. Reject anything outside `[A-Za-z0-9._-]`.
    if !is_safe_plugin_name(name) {
        return Err(CliError::Other(format!(
            "invalid plugin name '{name}': use only letters, digits, '.', '_' or '-'"
        )));
    }
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

    // Manifest. `.claude-plugin/plugin.toml` — the format PLUGIN_SYSTEM.md
    // calls preferred and the one the loader does NOT warn about. The
    // scaffolder emitted `aleph.plugin.toml` until 2026-08-19, so every plugin
    // Aleph created for you started life on the deprecated path.
    let runtime = template.runtime();
    debug_assert!(
        aleph_protocol::plugins::is_known_plugin_runtime(runtime),
        "a template must not scaffold a runtime the host cannot load"
    );
    let entry = match template {
        PluginTemplate::Mcp => ".mcp.json",
        PluginTemplate::Wasm => "target/wasm32-wasi/release/plugin.wasm",
        PluginTemplate::Static => "SKILL.md",
    };

    // Only the WASM template declares a handler-backed tool: a handler is an
    // exported guest function, and neither an MCP server (its tools come from
    // the server) nor a static plugin has one. Declaring one anyway is how the
    // old template produced a manifest that described a tool nothing could
    // dispatch.
    let tools = if matches!(template, PluginTemplate::Wasm) {
        format!(
            r#"
[[aleph.tools]]
name = "{name}_hello"
description = "A sample tool — replace with your own"
handler = "hello"
parameters = {{ type = "object", properties = {{ message = {{ type = "string" }} }} }}
"#
        )
    } else {
        String::new()
    };

    let manifest = format!(
        r#"name = "{name}"
version = "0.1.0"
description = "TODO: Describe your plugin"

[aleph]
runtime = "{runtime}"
entry = "{entry}"
{tools}"#
    );

    std::fs::create_dir_all(target.join(".claude-plugin"))?;
    std::fs::write(target.join(".claude-plugin/plugin.toml"), &manifest)?;

    match template {
        PluginTemplate::Mcp => scaffold_mcp(target, name)?,
        PluginTemplate::Wasm => scaffold_wasm(target, name)?,
        PluginTemplate::Static => scaffold_static(target, name)?,
    }

    Ok(())
}

/// Scaffold an MCP stdio server plugin.
///
/// Aleph runs this through its MCP client (`.mcp.json` → transient server
/// namespaced `plugin:<id>/<server>`), which is a path that exists. The
/// template it replaced targeted an `api.registerTool` host API that never
/// did — `registerTool` had exactly one occurrence in the whole tree, namely
/// the template that wrote it.
fn scaffold_mcp(target: &Path, name: &str) -> CliResult<()> {
    // .mcp.json — the entry Aleph actually reads. `${CLAUDE_PLUGIN_ROOT}` is
    // expanded by the host, so the server runs from the install directory
    // wherever the plugin ends up.
    let mcp_json = format!(
        r#"{{
  "mcpServers": {{
    "{name}": {{
      "command": "node",
      "args": ["${{CLAUDE_PLUGIN_ROOT}}/src/index.mjs"]
    }}
  }}
}}
"#
    );
    std::fs::write(target.join(".mcp.json"), mcp_json)?;

    let package_json = format!(
        r#"{{
  "name": "{name}",
  "version": "0.1.0",
  "description": "Aleph plugin (MCP stdio server)",
  "type": "module",
  "main": "src/index.mjs",
  "dependencies": {{
    "@modelcontextprotocol/sdk": "^1.0.0"
  }}
}}
"#
    );
    std::fs::write(target.join("package.json"), package_json)?;

    std::fs::create_dir_all(target.join("src"))?;
    let index_mjs = format!(
        r#"// {name} — Aleph plugin, served over MCP (stdio).
//
// Aleph starts this process from `.mcp.json` and speaks MCP to it, so every
// tool you register here shows up as `{name}__<tool>` in the agent's tool
// list. Run `npm install` once, then `aleph plugin install .`.

import {{ McpServer }} from "@modelcontextprotocol/sdk/server/mcp.js";
import {{ StdioServerTransport }} from "@modelcontextprotocol/sdk/server/stdio.js";
import {{ z }} from "zod";

const server = new McpServer({{ name: "{name}", version: "0.1.0" }});

server.tool(
  "hello",
  "A sample tool — replace with your own",
  {{ message: z.string().optional().describe("A greeting message") }},
  async ({{ message }}) => ({{
    content: [{{ type: "text", text: `Hello from {name}: ${{message ?? "world"}}` }}],
  }}),
);

await server.connect(new StdioServerTransport());
"#
    );
    std::fs::write(target.join("src/index.mjs"), index_mjs)?;

    std::fs::write(target.join(".gitignore"), "node_modules/\n")?;

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
            println!("  [info] {msg}");
        }
        for msg in &result.warnings {
            println!("  [warn] {msg}");
        }
        for msg in &result.errors {
            println!("  [error] {msg}");
        }
        if result.errors.is_empty() {
            println!("\nValidation passed.");
        } else {
            println!("\nValidation failed with {} error(s).", result.errors.len());
        }
    }

    Ok(())
}

/// Which manifest a plugin directory carries, and where.
///
/// Discovery order mirrors `extension::manifest::parse_manifest_from_dir_sync`
/// so `aleph plugin validate` reads the same file the server will.
enum FoundManifest {
    /// `.claude-plugin/plugin.toml` — the preferred format.
    Preferred(toml::Value),
    /// `aleph.plugin.toml` — deprecated; the loader warns on every load.
    Deprecated(toml::Value),
}

/// Locate and parse a plugin's manifest.
fn find_manifest(plugin_dir: &Path, result: &mut PluginValidation) -> Option<FoundManifest> {
    let candidates = [
        (
            plugin_dir.join(".claude-plugin/plugin.toml"),
            true,
            ".claude-plugin/plugin.toml",
        ),
        (
            plugin_dir.join("aleph.plugin.toml"),
            false,
            "aleph.plugin.toml",
        ),
    ];
    for (path, preferred, label) in candidates {
        if !path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                result.errors.push(format!("Cannot read {label}: {e}"));
                return None;
            }
        };
        return match content.parse::<toml::Value>() {
            Ok(v) => Some(if preferred {
                FoundManifest::Preferred(v)
            } else {
                FoundManifest::Deprecated(v)
            }),
            Err(e) => {
                result.errors.push(format!("Invalid TOML in {label}: {e}"));
                None
            }
        };
    }
    result.errors.push(
        "No manifest found. Expected .claude-plugin/plugin.toml (preferred) or aleph.plugin.toml"
            .to_string(),
    );
    None
}

/// Validate a plugin directory without contacting the server.
///
/// This deliberately stays local — `aleph plugin validate` is documented as
/// working with no daemon, and `interfaces/cli` may not depend on `alephcore`.
/// What it must NOT be is a *second schema*: until 2026-08-19 it accepted
/// `kind = "nodejs"`, which the server rejects with `unknown variant`, so
/// `aleph plugin init --type nodejs && aleph plugin validate .` printed a
/// green check for a plugin that could never load — and `aleph plugin pack`
/// shipped it. The runtime vocabulary now comes from
/// [`aleph_protocol::plugins::PLUGIN_RUNTIMES`], which is the same list the
/// server's `PluginKind` derives from.
fn validate_plugin_dir(plugin_dir: &Path) -> CliResult<PluginValidation> {
    let mut result = PluginValidation::default();

    if !plugin_dir.exists() {
        result.errors.push(format!(
            "Directory does not exist: {}",
            plugin_dir.display()
        ));
        return Ok(result);
    }

    let Some(found) = find_manifest(plugin_dir, &mut result) else {
        return Ok(result);
    };

    // The two dialects put the same facts in different places: the preferred
    // one is flat with an `[aleph]` block, the deprecated one nests under
    // `[plugin]`.
    let (toml, name_key, runtime_key, section, tools_path) = match &found {
        FoundManifest::Preferred(v) => (v, "name", "runtime", v.get("aleph"), "aleph"),
        FoundManifest::Deprecated(v) => {
            result.warnings.push(
                "aleph.plugin.toml is deprecated — the loader warns on every load. \
                 Migrate to .claude-plugin/plugin.toml"
                    .to_string(),
            );
            (v, "id", "kind", v.get("plugin"), "")
        }
    };

    let Some(section) = section else {
        match found {
            FoundManifest::Preferred(_) => result
                .warnings
                .push("No [aleph] section — the plugin loads as a static plugin".to_string()),
            FoundManifest::Deprecated(_) => {
                result.errors.push("Missing [plugin] section".to_string());
                return Ok(result);
            }
        }
        return Ok(result);
    };

    // Identity lives at the top level in the preferred format and inside
    // [plugin] in the deprecated one.
    let identity = match &found {
        FoundManifest::Preferred(v) => v,
        FoundManifest::Deprecated(_) => section,
    };
    match identity.get(name_key).and_then(|v| v.as_str()) {
        Some(val) if !val.is_empty() => {
            result.info.push(format!("Plugin: {val}"));
        }
        _ => result
            .errors
            .push(format!("Missing or empty required field: {name_key}")),
    }

    // The check that would have caught the scaffolder.
    match section.get(runtime_key).and_then(|v| v.as_str()) {
        Some(runtime) if aleph_protocol::plugins::is_known_plugin_runtime(runtime) => {
            result.info.push(format!("Runtime: {runtime}"));
        }
        Some(runtime) => result.errors.push(format!(
            "Unknown runtime '{runtime}'. The host can load: {}. \
             A manifest declaring anything else fails to parse and the plugin never loads.",
            aleph_protocol::plugins::PLUGIN_RUNTIMES.join(", ")
        )),
        None => result
            .info
            .push("Runtime: static (not declared)".to_string()),
    }

    // Entry file, when one is declared.
    if let Some(entry) = section.get("entry").and_then(|v| v.as_str()) {
        if !plugin_dir.join(entry).exists() {
            result
                .warnings
                .push(format!("Entry file not found: {entry} (run build first?)"));
        }
    }

    // Duplicate tool names — the server rejects these too.
    let tools = if tools_path.is_empty() {
        toml.get("tools")
    } else {
        section.get("tools")
    };
    if let Some(tools) = tools.and_then(|v| v.as_array()) {
        let mut names = std::collections::HashSet::new();
        for tool in tools {
            if let Some(tool_name) = tool.get("name").and_then(|v| v.as_str()) {
                if !names.insert(tool_name) {
                    result
                        .errors
                        .push(format!("Duplicate tool name: '{tool_name}'"));
                }
            }
        }
        result
            .info
            .push(format!("{} tool(s) declared", tools.len()));
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
                println!("{warned} optional check(s) need attention.");
            }
        } else {
            println!("{failed} required check(s) failed.");
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
    let result = std::process::Command::new("node").arg("--version").output();
    DoctorCheck {
        name: "node".into(),
        description: "Node.js (for MCP-server plugins)".into(),
        passed: result.as_ref().is_ok_and(|o| o.status.success()),
        required: false,
        message: match result {
            Ok(ref o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => "Not found. Install Node.js to run MCP-server plugins written in JS/TS.".into(),
        },
    }
}

fn check_npm_available() -> DoctorCheck {
    let result = std::process::Command::new("npm").arg("--version").output();
    DoctorCheck {
        name: "npm".into(),
        description: "npm package manager".into(),
        passed: result.as_ref().is_ok_and(|o| o.status.success()),
        required: false,
        message: match result {
            Ok(ref o) if o.status.success() => {
                format!("v{}", String::from_utf8_lossy(&o.stdout).trim())
            }
            _ => "Not found. Install npm to install an MCP-server plugin's dependencies.".into(),
        },
    }
}

fn check_wasm_target() -> DoctorCheck {
    let result = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    let has_wasi = result.as_ref().is_ok_and(|o| {
        let output = String::from_utf8_lossy(&o.stdout);
        output.contains("wasm32-wasi") || output.contains("wasm32-wasip1")
    });
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

/// Report on the directory global plugin installs actually land in.
///
/// This probed `~/.aleph/extensions` until 2026-08-19 — a path nothing in the
/// tree ever creates or reads. Plugins install into
/// `<aleph_home>/plugins/installed` (`default_install_dir`) or, for project
/// scope, `<project>/.aleph/plugins[.local]`. So the check reported "does not
/// exist" on a machine with plugins installed, and it built the path from a
/// hand-rolled `dirs::home_dir()`, which ignores `ALEPH_HOME` on top of that.
fn check_plugin_dir_exists() -> DoctorCheck {
    let plugin_dir = super::doctor::aleph_home().join("plugins/installed");
    let exists = plugin_dir.exists();
    DoctorCheck {
        name: "plugin-dir".into(),
        description: "Global plugin directory".into(),
        passed: exists,
        required: false,
        message: if exists {
            let count = std::fs::read_dir(&plugin_dir)
                .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count())
                .unwrap_or(0);
            format!("{} ({count} installed)", plugin_dir.display())
        } else {
            format!(
                "{} does not exist. It is created on the first global plugin install.",
                plugin_dir.display()
            )
        },
    }
}

// ---------------------------------------------------------------------------
// `aleph plugin init` — Static template scaffold
// ---------------------------------------------------------------------------

fn scaffold_static(target: &Path, name: &str) -> CliResult<()> {
    let skill_md = format!(
        r"---
name: {name}
description: TODO — describe what this skill does
---

# {name}

Write your skill instructions here. The AI assistant will follow these
instructions when this skill is invoked.

## Usage

Describe when and how to use this skill.
"
    );
    std::fs::write(target.join("SKILL.md"), skill_md)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// `aleph plugin pack`
// ---------------------------------------------------------------------------

/// A plugin name is safe for unescaped interpolation into TOML/Cargo/JSON/TS
/// template bodies iff every byte is a member of `[A-Za-z0-9._-]`. Empty
/// strings are rejected (every downstream consumer also wants non-empty).
fn is_safe_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .as_bytes()
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

#[cfg(test)]
mod name_tests {
    use super::is_safe_plugin_name;

    #[test]
    fn allows_npm_style_names() {
        for ok in ["my-plugin", "my_plugin", "plugin.js", "Plugin42", "v1.2.3"] {
            assert!(is_safe_plugin_name(ok), "should accept: {ok}");
        }
    }

    #[test]
    fn rejects_quotes_backslashes_newlines_and_empty() {
        for bad in [
            "",
            "a\"",
            "a\\",
            "a\nb",
            "a=b",
            "../escape",
            "name with space",
            "name;injection",
        ] {
            assert!(!is_safe_plugin_name(bad), "should reject: {bad:?}");
        }
    }
}

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
            eprintln!("  [error] {err}");
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
    let output_path = output.map_or_else(
        || plugin_dir.join(format!("{plugin_name}.aleph-plugin.zip")),
        std::path::Path::to_path_buf,
    );

    // 3. Create zip
    let file = std::fs::File::create(&output_path).map_err(CliError::Io)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // 4. Walk directory, add files
    add_dir_to_zip(&mut zip, plugin_dir, plugin_dir, &options)?;

    zip.finish()
        .map_err(|e| CliError::Other(format!("Failed to finalize zip: {e}")))?;

    println!("Packed plugin to: {}", output_path.display());
    let size = std::fs::metadata(&output_path).map_or(0, |m| m.len());
    println!("Archive size: {size} bytes");

    Ok(())
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
    options: &zip::write::SimpleFileOptions,
) -> CliResult<()> {
    let mut buf = Vec::new();
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
                .map_err(|e| CliError::Other(format!("Zip error: {e}")))?;
            let mut f = std::fs::File::open(&path).map_err(CliError::Io)?;
            buf.clear();
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

    /// Every template must scaffold a runtime the host can actually load.
    ///
    /// The test this replaces asserted `kind = "nodejs"` — the literal the
    /// scaffolder had just written — so it stayed green for as long as the
    /// scaffolder produced a manifest the server rejects with `unknown
    /// variant`. The assertion is now derived from the wire vocabulary, so it
    /// cannot agree with a wrong value.
    #[test]
    fn every_template_scaffolds_a_runtime_the_host_can_load() {
        for template in [
            PluginTemplate::Mcp,
            PluginTemplate::Wasm,
            PluginTemplate::Static,
        ] {
            let dir = tempdir().unwrap();
            let target = dir.path().join("p");
            scaffold_plugin(&target, "p", template).unwrap();

            let manifest_path = target.join(".claude-plugin/plugin.toml");
            assert!(
                manifest_path.exists(),
                "{template:?} must scaffold the preferred manifest, not the deprecated one"
            );
            let parsed: toml::Value = std::fs::read_to_string(&manifest_path)
                .unwrap()
                .parse()
                .unwrap_or_else(|e| panic!("{template:?} scaffolded unparseable TOML: {e}"));
            let runtime = parsed
                .get("aleph")
                .and_then(|a| a.get("runtime"))
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{template:?} declared no runtime"));
            assert!(
                aleph_protocol::plugins::is_known_plugin_runtime(runtime),
                "{template:?} scaffolded runtime '{runtime}', which the host cannot load"
            );
            assert_eq!(runtime, template.runtime());
        }
    }

    #[test]
    fn scaffold_mcp_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-plugin");

        scaffold_plugin(&target, "my-plugin", PluginTemplate::Mcp).unwrap();

        // `.mcp.json` is the file Aleph reads to start the server; without it
        // an "mcp" runtime declaration has nothing behind it.
        assert!(target.join(".mcp.json").exists());
        assert!(target.join("package.json").exists());
        assert!(target.join("src/index.mjs").exists());

        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(target.join(".mcp.json")).unwrap())
                .unwrap();
        assert!(
            mcp["mcpServers"]["my-plugin"]["command"].is_string(),
            "the scaffolded .mcp.json must declare a runnable server"
        );
    }

    #[test]
    fn scaffold_wasm_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-wasm");

        scaffold_plugin(&target, "my-wasm", PluginTemplate::Wasm).unwrap();

        assert!(target.join(".claude-plugin/plugin.toml").exists());
        assert!(target.join("Cargo.toml").exists());
        assert!(target.join("src/lib.rs").exists());

        let manifest = std::fs::read_to_string(target.join(".claude-plugin/plugin.toml")).unwrap();
        assert!(manifest.contains(r#"runtime = "wasm""#));
    }

    #[test]
    fn scaffold_static_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-static");

        scaffold_plugin(&target, "my-static", PluginTemplate::Static).unwrap();

        assert!(target.join(".claude-plugin/plugin.toml").exists());
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

        let result = scaffold_plugin(&target, "existing", PluginTemplate::Mcp);
        assert!(result.is_err());
    }

    #[test]
    fn accepts_existing_empty_directory() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("empty");
        std::fs::create_dir_all(&target).unwrap();

        let result = scaffold_plugin(&target, "empty", PluginTemplate::Mcp);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_valid_plugin() {
        let dir = tempdir().unwrap();
        scaffold_plugin(
            dir.path().join("p").as_path(),
            "test",
            PluginTemplate::Static,
        )
        .unwrap();

        let result = validate_plugin_dir(dir.path().join("p").as_path()).unwrap();
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert!(!result.info.is_empty());
    }

    #[test]
    fn validate_missing_manifest() {
        let dir = tempdir().unwrap();
        let result = validate_plugin_dir(dir.path()).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains(".claude-plugin/plugin.toml"));
    }

    /// The check that would have caught the scaffolder: a runtime the host
    /// cannot load must be an error, not a green check.
    #[test]
    fn validate_rejects_an_unloadable_runtime() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.path().join(".claude-plugin/plugin.toml"),
            "name = \"p\"\n\n[aleph]\nruntime = \"nodejs\"\n",
        )
        .unwrap();
        let result = validate_plugin_dir(dir.path()).unwrap();
        assert!(
            result.errors.iter().any(|e| e.contains("nodejs")),
            "expected an unknown-runtime error, got {:?}",
            result.errors
        );
    }

    /// The deprecated dialect still validates, and says so.
    #[test]
    fn validate_warns_on_the_deprecated_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("aleph.plugin.toml"),
            "[plugin]\nid = \"test\"\nkind = \"static\"\n",
        )
        .unwrap();
        let result = validate_plugin_dir(dir.path()).unwrap();
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert!(result.warnings.iter().any(|w| w.contains("deprecated")));
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
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Duplicate tool name: 'foo'")));
    }

    #[test]
    fn validate_nonexistent_directory() {
        let result = validate_plugin_dir(Path::new("/tmp/does-not-exist-aleph-test")).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("does not exist"));
    }

    #[test]
    fn template_from_str() {
        // The `nodejs` family still parses — it is what an author types —
        // but it now names the runtime that exists.
        for alias in ["mcp", "nodejs", "node", "js", "ts"] {
            assert!(
                matches!(
                    alias.parse::<PluginTemplate>().unwrap(),
                    PluginTemplate::Mcp
                ),
                "alias {alias} must map to the MCP template"
            );
        }
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
        scaffold_plugin(&plugin_dir, "p", PluginTemplate::Mcp).unwrap();

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
        assert!(names.iter().any(|n| n.contains("plugin.toml")));
    }

    /// The check must name the directory installs actually land in.
    ///
    /// It named `~/.aleph/extensions` — a path nothing in the tree creates or
    /// reads — so it reported "does not exist" on a machine with plugins
    /// installed. `ALEPH_HOME` is deliberately not set here: `std::env` is
    /// process-global and libtest runs in parallel, so setting it would race
    /// every sibling test that resolves a path. The resolver itself is
    /// covered by `doctor::aleph_home`.
    #[test]
    fn the_plugin_directory_check_names_the_install_directory() {
        let check = check_plugin_dir_exists();
        assert_eq!(check.name, "plugin-dir");
        assert!(
            !check.message.contains("extensions"),
            "expected the installs directory, got: {}",
            check.message
        );
        assert!(
            check.message.contains("plugins/installed")
                || check.message.contains("plugins\\installed"),
            "expected the installs directory, got: {}",
            check.message
        );
    }
}
