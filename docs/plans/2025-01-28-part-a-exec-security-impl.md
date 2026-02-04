# Part A: Tool Security Execution - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement shell command analysis, allowlist matching, approval decision logic, and socket protocol for secure command execution.

**Architecture:** Create a new `exec` module with shell parsing, command analysis, allowlist matching, and approval decision engine. The socket protocol enables UI-based approval dialogs.

**Tech Stack:** Rust, serde, regex, tokio (async), Unix Domain Sockets

---

### Task 1: Create exec module with config types

**Files:**
- Create: `core/src/exec/mod.rs`
- Create: `core/src/exec/config.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create `core/src/exec/mod.rs`**

```rust
//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod config;

pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    SocketConfig,
};
```

**Step 2: Create `core/src/exec/config.rs`**

```rust
//! Configuration types for command execution security.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Security level for command execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecSecurity {
    /// Reject all command execution
    #[default]
    Deny,
    /// Only allow whitelisted commands
    Allowlist,
    /// Allow all commands (full trust)
    Full,
}

/// Ask policy for commands not in allowlist
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecAsk {
    /// Never ask user (use fallback)
    Off,
    /// Ask when command not in allowlist (default)
    #[default]
    OnMiss,
    /// Ask for every execution
    Always,
}

/// Root configuration file for exec approvals
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecApprovalsFile {
    /// Config version (currently 1)
    #[serde(default = "default_version")]
    pub version: u8,

    /// Socket configuration for UI communication
    #[serde(default)]
    pub socket: Option<SocketConfig>,

    /// Default settings for all agents
    #[serde(default)]
    pub defaults: Option<ExecDefaults>,

    /// Per-agent configuration overrides
    #[serde(default)]
    pub agents: HashMap<String, AgentExecConfig>,
}

fn default_version() -> u8 {
    1
}

/// Socket configuration for approval communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketConfig {
    /// Socket path (default: ~/.aleph/exec-approvals.sock)
    #[serde(default)]
    pub path: Option<String>,

    /// Authentication token
    #[serde(default)]
    pub token: Option<String>,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            path: Some("~/.aleph/exec-approvals.sock".into()),
            token: None,
        }
    }
}

/// Default execution settings
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecDefaults {
    /// Security level
    #[serde(default)]
    pub security: Option<ExecSecurity>,

    /// Ask policy
    #[serde(default)]
    pub ask: Option<ExecAsk>,

    /// Fallback when ask is off or times out
    #[serde(default)]
    pub ask_fallback: Option<ExecSecurity>,

    /// Auto-allow commands from skills
    #[serde(default)]
    pub auto_allow_skills: Option<bool>,
}

/// Per-agent execution configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentExecConfig {
    /// Agent-specific defaults (inherits from global defaults)
    #[serde(flatten)]
    pub defaults: ExecDefaults,

    /// Command allowlist for this agent
    #[serde(default)]
    pub allowlist: Option<Vec<AllowlistEntry>>,
}

/// An entry in the command allowlist
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    /// Unique identifier
    #[serde(default)]
    pub id: Option<String>,

    /// Pattern to match (e.g., "/usr/bin/git", "~/bin/*", "git")
    pub pattern: String,

    /// Last time this entry was used (Unix timestamp)
    #[serde(default)]
    pub last_used_at: Option<i64>,

    /// Last command that matched this entry
    #[serde(default)]
    pub last_used_command: Option<String>,

    /// Last resolved path for this entry
    #[serde(default)]
    pub last_resolved_path: Option<String>,
}

impl ExecApprovalsFile {
    /// Get resolved config for an agent
    pub fn resolve_for_agent(&self, agent_id: &str) -> ResolvedExecConfig {
        let global = self.defaults.as_ref();
        let agent = self.agents.get(agent_id);

        let security = agent
            .and_then(|a| a.defaults.security)
            .or_else(|| global.and_then(|g| g.security))
            .unwrap_or_default();

        let ask = agent
            .and_then(|a| a.defaults.ask)
            .or_else(|| global.and_then(|g| g.ask))
            .unwrap_or_default();

        let ask_fallback = agent
            .and_then(|a| a.defaults.ask_fallback)
            .or_else(|| global.and_then(|g| g.ask_fallback))
            .unwrap_or(ExecSecurity::Deny);

        let auto_allow_skills = agent
            .and_then(|a| a.defaults.auto_allow_skills)
            .or_else(|| global.and_then(|g| g.auto_allow_skills))
            .unwrap_or(false);

        let allowlist = agent
            .and_then(|a| a.allowlist.clone())
            .unwrap_or_default();

        ResolvedExecConfig {
            security,
            ask,
            ask_fallback,
            auto_allow_skills,
            allowlist,
        }
    }
}

