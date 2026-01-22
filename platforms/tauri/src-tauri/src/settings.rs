use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::error::{AetherError, Result};

/// Complete settings structure matching frontend types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub general: GeneralSettings,
    pub shortcuts: ShortcutSettings,
    pub behavior: BehaviorSettings,
    pub providers: ProvidersSettings,
    pub generation: GenerationSettings,
    #[serde(default, rename = "generationProviders")]
    pub generation_providers: GenerationProvidersSettings,
    pub memory: MemorySettings,
    pub mcp: McpSettings,
    pub plugins: PluginsSettings,
    pub skills: SkillsSettings,
    pub agent: AgentSettings,
    pub search: SearchSettings,
    pub policies: PoliciesSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub sound_enabled: bool,
    pub launch_at_login: bool,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutSettings {
    pub show_halo: String,
    pub command_completion: String,
    pub toggle_listening: String,
    pub quick_capture: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorSettings {
    pub output_mode: String,
    pub typing_speed: u32,
    pub auto_dismiss_delay: u32,
    pub show_notifications: bool,
    pub pii_masking: bool,
    pub pii_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub enabled: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersSettings {
    pub providers: Vec<ProviderConfig>,
    pub default_provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationSettings {
    pub temperature: f32,
    pub max_tokens: u32,
    pub top_p: f32,
    pub frequency_penalty: f32,
    pub presence_penalty: f32,
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderConfig {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub category: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub enabled: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProvidersSettings {
    pub providers: Vec<GenerationProviderConfig>,
    pub default_image_provider_id: String,
    pub default_video_provider_id: String,
    pub default_audio_provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySettings {
    pub enabled: bool,
    pub auto_save: bool,
    pub max_history: u32,
    pub embedding_model: String,
    pub similarity_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSettings {
    pub servers: Vec<McpServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub source_url: Option<String>,
    pub enabled: bool,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsSettings {
    pub plugins: Vec<Plugin>,
    pub auto_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub trigger_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsSettings {
    pub skills: Vec<Skill>,
}

/// File operations configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub denied_paths: Vec<String>,
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    #[serde(default = "default_true")]
    pub require_confirmation_for_write: bool,
    #[serde(default = "default_true")]
    pub require_confirmation_for_delete: bool,
}

/// Code execution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_runtime")]
    pub default_runtime: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub sandbox_enabled: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub allowed_runtimes: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default = "default_pass_env")]
    pub pass_env: Vec<String>,
    #[serde(default = "default_blocked_commands")]
    pub blocked_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    #[serde(default)]
    pub file_ops: FileOpsConfig,
    #[serde(default)]
    pub code_exec: CodeExecConfig,
    #[serde(default = "default_true")]
    pub web_browsing: bool,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

// Default value functions
fn default_true() -> bool {
    true
}

fn default_max_file_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

fn default_runtime() -> String {
    "shell".to_string()
}

fn default_timeout() -> u64 {
    60
}

fn default_pass_env() -> Vec<String> {
    vec!["PATH".to_string(), "HOME".to_string(), "USER".to_string()]
}

fn default_blocked_commands() -> Vec<String> {
    vec![
        "rm -rf".to_string(),
        "format".to_string(),
        "del /f".to_string(),
    ]
}

fn default_max_iterations() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSettings {
    pub web_search_enabled: bool,
    pub search_engine: String,
    pub max_results: u32,
    pub safe_search: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoliciesSettings {
    pub content_filter: bool,
    pub filter_level: String,
    pub log_conversations: bool,
    pub data_retention_days: u32,
    pub allow_analytics: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: GeneralSettings::default(),
            shortcuts: ShortcutSettings::default(),
            behavior: BehaviorSettings::default(),
            providers: ProvidersSettings::default(),
            generation: GenerationSettings::default(),
            generation_providers: GenerationProvidersSettings::default(),
            memory: MemorySettings::default(),
            mcp: McpSettings::default(),
            plugins: PluginsSettings::default(),
            skills: SkillsSettings::default(),
            agent: AgentSettings::default(),
            search: SearchSettings::default(),
            policies: PoliciesSettings::default(),
        }
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            launch_at_login: false,
            language: "system".to_string(),
        }
    }
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            show_halo: "Ctrl+Alt+Space".to_string(),
            command_completion: "Ctrl+Alt+/".to_string(),
            toggle_listening: "Ctrl+Alt+L".to_string(),
            quick_capture: "Ctrl+Alt+C".to_string(),
        }
    }
}

impl Default for BehaviorSettings {
    fn default() -> Self {
        Self {
            output_mode: "replace".to_string(),
            typing_speed: 50,
            auto_dismiss_delay: 3,
            show_notifications: true,
            pii_masking: false,
            pii_keywords: vec![],
        }
    }
}

impl Default for ProvidersSettings {
    fn default() -> Self {
        Self {
            providers: vec![],
            default_provider_id: String::new(),
        }
    }
}

impl Default for GenerationSettings {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            max_tokens: 4096,
            top_p: 1.0,
            frequency_penalty: 0.0,
            presence_penalty: 0.0,
            streaming: true,
        }
    }
}

