/**
 * Gateway Store
 *
 * Manages Gateway WebSocket connection state and provides reactive access
 * to the Gateway client throughout the application.
 */

import { create } from 'zustand';
import { gateway, type GWPluginInfo, type GWSkillInfo, type GWMcpServerInfo, type GWProviderInfo } from '@/lib/gateway';

type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error';

interface GatewayStore {
  // Connection state
  connectionState: ConnectionState;
  error: string | null;

  // Actions
  connect: () => Promise<void>;
  disconnect: () => void;

  // Status
  isConnected: () => boolean;

  // Gateway operations with fallback support
  // These will try Gateway first, then fall back to Tauri commands

  // Plugins
  getPlugins: () => Promise<GWPluginInfo[]>;
  enablePlugin: (id: string) => Promise<boolean>;
  disablePlugin: (id: string) => Promise<boolean>;
  uninstallPlugin: (id: string) => Promise<boolean>;
  installPlugin: (url: string) => Promise<boolean>;

  // Skills
  getSkills: () => Promise<GWSkillInfo[]>;
  deleteSkill: (id: string) => Promise<boolean>;
  installSkill: (url: string) => Promise<boolean>;

  // MCP
  getMcpServers: () => Promise<GWMcpServerInfo[]>;
  enableMcpServer: (name: string) => Promise<boolean>;
  disableMcpServer: (name: string) => Promise<boolean>;
  removeMcpServer: (name: string) => Promise<boolean>;

  // Providers
  getProviders: () => Promise<GWProviderInfo[]>;
  setDefaultProvider: (name: string) => Promise<boolean>;

  // Memory
  clearMemory: () => Promise<void>;
  memoryStats: () => Promise<{ count: number; size_bytes: number }>;

  // Logs
  getLogLevel: () => Promise<string>;
  setLogLevel: (level: string) => Promise<void>;
  getLogDirectory: () => Promise<string>;
}

export const useGatewayStore = create<GatewayStore>((set, _get) => ({
  connectionState: 'disconnected',
  error: null,

  connect: async () => {
    set({ connectionState: 'connecting', error: null });
    try {
      await gateway.connect();
      set({ connectionState: 'connected' });
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Connection failed';
      set({ connectionState: 'error', error: message });
      throw error;
    }
  },

  disconnect: () => {
    gateway.disconnect();
    set({ connectionState: 'disconnected', error: null });
  },

  isConnected: () => gateway.isConnected,

  // Plugins
  getPlugins: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.pluginsList();
  },

  enablePlugin: async (id: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.pluginsEnable(id);
  },

  disablePlugin: async (id: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.pluginsDisable(id);
  },

  uninstallPlugin: async (id: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.pluginsUninstall(id);
  },

  installPlugin: async (url: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.pluginsInstall(url);
  },

  // Skills
  getSkills: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.skillsList();
  },

  deleteSkill: async (id: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.skillsDelete(id);
  },

  installSkill: async (url: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.skillsInstall(url);
  },

  // MCP
  getMcpServers: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.mcpListServers();
  },

  enableMcpServer: async (name: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.mcpEnableServer(name);
  },

  disableMcpServer: async (name: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.mcpDisableServer(name);
  },

  removeMcpServer: async (name: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.mcpRemoveServer(name);
  },

  // Providers
  getProviders: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.providersList();
  },

  setDefaultProvider: async (name: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.providersSetDefault(name);
  },

  // Memory
  clearMemory: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    await gateway.memoryClear();
  },

  memoryStats: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.memoryStats();
  },

  // Logs
  getLogLevel: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.logsGetLevel();
  },

  setLogLevel: async (level: string) => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    await gateway.logsSetLevel(level);
  },

  getLogDirectory: async () => {
    if (!gateway.isConnected) {
      throw new Error('Gateway not connected');
    }
    return await gateway.logsGetDirectory();
  },
}));

// Export gateway instance for direct access when needed
export { gateway };