/// Resolved execution configuration with all defaults applied
#[derive(Debug, Clone)]
pub struct ResolvedExecConfig {
    pub security: ExecSecurity,
    pub ask: ExecAsk,
    pub ask_fallback: ExecSecurity,
    pub auto_allow_skills: bool,
    pub allowlist: Vec<AllowlistEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_security_default() {
        assert_eq!(ExecSecurity::default(), ExecSecurity::Deny);
    }

    #[test]
    fn test_exec_ask_default() {
        assert_eq!(ExecAsk::default(), ExecAsk::OnMiss);
    }

    #[test]
    fn test_config_deserialize() {
        let toml_str = r#"
            version = 1

            [defaults]
            security = "allowlist"
            ask = "on-miss"

            [agents.main]
            security = "full"

            [[agents.main.allowlist]]
            pattern = "/usr/bin/git"
        "#;

        let config: ExecApprovalsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version, 1);
        assert!(config.agents.contains_key("main"));
    }

    #[test]
    fn test_resolve_for_agent() {
        let mut config = ExecApprovalsFile::default();
        config.defaults = Some(ExecDefaults {
            security: Some(ExecSecurity::Allowlist),
            ask: Some(ExecAsk::OnMiss),
            ..Default::default()
        });

        let mut agent_config = AgentExecConfig::default();
        agent_config.defaults.security = Some(ExecSecurity::Full);
        config.agents.insert("work".to_string(), agent_config);

        // Global defaults
        let main_resolved = config.resolve_for_agent("main");
        assert_eq!(main_resolved.security, ExecSecurity::Allowlist);

        // Agent override
        let work_resolved = config.resolve_for_agent("work");
        assert_eq!(work_resolved.security, ExecSecurity::Full);
    }
}
```

**Step 3: Add exec module to lib.rs**

Find `pub mod tools;` in `core/src/lib.rs` and add after it:

```rust
pub mod exec; // Command execution security
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check 2>&1 | head -30`

**Step 5: Commit**

```bash
git add core/src/exec/mod.rs core/src/exec/config.rs core/src/lib.rs
git commit -m "exec: add configuration types for command execution security"
```

---

### Task 2: Add command analysis types

**Files:**
- Create: `core/src/exec/analysis.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: Create `core/src/exec/analysis.rs`**

```rust
//! Command analysis structures.
//!
//! Represents parsed and analyzed shell commands for security decisions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Result of analyzing a shell command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAnalysis {
    /// Whether the command was successfully parsed
    pub ok: bool,

    /// Error reason if parsing failed
    pub reason: Option<String>,

    /// All command segments (flattened from all chains/pipelines)
    pub segments: Vec<CommandSegment>,

    /// Commands grouped by chain operators (&&, ||, ;)
    /// Each inner Vec is a pipeline (commands connected by |)
    pub chains: Option<Vec<Vec<CommandSegment>>>,
}

impl CommandAnalysis {
    /// Create a successful analysis with segments
    pub fn success(segments: Vec<CommandSegment>, chains: Vec<Vec<CommandSegment>>) -> Self {
        Self {
            ok: true,
            reason: None,
            segments,
            chains: Some(chains),
        }
    }

    /// Create a failed analysis with error reason
    pub fn error(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            reason: Some(reason.into()),
            segments: Vec::new(),
            chains: None,
        }
    }

    /// Get all executable names from segments
    pub fn executables(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|s| s.resolution.as_ref())
            .map(|r| r.executable_name.as_str())
            .collect()
    }
}

/// A single command segment (one command in a pipeline)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSegment {
    /// Raw command string
    pub raw: String,

    /// Tokenized arguments
    pub argv: Vec<String>,

    /// Resolution information (if executable was found)
    pub resolution: Option<CommandResolution>,
}

impl CommandSegment {
    /// Create a new segment with raw string and argv
    pub fn new(raw: impl Into<String>, argv: Vec<String>) -> Self {
        Self {
            raw: raw.into(),
            argv,
            resolution: None,
        }
    }

    /// Set resolution
    pub fn with_resolution(mut self, resolution: CommandResolution) -> Self {
        self.resolution = Some(resolution);
        self
    }

    /// Get the executable name (first argv element)
    pub fn executable(&self) -> Option<&str> {
        self.argv.first().map(|s| s.as_str())
    }
}

/// Resolution information for an executable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResolution {
    /// Raw executable as specified (e.g., "git", "./script.sh", "/usr/bin/ls")
    pub raw_executable: String,

    /// Fully resolved path (if found in PATH)
    pub resolved_path: Option<PathBuf>,

    /// Executable name (basename without path)
    pub executable_name: String,
}

impl CommandResolution {
    /// Create a resolution for an executable found in PATH
    pub fn found(raw: impl Into<String>, path: PathBuf) -> Self {
        let raw = raw.into();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&raw)
            .to_string();

        Self {
            raw_executable: raw,
            resolved_path: Some(path),
            executable_name: name,
        }
    }

    /// Create a resolution for an executable not found in PATH
    pub fn not_found(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let name = raw
            .rsplit('/')
            .next()
            .unwrap_or(&raw)
            .to_string();

        Self {
            raw_executable: raw,
            resolved_path: None,
            executable_name: name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_success() {
        let segment = CommandSegment::new("ls -la", vec!["ls".into(), "-la".into()]);
        let analysis = CommandAnalysis::success(vec![segment.clone()], vec![vec![segment]]);

        assert!(analysis.ok);
        assert!(analysis.reason.is_none());
        assert_eq!(analysis.segments.len(), 1);
    }

    #[test]
    fn test_analysis_error() {
        let analysis = CommandAnalysis::error("parse failed");

        assert!(!analysis.ok);
        assert_eq!(analysis.reason, Some("parse failed".to_string()));
    }

    #[test]
    fn test_segment_executable() {
        let segment = CommandSegment::new("git status", vec!["git".into(), "status".into()]);
        assert_eq!(segment.executable(), Some("git"));
    }

    #[test]
    fn test_resolution_found() {
        let res = CommandResolution::found("git", PathBuf::from("/usr/bin/git"));
        assert_eq!(res.executable_name, "git");
        assert!(res.resolved_path.is_some());
    }

    #[test]
    fn test_resolution_not_found() {
        let res = CommandResolution::not_found("./my-script.sh");
        assert_eq!(res.executable_name, "my-script.sh");
        assert!(res.resolved_path.is_none());
    }

    #[test]
    fn test_executables() {
        let seg1 = CommandSegment::new("ls", vec!["ls".into()])
            .with_resolution(CommandResolution::found("ls", PathBuf::from("/bin/ls")));
        let seg2 = CommandSegment::new("grep", vec!["grep".into()])
            .with_resolution(CommandResolution::found("grep", PathBuf::from("/usr/bin/grep")));

        let analysis = CommandAnalysis::success(vec![seg1, seg2], vec![]);
        let executables = analysis.executables();

        assert_eq!(executables, vec!["ls", "grep"]);
    }
}
```