impl Default for GenerationProvidersSettings {
    fn default() -> Self {
        Self {
            providers: vec![],
            default_image_provider_id: String::new(),
            default_video_provider_id: String::new(),
            default_audio_provider_id: String::new(),
        }
    }
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_save: true,
            max_history: 100,
            embedding_model: "text-embedding-3-small".to_string(),
            similarity_threshold: 0.7,
        }
    }
}

impl Default for McpSettings {
    fn default() -> Self {
        Self { servers: vec![] }
    }
}

impl Default for PluginsSettings {
    fn default() -> Self {
        Self {
            plugins: vec![],
            auto_update: true,
        }
    }
}

impl Default for SkillsSettings {
    fn default() -> Self {
        Self { skills: vec![] }
    }
}

impl Default for FileOpsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            allowed_paths: vec![],
            denied_paths: vec![],
            max_file_size: default_max_file_size(),
            require_confirmation_for_write: default_true(),
            require_confirmation_for_delete: default_true(),
        }
    }
}

impl Default for CodeExecConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_runtime: default_runtime(),
            timeout_seconds: default_timeout(),
            sandbox_enabled: default_true(),
            allow_network: false,
            allowed_runtimes: vec![],
            working_directory: None,
            pass_env: default_pass_env(),
            blocked_commands: default_blocked_commands(),
        }
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            file_ops: FileOpsConfig::default(),
            code_exec: CodeExecConfig::default(),
            web_browsing: default_true(),
            max_iterations: default_max_iterations(),
        }
    }
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            web_search_enabled: true,
            search_engine: "duckduckgo".to_string(),
            max_results: 5,
            safe_search: true,
        }
    }
}

impl Default for PoliciesSettings {
    fn default() -> Self {
        Self {
            content_filter: true,
            filter_level: "moderate".to_string(),
            log_conversations: false,
            data_retention_days: 30,
            allow_analytics: false,
        }
    }
}

// ============================================================================
// Unified Aether Directory Structure
// All platforms use ~/.config/aether for consistency with macOS Swift version
// ============================================================================

/// Aether directory structure under ~/.config/aether
///
/// ~/.config/aether/
/// ├── config/              # Configuration files
/// │   ├── settings.json    # Main settings
/// │   └── window-state.json
/// ├── data/                # Runtime data
/// │   ├── memory/          # Memory database and embeddings
/// │   │   ├── memory.db
/// │   │   └── embeddings/
/// │   └── conversations/   # Conversation history
/// ├── attachments/         # User attachments
/// ├── skills/              # Custom skills
/// ├── mcp/                 # MCP server configs
/// ├── plugins/             # Installed plugins
/// ├── cache/               # Temporary cache
/// └── logs/                # Application logs

