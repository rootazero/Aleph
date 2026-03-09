// =============================================================================
// Plugin Manifest (mirrors core/src/extension/manifest/types.rs)
// =============================================================================

/** Plugin runtime kind */
export type PluginKind = "wasm" | "nodejs" | "static";

/** Plugin permission types (mirrors PluginPermission enum) */
export type PluginPermission =
  | "network"
  | "filesystem"
  | "filesystem:read"
  | "filesystem:write"
  | "env"
  | (string & {}); // Custom permissions

/** Author information for plugin manifest */
export interface AuthorInfo {
  name?: string;
  email?: string;
  url?: string;
}

/** UI hints for configuration fields */
export interface ConfigUiHint {
  label?: string;
  help?: string;
  advanced?: boolean;
  sensitive?: boolean;
  placeholder?: string;
}

/** JSON Schema type (any valid JSON Schema object) */
export type JsonSchema = Record<string, unknown>;

/**
 * Plugin manifest — mirrors the `aleph.plugin.toml` structure.
 *
 * Matches `PluginManifest` in `core/src/extension/manifest/types.rs`.
 */
export interface PluginManifest {
  /** Unique plugin identifier (lowercase, alphanumeric with hyphens) */
  id: string;
  /** Human-readable plugin name */
  name: string;
  /** Plugin version (semver format) */
  version?: string;
  /** Plugin description */
  description?: string;
  /** Plugin runtime kind */
  kind: PluginKind;
  /** Entry point relative to plugin root */
  entry: string;
  /** JSON Schema for plugin configuration */
  configSchema?: JsonSchema;
  /** UI hints for configuration fields, keyed by field name */
  configUiHints?: Record<string, ConfigUiHint>;
  /** Required permissions */
  permissions?: PluginPermission[];
  /** Author information */
  author?: AuthorInfo;
  /** Plugin homepage URL */
  homepage?: string;
  /** Repository URL */
  repository?: string;
  /** License identifier (SPDX) */
  license?: string;
  /** Search keywords */
  keywords?: string[];
  /** Tool declarations */
  tools?: ManifestToolSection[];
  /** Hook declarations */
  hooks?: ManifestHookSection[];
  /** Command declarations */
  commands?: ManifestCommandSection[];
  /** Service declarations */
  services?: ManifestServiceSection[];
  /** Channel declarations */
  channels?: ManifestChannelSection[];
  /** Provider declarations */
  providers?: ManifestProviderSection[];
  /** HTTP route declarations */
  httpRoutes?: ManifestHttpRouteSection[];
  /** Prompt configuration */
  prompt?: ManifestPromptSection;
}

// =============================================================================
// Manifest Section Types (from aleph_plugin_toml.rs)
// =============================================================================

/** Tool declaration in aleph.plugin.toml [[tools]] */
export interface ManifestToolSection {
  name: string;
  description?: string;
  handler?: string;
  parameters?: JsonSchema;
}

/** Hook declaration in aleph.plugin.toml [[hooks]] */
export interface ManifestHookSection {
  event: string;
  kind?: "observer" | "interceptor";
  handler?: string;
  priority?: number;
}

/** Command declaration in aleph.plugin.toml [[commands]] */
export interface ManifestCommandSection {
  name: string;
  description?: string;
  handler?: string;
}

/** Service declaration in aleph.plugin.toml [[services]] */
export interface ManifestServiceSection {
  name: string;
  description?: string;
  startHandler?: string;
  stopHandler?: string;
}

/** Channel declaration in aleph.plugin.toml [[channels]] */
export interface ManifestChannelSection {
  id: string;
  label: string;
  handler?: string;
}

/** Provider declaration in aleph.plugin.toml [[providers]] */
export interface ManifestProviderSection {
  id: string;
  name: string;
  models?: string[];
  handler?: string;
}

/** HTTP route declaration in aleph.plugin.toml [[http_routes]] */
export interface ManifestHttpRouteSection {
  path: string;
  methods?: string[];
  handler: string;
}

/** Prompt configuration in aleph.plugin.toml [prompt] */
export interface ManifestPromptSection {
  file: string;
  scope?: "system" | "user";
}

// =============================================================================
// Tool Result (returned from tool execution)
// =============================================================================

/** A single content block in a tool result */
export interface ToolResultContent {
  type: "text";
  text: string;
}

/** Result returned from a tool execution */
export interface ToolResult {
  content: ToolResultContent[];
  isError?: boolean;
}

// =============================================================================
// Registration Types (mirrors core/src/extension/registry/types.rs)
// =============================================================================

/**
 * Tool definition sent during plugin registration.
 * Mirrors `ToolDefinition` in `core/src/extension/runtime/nodejs/ipc.rs`.
 */
export interface ToolDefinition {
  /** Unique tool name */
  name: string;
  /** Human-readable description */
  description: string;
  /** JSON Schema defining input parameters */
  parameters: JsonSchema;
  /** Handler function name within the plugin */
  handler: string;
}

/**
 * Hook definition sent during plugin registration.
 * Mirrors `HookDefinition` in `core/src/extension/runtime/nodejs/ipc.rs`.
 */
export interface HookDefinition {
  /** Event name (snake_case) */
  event: PluginHookEvent;
  /** Execution priority (lower = earlier, default 0) */
  priority?: number;
  /** Handler function name within the plugin */
  handler: string;
}

/**
 * Events that can trigger plugin hooks.
 * Mirrors `PluginHookEvent` in `core/src/extension/registry/types.rs`.
 * Uses snake_case for JSON-RPC IPC serialization.
 */