**Step 2: Update mod.rs**

```rust
//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod analysis;
pub mod config;

pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib exec 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/exec/analysis.rs core/src/exec/mod.rs
git commit -m "exec: add command analysis structures"
```

---

### Task 3: Implement shell command parser

**Files:**
- Create: `core/src/exec/parser.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: Create `core/src/exec/parser.rs`**

```rust
//! Shell command parser.
//!
//! Quote-aware parsing supporting pipes, chain operators, and escapes.

use super::analysis::{CommandAnalysis, CommandResolution, CommandSegment};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Characters that indicate unsafe command constructs
const DISALLOWED_CHARS: &[char] = &['`', '\n', '\r'];

/// Analyze a shell command
pub fn analyze_shell_command(
    command: &str,
    cwd: Option<&Path>,
    env: Option<&HashMap<String, String>>,
) -> CommandAnalysis {
    // Check for disallowed characters
    if command.chars().any(|c| DISALLOWED_CHARS.contains(&c)) {
        return CommandAnalysis::error("command contains disallowed characters");
    }

    // Split by chain operators (&&, ||, ;)
    let chain_parts = match split_command_chain(command) {
        Ok(parts) => parts,
        Err(reason) => return CommandAnalysis::error(reason),
    };

    let mut all_segments = Vec::new();
    let mut chains = Vec::new();

    for part in chain_parts {
        // Split by pipe |
        let pipeline_parts = match split_pipeline(&part) {
            Ok(parts) => parts,
            Err(reason) => return CommandAnalysis::error(reason),
        };

        let mut chain_segments = Vec::new();
        for raw in pipeline_parts {
            let argv = match tokenize_segment(&raw) {
                Some(tokens) if !tokens.is_empty() => tokens,
                Some(_) => continue, // Empty segment
                None => return CommandAnalysis::error("unable to parse command segment"),
            };

            let resolution = resolve_executable(&argv[0], cwd, env);
            let segment = CommandSegment::new(raw, argv).with_resolution(resolution);
            chain_segments.push(segment);
        }

        if !chain_segments.is_empty() {
            all_segments.extend(chain_segments.clone());
            chains.push(chain_segments);
        }
    }

    if all_segments.is_empty() {
        return CommandAnalysis::error("no valid command segments found");
    }

    CommandAnalysis::success(all_segments, chains)
}

/// Split command by chain operators (&&, ||, ;)
fn split_command_chain(command: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
                current.push(ch);
            }
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                    current.clear();
                } else {
                    // Background operator not allowed
                    return Err("background operator (&) not allowed".into());
                }
            }
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                    current.clear();
                } else {
                    // Single pipe is OK, keep in current
                    current.push(ch);
                }
            }
            ';' if !in_single && !in_double => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if in_single || in_double || escaped {
        return Err("unclosed quote or trailing escape".into());
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }

    Ok(parts)
}

/// Split a command chain part by pipe |
fn split_pipeline(command: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
                current.push(ch);
            }
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '|' if !in_single && !in_double => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if in_single || in_double || escaped {
        return Err("unclosed quote or trailing escape in pipeline".into());
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }

    Ok(parts)
}

