/**
 * Gateway WebSocket Client
 *
 * Provides JSON-RPC 2.0 communication with the Aleph Gateway server.
 * Used for settings, memory, plugins, skills, MCP, providers, and other operations.
 */

// ============================================================================
// Types
// ============================================================================

// JSON-RPC 2.0 types (kept for reference, using Gateway message format instead)
// interface JsonRpcRequest { jsonrpc: '2.0'; id: string; method: string; params?: unknown; }
// interface JsonRpcResponse<T = unknown> { jsonrpc: '2.0'; id: string; result?: T; error?: {...}; }

/** Gateway message types */
interface GatewayMessage {
  type: 'req' | 'res' | 'event' | 'stream';
  id?: string;
  method?: string;
  params?: unknown;
  ok?: boolean;
  payload?: unknown;
  error?: {
    code: number;
    message: string;
  };
}

/** Connection state */
type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error';

/** Event handler type */
type EventHandler = (event: GatewayMessage) => void;

// ============================================================================
// Gateway RPC Result Types
// ============================================================================

export interface GWLogLevelResult {
  level: string;
}

export interface GWLogDirectoryResult {
  directory: string;
}

export interface GWCommandInfo {
  name: string;
  description: string;
  category: string | null;
}

export interface GWMemorySearchItem {
  id: string;
  content: string;
  similarity: number;
  metadata: unknown | null;
}

export interface GWMemoryStatsResult {
  count: number;
  size_bytes: number;
}

export interface GWPluginInfo {
  id: string;
  name: string;
  version: string;
  enabled: boolean;
  description: string | null;
}

export interface GWSkillInfo {
  id: string;
  name: string;
  description: string | null;
  source: string | null;
}

export interface GWMcpServerInfo {
  name: string;
  enabled: boolean;
  url: string | null;
  transport: string | null;
}

export interface GWMcpServerConfig {
  name: string;
  enabled: boolean;
  transport: string;
  url?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
}

export interface GWProviderInfo {
  name: string;
  enabled: boolean;
  model: string;
  provider_type: string | null;
  is_default: boolean;
}

export interface GWProviderConfig {
  enabled: boolean;
  model: string;
  api_key?: string;
  base_url?: string;
}

export interface GWProviderTestResult {
  success: boolean;
  error: string | null;
  latency_ms: number | null;
}

export interface GWGenerationProviderInfo {
  name: string;
  enabled: boolean;
  provider_type: string;
  model: string | null;
}

export interface GWBehaviorConfig {
  auto_apply: boolean;
  confirm_before_apply: boolean;
  max_context_tokens: number | null;
}

export interface GWSearchConfig {
  enabled: boolean;
  provider: string | null;
  api_key?: string;
}

export interface GWSearchConfigView {
  enabled: boolean;
  provider: string | null;
}

export interface GWPoliciesConfig {
  allow_web_browsing: boolean;
  allow_file_access: boolean;
  allow_code_execution: boolean;
}

export interface GWShortcutsConfig {
  trigger_hotkey: string | null;
  vision_hotkey: string | null;
}

export interface GWTriggersConfig {
  double_tap_enabled: boolean;
  double_tap_interval_ms: number | null;
}

export interface GWCodeExecConfig {
  enabled: boolean;
  sandbox: boolean;
  timeout_ms: number | null;
}

export interface GWFileOpsConfig {
  enabled: boolean;
  allowed_paths: string[];
  denied_paths: string[];
}

// ============================================================================
// Gateway Client Class
// ============================================================================

class GatewayClient {
  private ws: WebSocket | null = null;
  private url: string;
  private state: ConnectionState = 'disconnected';
  private pendingRequests: Map<string, {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
    timeout: ReturnType<typeof setTimeout>;
  }> = new Map();
  private eventHandlers: Map<string, Set<EventHandler>> = new Map();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectDelay = 1000;

  constructor(url = 'ws://127.0.0.1:18789') {
    this.url = url;
  }

  // Connection Management

  get isConnected(): boolean {
    return this.state === 'connected' && this.ws?.readyState === WebSocket.OPEN;
  }

  get connectionState(): ConnectionState {
    return this.state;
  }

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      if (this.state === 'connected') {
        resolve();
        return;
      }

