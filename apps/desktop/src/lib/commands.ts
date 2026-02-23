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

export interface WindowPosition {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface AlephPaths {
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

export interface AlephPaths {
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

  // Window position
  saveWindowPosition: (windowName: string) => invoke('save_window_position', { windowName }),
  getWindowPosition: (windowName: string) => invoke<WindowPosition | null>('get_window_position', { windowName }),

  // Notifications
  sendNotification: (title: string, body: string) => invoke('send_notification', { title, body }),

  // Autostart
  getAutostartEnabled: () => invoke<boolean>('get_autostart_enabled'),
  setAutostartEnabled: (enabled: boolean) => invoke('set_autostart_enabled', { enabled }),

  // Paths (~/.config/aleph/*)
  getAlephPaths: () => invoke<AlephPaths>('get_aleph_paths'),

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