/// Tokenize a single command segment
pub fn tokenize_segment(segment: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in segment.chars() {
        if escaped {
            buf.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !buf.is_empty() {
                    tokens.push(std::mem::take(&mut buf));
                }
            }
            c => {
                buf.push(c);
            }
        }
    }

    if escaped || in_single || in_double {
        return None;
    }

    if !buf.is_empty() {
        tokens.push(buf);
    }

    Some(tokens)
}

/// Resolve an executable to its full path
fn resolve_executable(
    executable: &str,
    cwd: Option<&Path>,
    env: Option<&HashMap<String, String>>,
) -> CommandResolution {
    // Absolute path
    if executable.starts_with('/') {
        let path = PathBuf::from(executable);
        if path.exists() {
            return CommandResolution::found(executable, path);
        }
        return CommandResolution::not_found(executable);
    }

    // Relative path
    if executable.starts_with("./") || executable.starts_with("../") {
        if let Some(cwd) = cwd {
            let path = cwd.join(executable);
            if path.exists() {
                return CommandResolution::found(executable, path);
            }
        }
        return CommandResolution::not_found(executable);
    }

    // Search PATH
    let path_var = env
        .and_then(|e| e.get("PATH"))
        .map(|s| s.as_str())
        .or_else(|| std::env::var("PATH").ok().as_deref().map(|_| ""))
        .unwrap_or("");

    // Use system PATH if env doesn't have it
    let actual_path = if path_var.is_empty() {
        std::env::var("PATH").unwrap_or_default()
    } else {
        path_var.to_string()
    };

    for dir in actual_path.split(':') {
        let path = PathBuf::from(dir).join(executable);
        if path.exists() {
            return CommandResolution::found(executable, path);
        }
    }

    CommandResolution::not_found(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize_segment("ls -la").unwrap();
        assert_eq!(tokens, vec!["ls", "-la"]);
    }

    #[test]
    fn test_tokenize_single_quotes() {
        let tokens = tokenize_segment("echo 'hello world'").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenize_double_quotes() {
        let tokens = tokenize_segment(r#"echo "hello world""#).unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenize_escaped() {
        let tokens = tokenize_segment(r"echo hello\ world").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenize_unclosed_quote() {
        assert!(tokenize_segment("echo 'hello").is_none());
    }

    #[test]
    fn test_split_pipeline() {
        let parts = split_pipeline("ls | grep foo | wc -l").unwrap();
        assert_eq!(parts, vec!["ls", "grep foo", "wc -l"]);
    }

    #[test]
    fn test_split_chain_and() {
        let parts = split_command_chain("cd /tmp && ls").unwrap();
        assert_eq!(parts, vec!["cd /tmp", "ls"]);
    }

    #[test]
    fn test_split_chain_or() {
        let parts = split_command_chain("test -f foo || echo missing").unwrap();
        assert_eq!(parts, vec!["test -f foo", "echo missing"]);
    }

    #[test]
    fn test_split_chain_semicolon() {
        let parts = split_command_chain("echo a; echo b").unwrap();
        assert_eq!(parts, vec!["echo a", "echo b"]);
    }

    #[test]
    fn test_background_operator_rejected() {
        let result = split_command_chain("sleep 10 &");
        assert!(result.is_err());
    }

    #[test]
    fn test_analyze_simple() {
        let analysis = analyze_shell_command("ls -la", None, None);
        assert!(analysis.ok);
        assert_eq!(analysis.segments.len(), 1);
        assert_eq!(analysis.segments[0].argv, vec!["ls", "-la"]);
    }

    #[test]
    fn test_analyze_pipeline() {
        let analysis = analyze_shell_command("cat file.txt | grep foo | wc -l", None, None);
        assert!(analysis.ok);
        assert_eq!(analysis.segments.len(), 3);
    }

    #[test]
    fn test_analyze_disallowed_backtick() {
        let analysis = analyze_shell_command("echo `whoami`", None, None);
        assert!(!analysis.ok);
    }

    #[test]
    fn test_analyze_complex() {
        let analysis = analyze_shell_command("cd /tmp && ls | grep foo; echo done", None, None);
        assert!(analysis.ok);
        assert_eq!(analysis.chains.as_ref().unwrap().len(), 3);
    }
}
```

**Step 2: Update mod.rs**

```rust
//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod analysis;
pub mod config;
pub mod parser;

pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
pub use parser::{analyze_shell_command, tokenize_segment};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib exec 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/exec/parser.rs core/src/exec/mod.rs
git commit -m "exec: add quote-aware shell command parser"
```

---

### Task 4: Implement allowlist matching

**Files:**
- Create: `core/src/exec/allowlist.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: Create `core/src/exec/allowlist.rs`**

```rust
//! Allowlist pattern matching for executables.

use super::analysis::CommandResolution;
use super::config::AllowlistEntry;
use std::path::Path;

/// Check if a resolution matches any allowlist entry
pub fn match_allowlist<'a>(
    allowlist: &'a [AllowlistEntry],
    resolution: &CommandResolution,
) -> Option<&'a AllowlistEntry> {
    for entry in allowlist {
        if matches_entry(entry, resolution) {
            return Some(entry);
        }
    }
    None
}

/// Check if a resolution matches a single entry
fn matches_entry(entry: &AllowlistEntry, resolution: &CommandResolution) -> bool {
    let pattern = &entry.pattern;

    // Exact executable name match (e.g., "git")
    if !pattern.contains('/') && !pattern.contains('*') {
        return resolution.executable_name.eq_ignore_ascii_case(pattern);
    }

    // Wildcard pattern (e.g., "~/bin/*", "/usr/local/bin/*")
    if pattern.ends_with("/*") {
        let dir_pattern = &pattern[..pattern.len() - 2];
        let expanded = expand_home(dir_pattern);

        if let Some(resolved) = &resolution.resolved_path {
            if let Some(parent) = resolved.parent() {
                return parent.to_string_lossy().eq_ignore_ascii_case(&expanded);
            }
        }
        return false;
    }

    // Glob pattern with * in middle (e.g., "git-*")
    if pattern.contains('*') {
        return glob_match(pattern, &resolution.executable_name)
            || resolution
                .resolved_path
                .as_ref()
                .map(|p| glob_match(pattern, &p.to_string_lossy()))
                .unwrap_or(false);
    }

    // Absolute or relative path match
    let expanded = expand_home(pattern);
    if let Some(resolved) = &resolution.resolved_path {
        return resolved.to_string_lossy().eq_ignore_ascii_case(&expanded);
    }

    // Raw executable match
    resolution.raw_executable.eq_ignore_ascii_case(&expanded)
}

/// Expand ~ to home directory
fn expand_home(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &path[1..]);
        }
    }
    path.to_string()
}

/// Simple glob matching with * wildcard
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let text = text.to_lowercase();

    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        return pattern == text;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if let Some(found) = text[pos..].find(part) {
            if i == 0 && found != 0 {
                return false; // First part must match at start
            }
            pos += found + part.len();
        } else {
            return false;
        }
    }

    // Last part must match at end
    if let Some(last) = parts.last() {
        if !last.is_empty() && !text.ends_with(last) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(pattern: &str) -> AllowlistEntry {
        AllowlistEntry {
            id: None,
            pattern: pattern.to_string(),
            last_used_at: None,
            last_used_command: None,
            last_resolved_path: None,
        }
    }

    fn resolution(name: &str, path: Option<&str>) -> CommandResolution {
        CommandResolution {
            raw_executable: name.to_string(),
            resolved_path: path.map(PathBuf::from),
            executable_name: name.to_string(),
        }
    }

    #[test]
    fn test_exact_name_match() {
        let entries = vec![entry("git")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_exact_name_case_insensitive() {
        let entries = vec![entry("Git")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_exact_path_match() {
        let entries = vec![entry("/usr/bin/git")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_directory_wildcard() {
        let entries = vec![entry("/usr/bin/*")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_directory_wildcard_no_match() {
        let entries = vec![entry("/usr/local/bin/*")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_none());
    }

    #[test]
    fn test_glob_pattern() {
        let entries = vec![entry("git-*")];
        let res = resolution("git-rebase", Some("/usr/bin/git-rebase"));

        assert!(match_allowlist(&entries, &res).is_some());
    }

    #[test]
    fn test_no_match() {
        let entries = vec![entry("npm")];
        let res = resolution("git", Some("/usr/bin/git"));

        assert!(match_allowlist(&entries, &res).is_none());
    }

    #[test]
    fn test_glob_match_simple() {
        assert!(glob_match("git-*", "git-rebase"));
        assert!(glob_match("*-test", "my-test"));
        assert!(glob_match("foo*bar", "fooxyzbar"));
        assert!(!glob_match("git-*", "npm"));
    }
}
```

**Step 2: Update mod.rs**

```rust
//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod allowlist;
pub mod analysis;
pub mod config;
pub mod parser;

pub use allowlist::match_allowlist;
pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
pub use parser::{analyze_shell_command, tokenize_segment};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib exec 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/exec/allowlist.rs core/src/exec/mod.rs
git commit -m "exec: add allowlist pattern matching"
```

---

### Task 5: Implement approval decision logic

**Files:**
- Create: `core/src/exec/decision.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: Create `core/src/exec/decision.rs`**

```rust
//! Approval decision logic for command execution.

use super::allowlist::match_allowlist;
use super::analysis::{CommandAnalysis, CommandSegment};
use super::config::{ExecAsk, ExecSecurity, ResolvedExecConfig};

/// Default safe binaries (read-only operations)
pub const DEFAULT_SAFE_BINS: &[&str] = &[
    "jq", "grep", "cut", "sort", "uniq", "head", "tail", "tr", "wc", "cat", "echo", "pwd", "ls",
    "which", "env", "date", "true", "false", "test", "basename", "dirname", "realpath", "stat",
    "file", "diff", "comm", "tee", "xargs", "seq", "printf",
];

/// Decision result for command execution
#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    /// Allow execution
    Allow,
    /// Deny execution with reason
    Deny { reason: String },
    /// Need user approval
    NeedApproval { request: ApprovalRequest },
}

/// Request for user approval
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Unique request ID
    pub id: String,
    /// Full command string
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Command analysis result
    pub analysis: CommandAnalysis,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
}

/// Context for execution decision
#[derive(Debug, Clone)]
pub struct ExecContext {
    pub agent_id: String,
    pub session_key: String,
    pub cwd: Option<String>,
    pub command: String,
    /// Whether this command is from a skill
    pub from_skill: bool,
}

/// Decide whether to allow command execution
pub fn decide_exec_approval(
    config: &ResolvedExecConfig,
    analysis: &CommandAnalysis,
    context: &ExecContext,
) -> ApprovalDecision {
    // 1. Analysis must be OK
    if !analysis.ok {
        return ApprovalDecision::Deny {
            reason: analysis
                .reason
                .clone()
                .unwrap_or_else(|| "command parse error".into()),
        };
    }

    // 2. Check security level
    match config.security {
        ExecSecurity::Deny => {
            return ApprovalDecision::Deny {
                reason: "command execution denied by security policy".into(),
            };
        }
        ExecSecurity::Full => {
            return ApprovalDecision::Allow;
        }
        ExecSecurity::Allowlist => { /* continue checking */ }
    }

    // 3. Auto-allow skills if configured
    if config.auto_allow_skills && context.from_skill {
        return ApprovalDecision::Allow;
    }

    // 4. Check all segments
    for segment in &analysis.segments {
        match check_segment(config, segment) {
            SegmentDecision::Allow => continue,
            SegmentDecision::NeedApproval => {
                // Check ask policy
                if config.ask == ExecAsk::Off {
                    return apply_fallback(config.ask_fallback);
                }
                return ApprovalDecision::NeedApproval {
                    request: build_approval_request(analysis, context),
                };
            }
            SegmentDecision::Deny(reason) => {
                return ApprovalDecision::Deny { reason };
            }
        }
    }

    // 5. Check if ask=always
    if config.ask == ExecAsk::Always {
        return ApprovalDecision::NeedApproval {
            request: build_approval_request(analysis, context),
        };
    }

    ApprovalDecision::Allow
}

/// Decision for a single segment
enum SegmentDecision {
    Allow,
    NeedApproval,
    Deny(String),
}

/// Check a single command segment
fn check_segment(config: &ResolvedExecConfig, segment: &CommandSegment) -> SegmentDecision {
    let Some(resolution) = &segment.resolution else {
        return SegmentDecision::NeedApproval;
    };

    // Check safe bins (with argument restrictions)
    if is_safe_bin_usage(&resolution.executable_name, &segment.argv) {
        return SegmentDecision::Allow;
    }

    // Check allowlist
    if match_allowlist(&config.allowlist, resolution).is_some() {
        return SegmentDecision::Allow;
    }

    SegmentDecision::NeedApproval
}

/// Check if command uses a safe binary without dangerous arguments
fn is_safe_bin_usage(executable: &str, argv: &[String]) -> bool {
    if !DEFAULT_SAFE_BINS
        .iter()
        .any(|b| b.eq_ignore_ascii_case(executable))
    {
        return false;
    }

    // Arguments must not contain file paths or redirections
    for arg in argv.iter().skip(1) {
        // Skip flags
        if arg.starts_with('-') {
            continue;
        }
        // Disallow paths
        if arg.contains('/') || arg.contains('\\') {
            return false;
        }
    }

    true
}

/// Apply fallback security level
fn apply_fallback(fallback: ExecSecurity) -> ApprovalDecision {
    match fallback {
        ExecSecurity::Full => ApprovalDecision::Allow,
        _ => ApprovalDecision::Deny {
            reason: "approval required but ask is disabled".into(),
        },
    }
}

/// Build an approval request
fn build_approval_request(analysis: &CommandAnalysis, context: &ExecContext) -> ApprovalRequest {
    ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        command: context.command.clone(),
        cwd: context.cwd.clone(),
        analysis: analysis.clone(),
        agent_id: context.agent_id.clone(),
        session_key: context.session_key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::parser::analyze_shell_command;

    fn default_config() -> ResolvedExecConfig {
        ResolvedExecConfig {
            security: ExecSecurity::Allowlist,
            ask: ExecAsk::OnMiss,
            ask_fallback: ExecSecurity::Deny,
            auto_allow_skills: false,
            allowlist: vec![],
        }
    }

    fn context(command: &str) -> ExecContext {
        ExecContext {
            agent_id: "main".into(),
            session_key: "agent:main:main".into(),
            cwd: None,
            command: command.into(),
            from_skill: false,
        }
    }

    #[test]
    fn test_deny_policy() {
        let config = ResolvedExecConfig {
            security: ExecSecurity::Deny,
            ..default_config()
        };
        let analysis = analyze_shell_command("ls", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("ls"));

        assert!(matches!(decision, ApprovalDecision::Deny { .. }));
    }

    #[test]
    fn test_full_policy() {
        let config = ResolvedExecConfig {
            security: ExecSecurity::Full,
            ..default_config()
        };
        let analysis = analyze_shell_command("rm -rf /", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("rm -rf /"));

        assert!(matches!(decision, ApprovalDecision::Allow));
    }

    #[test]
    fn test_safe_bin_allowed() {
        let config = default_config();
        let analysis = analyze_shell_command("echo hello", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("echo hello"));

        assert!(matches!(decision, ApprovalDecision::Allow));
    }

    #[test]
    fn test_safe_bin_with_path_needs_approval() {
        let config = default_config();
        let analysis = analyze_shell_command("cat /etc/passwd", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("cat /etc/passwd"));

        assert!(matches!(decision, ApprovalDecision::NeedApproval { .. }));
    }

    #[test]
    fn test_unknown_command_needs_approval() {
        let config = default_config();
        let analysis = analyze_shell_command("npm install", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("npm install"));

        assert!(matches!(decision, ApprovalDecision::NeedApproval { .. }));
    }

    #[test]
    fn test_ask_off_uses_fallback() {
        let config = ResolvedExecConfig {
            ask: ExecAsk::Off,
            ask_fallback: ExecSecurity::Deny,
            ..default_config()
        };
        let analysis = analyze_shell_command("npm install", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("npm install"));

        assert!(matches!(decision, ApprovalDecision::Deny { .. }));
    }

    #[test]
    fn test_auto_allow_skills() {
        let config = ResolvedExecConfig {
            auto_allow_skills: true,
            ..default_config()
        };
        let analysis = analyze_shell_command("npm install", None, None);
        let mut ctx = context("npm install");
        ctx.from_skill = true;
        let decision = decide_exec_approval(&config, &analysis, &ctx);

        assert!(matches!(decision, ApprovalDecision::Allow));
    }

    #[test]
    fn test_is_safe_bin_usage() {
        assert!(is_safe_bin_usage("echo", &["echo".into(), "hello".into()]));
        assert!(is_safe_bin_usage("ls", &["ls".into(), "-la".into()]));
        assert!(!is_safe_bin_usage("cat", &["cat".into(), "/etc/passwd".into()]));
        assert!(!is_safe_bin_usage("npm", &["npm".into(), "install".into()]));
    }
}
```

**Step 2: Update mod.rs**

```rust
//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod allowlist;
pub mod analysis;
pub mod config;
pub mod decision;
pub mod parser;

pub use allowlist::match_allowlist;
pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
pub use decision::{
    decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext, DEFAULT_SAFE_BINS,
};
pub use parser::{analyze_shell_command, tokenize_segment};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib exec 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/exec/decision.rs core/src/exec/mod.rs
git commit -m "exec: add approval decision logic with safe bins"
```

---

### Task 6: Add socket protocol types

**Files:**
- Create: `core/src/exec/socket.rs`
- Modify: `core/src/exec/mod.rs`

**Step 1: Create `core/src/exec/socket.rs`**

```rust
//! Socket protocol for approval communication.
//!
//! Defines the JSON message format for UI-Core communication.

use serde::{Deserialize, Serialize};

/// Message sent over the approval socket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketMessage {
    /// Request for approval (Core -> UI)
    Request {
        /// Authentication token
        token: String,
        /// Request ID
        id: String,
        /// Request payload
        request: ApprovalRequestPayload,
    },

    /// Decision response (UI -> Core)
    Decision {
        /// Request ID being answered
        id: String,
        /// The decision
        decision: ApprovalDecisionType,
    },

    /// Error message
    Error {
        /// Error message
        message: String,
    },
}

