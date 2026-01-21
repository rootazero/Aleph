import { create } from 'zustand';
import {
  commands,
  type Settings,
  type GeneralSettings,
  type ShortcutSettings,
  type BehaviorSettings,
  type ProvidersSettings,
  type GenerationSettings,
  type MemorySettings,
  type McpSettings,
  type PluginsSettings,
  type SkillsSettings,
  type AgentSettings,
  type SearchSettings,
  type PoliciesSettings,
} from '@/lib/commands';

interface SettingsStore {
  // Settings data
  general: GeneralSettings;
  shortcuts: ShortcutSettings;
  behavior: BehaviorSettings;
  providers: ProvidersSettings;
  generation: GenerationSettings;
  memory: MemorySettings;
  mcp: McpSettings;
  plugins: PluginsSettings;
  skills: SkillsSettings;
  agent: AgentSettings;
  search: SearchSettings;
  policies: PoliciesSettings;

  // State
  isDirty: boolean;
  isLoading: boolean;
  error: string | null;

  // Actions
  load: () => Promise<void>;
  updateGeneral: (partial: Partial<GeneralSettings>) => void;
  updateShortcuts: (partial: Partial<ShortcutSettings>) => void;
  updateBehavior: (partial: Partial<BehaviorSettings>) => void;
  updateProviders: (partial: Partial<ProvidersSettings>) => void;
  updateGeneration: (partial: Partial<GenerationSettings>) => void;
  updateMemory: (partial: Partial<MemorySettings>) => void;
  updateMcp: (partial: Partial<McpSettings>) => void;
  updatePlugins: (partial: Partial<PluginsSettings>) => void;
  updateSkills: (partial: Partial<SkillsSettings>) => void;
  updateAgent: (partial: Partial<AgentSettings>) => void;
  updateSearch: (partial: Partial<SearchSettings>) => void;
  updatePolicies: (partial: Partial<PoliciesSettings>) => void;
  save: () => Promise<void>;
  discard: () => void;
  hasChanges: () => boolean;
}

const defaultSettings: Settings = {
  general: {
    sound_enabled: true,
    launch_at_login: false,
    language: 'system',
  },
  shortcuts: {
    show_halo: 'Ctrl+Alt+Space',
    command_completion: 'Ctrl+Alt+/',
    toggle_listening: 'Ctrl+Alt+L',
    quick_capture: 'Ctrl+Alt+C',
  },
  behavior: {
    output_mode: 'replace',
    typing_speed: 50,
    auto_dismiss_delay: 3,
    show_notifications: true,
    pii_masking: false,
    pii_keywords: [],
  },
  providers: {
    providers: [],
    default_provider_id: '',
  },
  generation: {
    temperature: 0.7,
    max_tokens: 4096,
    top_p: 1.0,
    frequency_penalty: 0,
    presence_penalty: 0,
    streaming: true,
  },
  memory: {
    enabled: true,
    auto_save: true,
    max_history: 100,
    embedding_model: 'text-embedding-3-small',
    similarity_threshold: 0.7,
  },
  mcp: {
    servers: [],
  },
  plugins: {
    plugins: [],
    auto_update: true,
  },
  skills: {
    skills: [],
  },
  agent: {
    file_operations: true,
    code_execution: false,
    web_browsing: true,
    max_iterations: 10,
    require_confirmation: true,
    sandbox_mode: true,
    allowed_paths: [],
    blocked_commands: ['rm -rf', 'format', 'del /f'],
  },
  search: {
    web_search_enabled: true,
    search_engine: 'duckduckgo',
    max_results: 5,
    safe_search: true,
  },
  policies: {
    content_filter: true,
    filter_level: 'moderate',
    log_conversations: false,
    data_retention_days: 30,
    allow_analytics: false,
  },
};

let originalSettings: Settings | null = null;

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  // Initial state
  ...defaultSettings,
  isDirty: false,
  isLoading: false,
  error: null,

  load: async () => {
    set({ isLoading: true, error: null });
    try {
      const settings = await commands.getSettings();
      originalSettings = settings;
      set({
        ...settings,
        isDirty: false,
        isLoading: false,
      });
    } catch (error) {
      console.error('Failed to load settings:', error);
      // Use defaults on error
      originalSettings = defaultSettings;
      set({
        ...defaultSettings,
        error: String(error),
        isLoading: false,
      });
    }
  },

  updateGeneral: (partial) => {
    set((state) => ({
      general: { ...state.general, ...partial },
      isDirty: true,
    }));
  },

  updateShortcuts: (partial) => {
    set((state) => ({
      shortcuts: { ...state.shortcuts, ...partial },
      isDirty: true,
    }));
  },

  updateBehavior: (partial) => {
    set((state) => ({
      behavior: { ...state.behavior, ...partial },
      isDirty: true,
    }));
  },

  updateProviders: (partial) => {
    set((state) => ({
      providers: { ...state.providers, ...partial },
      isDirty: true,
    }));
  },

  updateGeneration: (partial) => {
    set((state) => ({
      generation: { ...state.generation, ...partial },
      isDirty: true,
    }));
  },

  updateMemory: (partial) => {
    set((state) => ({
      memory: { ...state.memory, ...partial },
      isDirty: true,
    }));
  },

  updateMcp: (partial) => {
    set((state) => ({
      mcp: { ...state.mcp, ...partial },
      isDirty: true,
    }));
  },

  updatePlugins: (partial) => {
    set((state) => ({
      plugins: { ...state.plugins, ...partial },
      isDirty: true,
    }));
  },

  updateSkills: (partial) => {
    set((state) => ({
      skills: { ...state.skills, ...partial },
      isDirty: true,
    }));
  },

  updateAgent: (partial) => {
    set((state) => ({
      agent: { ...state.agent, ...partial },
      isDirty: true,
    }));
  },

  updateSearch: (partial) => {
    set((state) => ({
      search: { ...state.search, ...partial },
      isDirty: true,
    }));
  },

  updatePolicies: (partial) => {
    set((state) => ({
      policies: { ...state.policies, ...partial },
      isDirty: true,
    }));
  },

  save: async () => {
    const state = get();
    const settings: Settings = {
      general: state.general,
      shortcuts: state.shortcuts,
      behavior: state.behavior,
      providers: state.providers,
      generation: state.generation,
      memory: state.memory,
      mcp: state.mcp,
      plugins: state.plugins,
      skills: state.skills,
      agent: state.agent,
      search: state.search,
      policies: state.policies,
    };

    try {
      await commands.saveSettings(settings);
      originalSettings = settings;
      set({ isDirty: false, error: null });
    } catch (error) {
      console.error('Failed to save settings:', error);
      set({ error: String(error) });
      throw error;
    }
  },

  discard: () => {
    if (originalSettings) {
      set({
        ...originalSettings,
        isDirty: false,
        error: null,
      });
    }
  },

  hasChanges: () => get().isDirty,
}));
