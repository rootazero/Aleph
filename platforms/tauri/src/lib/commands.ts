import { invoke } from '@tauri-apps/api/core';

// Types
export interface Position {
  x: number;
  y: number;
}

export interface AppVersion {
  version: string;
  build: string;
}

export interface GeneralSettings {
  sound_enabled: boolean;
  launch_at_login: boolean;
  language: string;
}

export interface ShortcutSettings {
  show_halo: string;
  command_completion: string;
  toggle_listening: string;
  quick_capture: string;
}

export interface BehaviorSettings {
  output_mode: 'replace' | 'append' | 'clipboard' | 'typewriter' | 'instant';
  typing_speed: number; // 50-400 chars/sec
  auto_dismiss_delay: number; // seconds
  show_notifications: boolean;
  pii_masking: boolean;
  pii_keywords: string[];
  // PII scrubbing options (macOS-aligned)
  pii_scrub_email?: boolean;
  pii_scrub_phone?: boolean;
  pii_scrub_ssn?: boolean;
  pii_scrub_credit_card?: boolean;
}

export interface ProviderConfig {
  id: string;
  name: string;
  type: 'openai' | 'anthropic' | 'gemini' | 'ollama' | 'custom';
  api_key?: string;
  base_url?: string;
  model?: string;
  enabled: boolean;
  is_default: boolean;
}

export interface ProvidersSettings {
  providers: ProviderConfig[];
  default_provider_id: string;
}

export interface GenerationSettings {
  temperature: number;
  max_tokens: number;
  top_p: number;
  frequency_penalty: number;
  presence_penalty: number;
  streaming: boolean;
}

export interface GenerationProviderConfig {
  id: string;
  name: string;
  type: string;
  category: 'image' | 'video' | 'audio';
  api_key?: string;
  base_url?: string;
  model?: string;
  enabled: boolean;
  is_default: boolean;
}

export interface GenerationProvidersSettings {
  providers: GenerationProviderConfig[];
  default_image_provider_id: string;
  default_video_provider_id: string;
  default_audio_provider_id: string;
}

export interface MemorySettings {
  enabled: boolean;
  auto_save: boolean;
  max_history: number;
  embedding_model: string;
  similarity_threshold: number;
}

export interface McpServer {
  id: string;
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
}

export interface McpSettings {
  servers: McpServer[];
}

export interface Plugin {
  id: string;
  name: string;
  version: string;
  description: string;
  source: 'git' | 'zip' | 'local';
  source_url?: string;
  enabled: boolean;
  config?: Record<string, unknown>;
}

export interface PluginsSettings {
  plugins: Plugin[];
  auto_update: boolean;
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  trigger_keywords: string[];
}

export interface SkillsSettings {
  skills: Skill[];
}

export interface FileOpsConfig {
  enabled: boolean;
  allowed_paths: string[];
  denied_paths: string[];
  max_file_size: number;
  require_confirmation_for_write: boolean;
  require_confirmation_for_delete: boolean;
}

export interface CodeExecConfig {
  enabled: boolean;
  default_runtime: 'shell' | 'python' | 'node';
  timeout_seconds: number;
  sandbox_enabled: boolean;
  allow_network: boolean;
  allowed_runtimes: string[];
  working_directory: string | null;
  pass_env: string[];
  blocked_commands: string[];
}

export interface AgentSettings {
  file_ops: FileOpsConfig;
  code_exec: CodeExecConfig;
  web_browsing: boolean;
  max_iterations: number;
}

export interface SearchSettings {
  web_search_enabled: boolean;
  search_engine: 'google' | 'bing' | 'duckduckgo';
  max_results: number;
  safe_search: boolean;
}

export interface PoliciesSettings {
  content_filter: boolean;
  filter_level: 'strict' | 'moderate' | 'off';
  log_conversations: boolean;
  data_retention_days: number;
  allow_analytics: boolean;
}

export interface Settings {
  general: GeneralSettings;
  shortcuts: ShortcutSettings;
  behavior: BehaviorSettings;
  providers: ProvidersSettings;
  generation: GenerationSettings;
  generationProviders: GenerationProvidersSettings;
  memory: MemorySettings;
  mcp: McpSettings;
  plugins: PluginsSettings;
  skills: SkillsSettings;
  agent: AgentSettings;
  search: SearchSettings;
  policies: PoliciesSettings;
}