/// Get the base Aether directory (~/.config/aether)
pub fn get_aether_base_dir() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| AetherError::Config("Cannot find home directory".to_string()))?;

    let aether_dir = home.join(".config").join("aether");

    if !aether_dir.exists() {
        fs::create_dir_all(&aether_dir)
            .map_err(|e| AetherError::Config(format!("Cannot create aether directory: {}", e)))?;
    }

    Ok(aether_dir)
}

/// Get a subdirectory under the Aether base directory
pub fn get_aether_subdir(subdir: &str) -> Result<PathBuf> {
    let base = get_aether_base_dir()?;
    let path = base.join(subdir);

    if !path.exists() {
        fs::create_dir_all(&path)
            .map_err(|e| AetherError::Config(format!("Cannot create directory {}: {}", subdir, e)))?;
    }

    Ok(path)
}

/// Get the config directory (~/.config/aether/config)
pub fn get_config_dir() -> Result<PathBuf> {
    get_aether_subdir("config")
}

/// Get the data directory (~/.config/aether/data)
pub fn get_data_dir() -> Result<PathBuf> {
    get_aether_subdir("data")
}

/// Get the memory directory (~/.config/aether/data/memory)
pub fn get_memory_dir() -> Result<PathBuf> {
    get_aether_subdir("data/memory")
}

/// Get the attachments directory (~/.config/aether/attachments)
pub fn get_attachments_dir() -> Result<PathBuf> {
    get_aether_subdir("attachments")
}

/// Get the skills directory (~/.config/aether/skills)
pub fn get_skills_dir() -> Result<PathBuf> {
    get_aether_subdir("skills")
}

/// Get the MCP directory (~/.config/aether/mcp)
pub fn get_mcp_dir() -> Result<PathBuf> {
    get_aether_subdir("mcp")
}

/// Get the plugins directory (~/.config/aether/plugins)
pub fn get_plugins_dir() -> Result<PathBuf> {
    get_aether_subdir("plugins")
}

/// Get the cache directory (~/.config/aether/cache)
pub fn get_cache_dir() -> Result<PathBuf> {
    get_aether_subdir("cache")
}

/// Get the logs directory (~/.config/aether/logs)
pub fn get_logs_dir() -> Result<PathBuf> {
    get_aether_subdir("logs")
}

/// Get the settings file path (~/.config/aether/config/settings.json)
pub fn get_settings_path() -> Result<PathBuf> {
    let config_dir = get_config_dir()?;
    Ok(config_dir.join("settings.json"))
}

/// Load settings from disk with backward compatibility migration
pub fn load_settings() -> Result<Settings> {
    let path = get_settings_path()?;

    if !path.exists() {
        tracing::info!("Settings file not found, using defaults");
        return Ok(Settings::default());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|e| AetherError::Config(format!("Cannot read settings file: {}", e)))?;

    // First try to parse as-is (new format)
    if let Ok(settings) = serde_json::from_str::<Settings>(&contents) {
        tracing::info!("Settings loaded from {:?}", path);
        return Ok(settings);
    }

    // Try to parse with migration from old format
    tracing::info!("Attempting to migrate old settings format");
    let mut json_value: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| AetherError::Config(format!("Cannot parse settings file: {}", e)))?;

    // Migrate agent settings if in old format
    if let Some(agent) = json_value.get_mut("agent") {
        if let Some(agent_obj) = agent.as_object_mut() {
            // Check for old format markers (flat structure)
            if agent_obj.contains_key("file_operations") || agent_obj.contains_key("code_execution") {
                let migrated = migrate_agent_settings(agent_obj);
                *agent = serde_json::to_value(migrated)
                    .map_err(|e| AetherError::Config(format!("Migration error: {}", e)))?;
                tracing::info!("Agent settings migrated to new format");
            }
        }
    }

    let settings: Settings = serde_json::from_value(json_value)
        .map_err(|e| AetherError::Config(format!("Cannot parse migrated settings: {}", e)))?;

    // Save migrated settings
    if let Err(e) = save_settings(&settings) {
        tracing::warn!("Failed to save migrated settings: {}", e);
    }

    tracing::info!("Settings loaded and migrated from {:?}", path);
    Ok(settings)
}