/// Payload for an approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestPayload {
    /// Full command string
    pub command: String,

    /// Working directory
    pub cwd: Option<String>,

    /// Agent ID
    pub agent_id: String,

    /// Session key
    pub session_key: String,

    /// Primary executable name
    pub executable: String,

    /// Resolved path (if found)
    pub resolved_path: Option<String>,

    /// All command segments
    pub segments: Vec<SegmentInfo>,
}

/// Information about a command segment for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    /// Raw command text
    pub raw: String,

    /// Executable name
    pub executable: String,

    /// Resolved path
    pub resolved_path: Option<String>,

    /// Arguments (excluding executable)
    pub args: Vec<String>,
}

/// Type of approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecisionType {
    /// Allow this execution only
    AllowOnce,
    /// Allow and add to allowlist
    AllowAlways,
    /// Deny execution
    Deny,
}

impl ApprovalRequestPayload {
    /// Create from approval request
    pub fn from_request(request: &super::decision::ApprovalRequest) -> Self {
        let segments: Vec<SegmentInfo> = request
            .analysis
            .segments
            .iter()
            .map(|s| SegmentInfo {
                raw: s.raw.clone(),
                executable: s
                    .resolution
                    .as_ref()
                    .map(|r| r.executable_name.clone())
                    .unwrap_or_else(|| s.argv.first().cloned().unwrap_or_default()),
                resolved_path: s.resolution.as_ref().and_then(|r| {
                    r.resolved_path.as_ref().map(|p| p.to_string_lossy().into())
                }),
                args: s.argv.iter().skip(1).cloned().collect(),
            })
            .collect();

        let primary = segments.first();

        Self {
            command: request.command.clone(),
            cwd: request.cwd.clone(),
            agent_id: request.agent_id.clone(),
            session_key: request.session_key.clone(),
            executable: primary.map(|s| s.executable.clone()).unwrap_or_default(),
            resolved_path: primary.and_then(|s| s.resolved_path.clone()),
            segments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_message_request_serialize() {
        let msg = SocketMessage::Request {
            token: "secret".into(),
            id: "req-123".into(),
            request: ApprovalRequestPayload {
                command: "npm install".into(),
                cwd: Some("/project".into()),
                agent_id: "main".into(),
                session_key: "agent:main:main".into(),
                executable: "npm".into(),
                resolved_path: Some("/usr/bin/npm".into()),
                segments: vec![SegmentInfo {
                    raw: "npm install".into(),
                    executable: "npm".into(),
                    resolved_path: Some("/usr/bin/npm".into()),
                    args: vec!["install".into()],
                }],
            },
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"request""#));
        assert!(json.contains(r#""token":"secret""#));
    }

    #[test]
    fn test_socket_message_decision_serialize() {
        let msg = SocketMessage::Decision {
            id: "req-123".into(),
            decision: ApprovalDecisionType::AllowOnce,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"decision""#));
        assert!(json.contains(r#""decision":"allow-once""#));
    }

    #[test]
    fn test_socket_message_deserialize() {
        let json = r#"{"type":"decision","id":"req-123","decision":"allow-always"}"#;
        let msg: SocketMessage = serde_json::from_str(json).unwrap();

        assert!(matches!(
            msg,
            SocketMessage::Decision {
                decision: ApprovalDecisionType::AllowAlways,
                ..
            }
        ));
    }

    #[test]
    fn test_approval_decision_types() {
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowOnce).unwrap(),
            r#""allow-once""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::AllowAlways).unwrap(),
            r#""allow-always""#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalDecisionType::Deny).unwrap(),
            r#""deny""#
        );
    }
}
```

**Step 2: Update mod.rs**

```rust
//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod allowlist;
pub mod analysis;
pub mod config;
pub mod decision;
pub mod parser;
pub mod socket;

pub use allowlist::match_allowlist;
pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
pub use decision::{
    decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext, DEFAULT_SAFE_BINS,
};
pub use parser::{analyze_shell_command, tokenize_segment};
pub use socket::{ApprovalDecisionType, ApprovalRequestPayload, SegmentInfo, SocketMessage};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib exec 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/exec/socket.rs core/src/exec/mod.rs
git commit -m "exec: add socket protocol types for approval communication"
```

---

### Task 7: Full test pass and module exports

**Files:**
- Modify: `core/src/lib.rs`

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib exec 2>&1 | tail -50`

**Step 2: Add exports to lib.rs**

Find the tools exports section and add exec exports:

```rust
// Exec security exports (command execution approval)
pub use crate::exec::{
    // Config
    ExecSecurity, ExecAsk, ExecApprovalsFile, ExecDefaults, AgentExecConfig,
    AllowlistEntry, SocketConfig, ResolvedExecConfig,
    // Analysis
    CommandAnalysis, CommandSegment, CommandResolution,
    // Parser
    analyze_shell_command, tokenize_segment,
    // Allowlist
    match_allowlist,
    // Decision
    decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext, DEFAULT_SAFE_BINS,
    // Socket
    SocketMessage, ApprovalRequestPayload, SegmentInfo, ApprovalDecisionType,
};
```

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check 2>&1 | tail -20`

**Step 4: Final commit**

```bash
git add core/src/lib.rs
git commit -m "exec: export command execution security types from lib.rs"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Config types | `exec/mod.rs`, `config.rs`, `lib.rs` |
| 2 | Analysis structures | `analysis.rs` |
| 3 | Shell parser | `parser.rs` |
| 4 | Allowlist matching | `allowlist.rs` |
| 5 | Decision logic | `decision.rs` |
| 6 | Socket protocol | `socket.rs` |
| 7 | Full test + exports | `lib.rs` |

**Note:** This implementation provides the types, parsing, and decision logic. The actual socket server and bash tool integration will be implemented when the gateway infrastructure is ready.
