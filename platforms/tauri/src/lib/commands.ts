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

export interface Settings {
  general: GeneralSettings;
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
  saveSettings: (settings: Settings) => invoke('save_settings', { settings }),
};