export type PluginHookEvent =
  | "before_agent_start"
  | "agent_end"
  | "before_tool_call"
  | "after_tool_call"
  | "tool_result_persist"
  | "message_received"
  | "message_sending"
  | "message_sent"
  | "session_start"
  | "session_end"
  | "before_compaction"
  | "after_compaction"
  | "gateway_start"
  | "gateway_stop";

/** Channel definition for plugin registration */
export interface ChannelDefinition {
  id: string;
  label: string;
}

/** Provider definition for plugin registration */
export interface ProviderDefinition {
  id: string;
  name: string;
  models: string[];
}

/** Gateway method definition for plugin registration */
export interface GatewayMethodDefinition {
  method: string;
  handler: string;
}

/** Service registration */
export interface ServiceRegistration {
  id: string;
  name: string;
  startHandler: string;
  stopHandler: string;
}

/** Command registration */
export interface CommandRegistration {
  name: string;
  description: string;
  handler: string;
}

/** HTTP route registration */
export interface HttpRouteRegistration {
  path: string;
  methods: string[];
  handler: string;
}

/**
 * Plugin registration params sent from Node.js plugin to host.
 * Mirrors `PluginRegistrationParams` in `core/src/extension/runtime/nodejs/ipc.rs`.
 */
export interface PluginRegistrationParams {
  pluginId: string;
  tools?: ToolDefinition[];
  hooks?: HookDefinition[];
  channels?: ChannelDefinition[];
  providers?: ProviderDefinition[];
  gatewayMethods?: GatewayMethodDefinition[];
}

// =============================================================================
// Hook Request / Response (what hooks receive and return)
// =============================================================================

/** Request payload sent to a hook handler */
export interface HookRequest {
  /** The event that triggered this hook */
  event: PluginHookEvent;
  /** Event-specific data */
  data: Record<string, unknown>;
  /** Metadata about the hook invocation */
  meta?: {
    /** ID of the session, if applicable */
    sessionId?: string;
    /** Timestamp of the event */
    timestamp?: string;
  };
}

/** Response returned from a hook handler */
export interface HookResponse {
  /** Whether to allow the operation to proceed (for interceptor hooks) */
  allow?: boolean;
  /** Modified data to pass downstream (for interceptor hooks) */
  data?: Record<string, unknown>;
  /** Optional message explaining the hook's decision */
  message?: string;
}

// =============================================================================
// JSON-RPC 2.0 IPC Protocol (mirrors core/src/extension/runtime/nodejs/ipc.rs)
// =============================================================================

/** JSON-RPC 2.0 request */
export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: string;
  method: string;
  params?: unknown;
}

/** JSON-RPC 2.0 error object */
export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

/** JSON-RPC 2.0 response */
export interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: string;
  result?: unknown;
  error?: JsonRpcError;
}

/** JSON-RPC 2.0 notification (no id, no response expected) */
export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

// =============================================================================
// Plugin API Interface (what Node.js plugins receive)
// =============================================================================

/** Handler for tool execution */
export type ToolHandler = (
  toolCallId: string,
  params: Record<string, unknown>,
) => Promise<ToolResult>;

/** Handler for hook events */
export type HookHandler = (request: HookRequest) => Promise<HookResponse>;

/** Tool registration with inline execute handler */
export interface ToolRegistrationWithHandler {
  name: string;
  description: string;
  parameters: JsonSchema;
  handler?: string;
  execute?: ToolHandler;
}

/** Hook registration with inline handler */
export interface HookRegistrationWithHandler {
  event: PluginHookEvent;
  handler?: string;
  priority?: number;
  execute?: HookHandler;
}

/**
 * Plugin API provided to Node.js plugins during initialization.
 *
 * This is the main interface plugins interact with. The host provides
 * an implementation of this interface when loading the plugin.
 */
export interface AlephPluginApi {
  /** Plugin's unique identifier */
  id: string;
  /** Plugin's display name */
  name: string;
  /** Global Aleph configuration */
  config: Record<string, unknown>;
  /** Plugin-specific configuration */
  pluginConfig?: Record<string, unknown>;

  /** Register a callable tool */
  registerTool(tool: ToolRegistrationWithHandler): void;
  /** Register a hook for system events */
  registerHook(hook: HookRegistrationWithHandler): void;
  /** Register a background service */
  registerService(service: ServiceRegistration): void;
  /** Register a messaging channel */
  registerChannel(channel: ChannelDefinition): void;
  /** Register an in-chat command */
  registerCommand(command: CommandRegistration): void;
  /** Register an HTTP route */
  registerHttpRoute(route: HttpRouteRegistration): void;
  /** Register an AI model provider */
  registerProvider(provider: ProviderDefinition): void;
  /** Register a gateway RPC method */
  registerGatewayMethod(method: GatewayMethodDefinition): void;

  /** Resolve a path relative to the plugin root */
  resolvePath(input: string): string;
  /** Listen for a plugin lifecycle event */
  on(event: PluginHookEvent, handler: HookHandler, options?: { priority?: number }): void;
}

/**
 * Plugin entry point function signature.
 *
 * Your plugin's default export should match this type:
 * ```ts
 * const plugin: AlephPluginEntry = async (api) => {
 *   api.registerTool({ ... });
 * };
 * export default plugin;
 * ```
 */
export type AlephPluginEntry = (api: AlephPluginApi) => Promise<void>;