export interface WindowPosition {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface AetherPaths {
  base: string;
  config: string;
  data: string;
  memory: string;
  attachments: string;
  skills: string;
  mcp: string;
  plugins: string;
  cache: string;
  logs: string;
}

// ============================================================================
// AI Core Types
// ============================================================================

/** Generation provider information */
export interface GenerationProviderInfo {
  name: string;
  color: string;
  supported_types: string[];
  default_model: string | null;
}

/** Memory item from search */
export interface MemoryItem {
  id: string;
  user_input: string;
  assistant_response: string;
  timestamp: number;
  app_context: string | null;
}

/** Memory statistics */
export interface MemoryStats {
  total_memories: number;
  total_apps: number;
  database_size_mb: number;
  oldest_memory_timestamp: number;
  newest_memory_timestamp: number;
}

/** Tool information */
export interface ToolInfo {
  name: string;
  description: string;
  source: string;
}

/** MCP server information */
export interface McpServerInfo {
  id: string;
  name: string;
  server_type: string;
  enabled: boolean;
  command: string | null;
  trigger_command: string | null;
}

/** MCP configuration */
export interface McpConfig {
  enabled: boolean;
  fs_enabled: boolean;
  git_enabled: boolean;
  shell_enabled: boolean;
  system_info_enabled: boolean;
}

/** Skill information */
export interface SkillInfo {
  id: string;
  name: string;
  description: string;
  allowed_tools: string[];
}

/**
 * Tauri command wrappers
 */
export const commands = {
  // App
  getAppVersion: () => invoke<AppVersion>('get_app_version'),

  // Cursor
  getCursorPosition: () => invoke<Position>('get_cursor_position'),

  // Windows
  showHaloWindow: () => invoke('show_halo_window'),
  hideHaloWindow: () => invoke('hide_halo_window'),
  openSettingsWindow: () => invoke('open_settings_window'),

  // Settings
  getSettings: () => invoke<Settings>('get_settings'),
  saveSettings: (settings: Settings) => invoke('save_settings', { newSettings: settings }),

  // Window position
  saveWindowPosition: (windowName: string) => invoke('save_window_position', { windowName }),
  getWindowPosition: (windowName: string) => invoke<WindowPosition | null>('get_window_position', { windowName }),

  // Notifications
  sendNotification: (title: string, body: string) => invoke('send_notification', { title, body }),

  // Autostart
  getAutostartEnabled: () => invoke<boolean>('get_autostart_enabled'),
  setAutostartEnabled: (enabled: boolean) => invoke('set_autostart_enabled', { enabled }),

  // Paths (~/.config/aleph/*)
  getAetherPaths: () => invoke<AetherPaths>('get_aether_paths'),

  // ============================================================================
  // AI Processing
  // ============================================================================

  /** Process user input through the AI */
  processInput: (input: string, topicId?: string, stream?: boolean) =>
    invoke('process_input', { input, topicId, stream }),

  /** Cancel the current AI processing operation */
  cancelProcessing: () => invoke('cancel_processing'),

  /** Check if processing is cancelled */
  isProcessingCancelled: () => invoke<boolean>('is_processing_cancelled'),

  /** Generate a topic title from conversation */
  generateTopicTitle: (userInput: string, aiResponse: string) =>
    invoke<string>('generate_topic_title', { userInput, aiResponse }),

  /** Extract text from an image using OCR */
  extractTextFromImage: (imageData: number[]) =>
    invoke<string>('extract_text_from_image', { imageData }),

  // ============================================================================
  // Provider Management
  // ============================================================================

  /** List all configured generation providers */
  listGenerationProviders: () => invoke<GenerationProviderInfo[]>('list_generation_providers'),

  /** Set the default provider */
  setDefaultProvider: (providerName: string) =>
    invoke('set_default_provider', { providerName }),

  /** Reload configuration from disk */
  reloadConfig: () => invoke('reload_config'),

  // ============================================================================
  // Memory Management
  // ============================================================================

  /** Search memory with a query */
  searchMemory: (query: string, limit?: number) =>
    invoke<MemoryItem[]>('search_memory', { query, limit }),

  /** Get memory statistics */
  getMemoryStats: () => invoke<MemoryStats>('get_memory_stats'),

  /** Clear all memory entries */
  clearMemory: () => invoke('clear_memory'),

  // ============================================================================
  // Tool Management
  // ============================================================================

  /** List all available tools */
  listTools: () => invoke<ToolInfo[]>('list_tools'),

  /** Get tool count */
  getToolCount: () => invoke<number>('get_tool_count'),

  // ============================================================================
  // MCP Server Management
  // ============================================================================

  /** List MCP servers */
  listMcpServers: () => invoke<McpServerInfo[]>('list_mcp_servers'),

  /** Get MCP configuration */
  getMcpConfig: () => invoke<McpConfig>('get_mcp_config'),

  // ============================================================================
  // Skills Management
  // ============================================================================

  /** List installed skills */
  listSkills: () => invoke<SkillInfo[]>('list_skills'),

  // ============================================================================
  // Logs
  // ============================================================================

  /** Get application logs */
  getLogs: () => invoke<string>('get_logs'),

  // ============================================================================
  // Confirmations & Responses
  // ============================================================================

  /** Respond to tool confirmation request */
  respondToolConfirmation: (confirmationId: string, approved: boolean) =>
    invoke('respond_tool_confirmation', { confirmationId, approved }),

  /** Respond to plan confirmation request */
  respondPlanConfirmation: (planId: string, approved: boolean) =>
    invoke('respond_plan_confirmation', { planId, approved }),

  /** Respond to clarification request */
  respondClarification: (clarificationId: string, response: string) =>
    invoke('respond_clarification', { clarificationId, response }),
};