      if (this.state === 'connecting') {
        // Wait for existing connection attempt
        const checkInterval = setInterval(() => {
          if (this.state === 'connected') {
            clearInterval(checkInterval);
            resolve();
          } else if (this.state === 'error' || this.state === 'disconnected') {
            clearInterval(checkInterval);
            reject(new Error('Connection failed'));
          }
        }, 100);
        return;
      }

      this.state = 'connecting';

      try {
        this.ws = new WebSocket(this.url);

        this.ws.onopen = () => {
          console.log('[Gateway] Connected to', this.url);
          this.state = 'connected';
          this.reconnectAttempts = 0;
          this.sendConnect().then(resolve).catch(reject);
        };

        this.ws.onclose = () => {
          console.log('[Gateway] Disconnected');
          this.state = 'disconnected';
          this.handleDisconnect();
        };

        this.ws.onerror = (error) => {
          console.error('[Gateway] WebSocket error:', error);
          this.state = 'error';
          reject(new Error('WebSocket connection error'));
        };

        this.ws.onmessage = (event) => {
          this.handleMessage(event.data);
        };
      } catch (error) {
        this.state = 'error';
        reject(error);
      }
    });
  }

  disconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }

    this.state = 'disconnected';

    // Reject all pending requests
    for (const [_id, pending] of this.pendingRequests) {
      clearTimeout(pending.timeout);
      pending.reject(new Error('Connection closed'));
    }
    this.pendingRequests.clear();
  }

  private async sendConnect(): Promise<void> {
    const connectRequest: GatewayMessage = {
      type: 'req',
      id: this.generateId(),
      method: 'connect',
      params: {
        minProtocol: 1,
        maxProtocol: 1,
        client: {
          id: 'tauri',
          version: '0.1.0',
          platform: 'tauri',
        },
        role: 'operator',
        scopes: ['operator.read', 'operator.write'],
      },
    };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error('Connect timeout'));
      }, 5000);

      const originalHandler = this.ws!.onmessage;
      this.ws!.onmessage = (event) => {
        clearTimeout(timeout);
        this.ws!.onmessage = originalHandler;

        try {
          const msg = JSON.parse(event.data) as GatewayMessage;
          if (msg.type === 'res' && msg.ok) {
            console.log('[Gateway] Handshake complete');
            resolve();
          } else {
            reject(new Error(msg.error?.message || 'Handshake failed'));
          }
        } catch (e) {
          reject(e);
        }
      };

      this.ws!.send(JSON.stringify(connectRequest));
    });
  }

  private handleDisconnect(): void {
    // Reject all pending requests
    for (const [_id, pending] of this.pendingRequests) {
      clearTimeout(pending.timeout);
      pending.reject(new Error('Connection lost'));
    }
    this.pendingRequests.clear();

    // Schedule reconnect
    if (this.reconnectAttempts < this.maxReconnectAttempts) {
      const delay = this.reconnectDelay * Math.pow(2, this.reconnectAttempts);
      console.log(`[Gateway] Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts + 1})`);

      this.reconnectTimer = setTimeout(() => {
        this.reconnectAttempts++;
        this.connect().catch(() => {
          // Will trigger another reconnect attempt
        });
      }, delay);
    }
  }

  private handleMessage(data: string): void {
    try {
      const msg = JSON.parse(data) as GatewayMessage;

      if (msg.type === 'res' && msg.id) {
        // Response to a request
        const pending = this.pendingRequests.get(msg.id);
        if (pending) {
          clearTimeout(pending.timeout);
          this.pendingRequests.delete(msg.id);

          if (msg.ok) {
            pending.resolve(msg.payload);
          } else {
            pending.reject(new Error(msg.error?.message || 'Request failed'));
          }
        }
      } else if (msg.type === 'event') {
        // Server-pushed event
        this.dispatchEvent(msg);
      } else if (msg.type === 'stream') {
        // Streaming data
        this.dispatchEvent(msg);
      }
    } catch (e) {
      console.error('[Gateway] Failed to parse message:', e);
    }
  }

  private dispatchEvent(event: GatewayMessage): void {
    const topic = (event as { topic?: string }).topic || '*';
    const handlers = this.eventHandlers.get(topic) || new Set();
    const wildcardHandlers = this.eventHandlers.get('*') || new Set();

    for (const handler of [...handlers, ...wildcardHandlers]) {
      try {
        handler(event);
      } catch (e) {
        console.error('[Gateway] Event handler error:', e);
      }
    }
  }

  // RPC Methods

  async call<T>(method: string, params?: unknown): Promise<T> {
    if (!this.isConnected) {
      throw new Error('Not connected to Gateway');
    }

    const id = this.generateId();
    const request: GatewayMessage = {
      type: 'req',
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pendingRequests.delete(id);
        reject(new Error(`Request timeout: ${method}`));
      }, 30000);

      this.pendingRequests.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
        timeout,
      });

      this.ws!.send(JSON.stringify(request));
    });
  }

  // Event Subscription

  subscribe(topic: string, handler: EventHandler): () => void {
    if (!this.eventHandlers.has(topic)) {
      this.eventHandlers.set(topic, new Set());
    }
    this.eventHandlers.get(topic)!.add(handler);

    // Return unsubscribe function
    return () => {
      const handlers = this.eventHandlers.get(topic);
      if (handlers) {
        handlers.delete(handler);
        if (handlers.size === 0) {
          this.eventHandlers.delete(topic);
        }
      }
    };
  }

  // Helper

  private generateId(): string {
    return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
  }

  // ============================================================================
  // RPC Convenience Methods
  // ============================================================================

  // Logs

  async logsGetLevel(): Promise<string> {
    const result = await this.call<GWLogLevelResult>('logs.getLevel');
    return result.level;
  }

  async logsSetLevel(level: string): Promise<void> {
    await this.call('logs.setLevel', { level });
  }

  async logsGetDirectory(): Promise<string> {
    const result = await this.call<GWLogDirectoryResult>('logs.getDirectory');
    return result.directory;
  }

  // Commands

  async commandsList(): Promise<GWCommandInfo[]> {
    const result = await this.call<{ commands: GWCommandInfo[] }>('commands.list');
    return result.commands;
  }

  // Memory

  async memorySearch(query: string, limit?: number): Promise<GWMemorySearchItem[]> {
    const result = await this.call<{ results: GWMemorySearchItem[] }>('memory.search', { query, limit });
    return result.results;
  }

  async memoryDelete(id: string): Promise<void> {
    await this.call('memory.delete', { id });
  }

  async memoryClear(): Promise<void> {
    await this.call('memory.clear');
  }

  async memoryClearFacts(): Promise<number> {
    const result = await this.call<{ deleted: number }>('memory.clearFacts');
    return result.deleted;
  }

  async memoryStats(): Promise<GWMemoryStatsResult> {
    return await this.call<GWMemoryStatsResult>('memory.stats');
  }

  async memoryCompress(): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('memory.compress');
    return result.ok;
  }

  async memoryAppList(): Promise<string[]> {
    const result = await this.call<{ apps: string[] }>('memory.appList');
    return result.apps;
  }

  // Plugins

  async pluginsList(): Promise<GWPluginInfo[]> {
    const result = await this.call<{ plugins: GWPluginInfo[] }>('plugins.list');
    return result.plugins;
  }

  async pluginsInstall(url: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('plugins.install', { url });
    return result.ok;
  }

  async pluginsInstallFromZip(data: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('plugins.installFromZip', { data });
    return result.ok;
  }

  async pluginsUninstall(id: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('plugins.uninstall', { id });
    return result.ok;
  }

  async pluginsEnable(id: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('plugins.enable', { id });
    return result.ok;
  }

  async pluginsDisable(id: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('plugins.disable', { id });
    return result.ok;
  }

  // Skills

  async skillsList(): Promise<GWSkillInfo[]> {
    const result = await this.call<{ skills: GWSkillInfo[] }>('skills.list');
    return result.skills;
  }

  async skillsInstall(url: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('skills.install', { url });
    return result.ok;
  }

  async skillsInstallFromZip(data: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('skills.installFromZip', { data });
    return result.ok;
  }

  async skillsDelete(id: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('skills.delete', { id });
    return result.ok;
  }

  // MCP

  async mcpListServers(): Promise<GWMcpServerInfo[]> {
    const result = await this.call<{ servers: GWMcpServerInfo[] }>('mcp.listServers');
    return result.servers;
  }

  async mcpGetServer(name: string): Promise<GWMcpServerInfo> {
    const result = await this.call<{ server: GWMcpServerInfo }>('mcp.getServer', { name });
    return result.server;
  }

  async mcpAddServer(config: GWMcpServerConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('mcp.addServer', { config });
    return result.ok;
  }

  async mcpRemoveServer(name: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('mcp.removeServer', { name });
    return result.ok;
  }

  async mcpEnableServer(name: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('mcp.enableServer', { name });
    return result.ok;
  }

  async mcpDisableServer(name: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('mcp.disableServer', { name });
    return result.ok;
  }

  // Providers

  async providersList(): Promise<GWProviderInfo[]> {
    const result = await this.call<{ providers: GWProviderInfo[] }>('providers.list');
    return result.providers;
  }

  async providersGet(name: string): Promise<GWProviderInfo> {
    const result = await this.call<{ provider: GWProviderInfo }>('providers.get', { name });
    return result.provider;
  }

  async providersUpdate(name: string, config: GWProviderConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('providers.update', { name, config });
    return result.ok;
  }

  async providersDelete(name: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('providers.delete', { name });
    return result.ok;
  }

  async providersTest(config: GWProviderConfig): Promise<GWProviderTestResult> {
    return await this.call<GWProviderTestResult>('providers.test', { config });
  }

  async providersSetDefault(name: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('providers.setDefault', { name });
    return result.ok;
  }

  // Generation Providers

  async generationListProviders(): Promise<GWGenerationProviderInfo[]> {
    const result = await this.call<{ providers: GWGenerationProviderInfo[] }>('generation.listProviders');
    return result.providers;
  }

  // Config: Behavior

  async configBehaviorGet(): Promise<GWBehaviorConfig> {
    const result = await this.call<{ behavior: GWBehaviorConfig }>('config.behavior.get');
    return result.behavior;
  }

  async configBehaviorUpdate(config: GWBehaviorConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('config.behavior.update', config);
    return result.ok;
  }

  // Config: Search

  async configSearchGet(): Promise<GWSearchConfigView> {
    const result = await this.call<{ search: GWSearchConfigView }>('config.search.get');
    return result.search;
  }

  async configSearchUpdate(config: GWSearchConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('config.search.update', config);
    return result.ok;
  }

  // Config: Policies

  async configPoliciesGet(): Promise<GWPoliciesConfig> {
    const result = await this.call<{ policies: GWPoliciesConfig }>('config.policies.get');
    return result.policies;
  }

  async configPoliciesUpdate(config: GWPoliciesConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('config.policies.update', config);
    return result.ok;
  }

  // Config: Shortcuts

  async configShortcutsGet(): Promise<GWShortcutsConfig> {
    const result = await this.call<{ shortcuts: GWShortcutsConfig }>('config.shortcuts.get');
    return result.shortcuts;
  }

  async configShortcutsUpdate(config: GWShortcutsConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('config.shortcuts.update', config);
    return result.ok;
  }

  // Config: Security

  async configSecurityGetCodeExec(): Promise<GWCodeExecConfig> {
    const result = await this.call<{ codeExec: GWCodeExecConfig }>('config.security.getCodeExec');
    return result.codeExec;
  }

  async configSecurityUpdateCodeExec(config: GWCodeExecConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('config.security.updateCodeExec', config);
    return result.ok;
  }

  async configSecurityGetFileOps(): Promise<GWFileOpsConfig> {
    const result = await this.call<{ fileOps: GWFileOpsConfig }>('config.security.getFileOps');
    return result.fileOps;
  }

  async configSecurityUpdateFileOps(config: GWFileOpsConfig): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('config.security.updateFileOps', config);
    return result.ok;
  }

  // Agent Extensions

  async agentConfirmPlan(planId: string, confirmed: boolean): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('agent.confirmPlan', {
      plan_id: planId,
      confirmed,
    });
    return result.ok;
  }

  async agentRespondToInput(requestId: string, response: string): Promise<boolean> {
    const result = await this.call<{ ok: boolean }>('agent.respondToInput', {
      request_id: requestId,
      response,
    });
    return result.ok;
  }

  async agentGenerateTitle(userInput: string, aiResponse: string): Promise<string> {
    const result = await this.call<{ title: string }>('agent.generateTitle', {
      user_input: userInput,
      ai_response: aiResponse,
    });
    return result.title;
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

export const gateway = new GatewayClient();

// Auto-connect on import (optional - can be disabled)
// gateway.connect().catch(console.error);