/// Migrate old agent settings format to new nested structure
fn migrate_agent_settings(old: &serde_json::Map<String, serde_json::Value>) -> AgentSettings {
    // Extract old values with defaults
    let file_operations = old.get("file_operations")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let code_execution = old.get("code_execution")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let web_browsing = old.get("web_browsing")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let max_iterations = old.get("max_iterations")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(10);
    let require_confirmation = old.get("require_confirmation")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let sandbox_mode = old.get("sandbox_mode")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let allowed_paths: Vec<String> = old.get("allowed_paths")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let blocked_commands: Vec<String> = old.get("blocked_commands")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(default_blocked_commands);

    AgentSettings {
        file_ops: FileOpsConfig {
            enabled: file_operations,
            allowed_paths,
            denied_paths: vec![],
            max_file_size: default_max_file_size(),
            require_confirmation_for_write: require_confirmation,
            require_confirmation_for_delete: require_confirmation,
        },
        code_exec: CodeExecConfig {
            enabled: code_execution,
            default_runtime: default_runtime(),
            timeout_seconds: default_timeout(),
            sandbox_enabled: sandbox_mode,
            allow_network: false,
            allowed_runtimes: vec![],
            working_directory: None,
            pass_env: default_pass_env(),
            blocked_commands,
        },
        web_browsing,
        max_iterations,
    }
}

/// Save settings to disk
pub fn save_settings(settings: &Settings) -> Result<()> {
    let path = get_settings_path()?;

    let contents = serde_json::to_string_pretty(settings)
        .map_err(|e| AetherError::Config(format!("Cannot serialize settings: {}", e)))?;

    fs::write(&path, contents)
        .map_err(|e| AetherError::Config(format!("Cannot write settings file: {}", e)))?;

    tracing::info!("Settings saved to {:?}", path);
    Ok(())
}

/// Window state for position memory
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowState {
    pub settings: Option<WindowPosition>,
    pub conversation: Option<WindowPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Get window state file path (~/.config/aether/config/window-state.json)
pub fn get_window_state_path() -> Result<PathBuf> {
    let config_dir = get_config_dir()?;
    Ok(config_dir.join("window-state.json"))
}

/// Load window state
pub fn load_window_state() -> Result<WindowState> {
    let path = get_window_state_path()?;

    if !path.exists() {
        return Ok(WindowState::default());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|e| AetherError::Config(format!("Cannot read window state: {}", e)))?;

    let state: WindowState = serde_json::from_str(&contents)
        .map_err(|e| AetherError::Config(format!("Cannot parse window state: {}", e)))?;

    Ok(state)
}

/// Save window state
pub fn save_window_state(state: &WindowState) -> Result<()> {
    let path = get_window_state_path()?;

    let contents = serde_json::to_string_pretty(state)
        .map_err(|e| AetherError::Config(format!("Cannot serialize window state: {}", e)))?;

    fs::write(&path, contents)
        .map_err(|e| AetherError::Config(format!("Cannot write window state: {}", e)))?;

    Ok(())
}

// ============================================================================
// Path Constants for Frontend
// ============================================================================

/// Get all Aether paths for frontend use
#[derive(Debug, Clone, Serialize)]
pub struct AetherPaths {
    pub base: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub memory: PathBuf,
    pub attachments: PathBuf,
    pub skills: PathBuf,
    pub mcp: PathBuf,
    pub plugins: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
}

/// Get all Aether paths
pub fn get_aether_paths() -> Result<AetherPaths> {
    Ok(AetherPaths {
        base: get_aether_base_dir()?,
        config: get_config_dir()?,
        data: get_data_dir()?,
        memory: get_memory_dir()?,
        attachments: get_attachments_dir()?,
        skills: get_skills_dir()?,
        mcp: get_mcp_dir()?,
        plugins: get_plugins_dir()?,
        cache: get_cache_dir()?,
        logs: get_logs_dir()?,
    })
}
