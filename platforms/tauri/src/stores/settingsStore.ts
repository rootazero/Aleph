import { create } from 'zustand';
import { commands, type Settings, type GeneralSettings } from '@/lib/commands';

interface SettingsStore {
  // Settings data
  general: GeneralSettings;

  // State
  isDirty: boolean;
  isLoading: boolean;
  error: string | null;

  // Actions
  load: () => Promise<void>;
  updateGeneral: (partial: Partial<GeneralSettings>) => void;
  save: () => Promise<void>;
  discard: () => void;
  hasChanges: () => boolean;
}

const defaultGeneralSettings: GeneralSettings = {
  sound_enabled: true,
  launch_at_login: false,
  language: 'system',
};

let originalSettings: Settings | null = null;

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  // Initial state
  general: defaultGeneralSettings,
  isDirty: false,
  isLoading: false,
  error: null,

  load: async () => {
    set({ isLoading: true, error: null });
    try {
      const settings = await commands.getSettings();
      originalSettings = settings;
      set({
        general: settings.general,
        isDirty: false,
        isLoading: false,
      });
    } catch (error) {
      console.error('Failed to load settings:', error);
      set({
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

  save: async () => {
    const { general } = get();
    const settings: Settings = { general };

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
        general: originalSettings.general,
        isDirty: false,
        error: null,
      });
    }
  },

  hasChanges: () => get().isDirty,
}));
