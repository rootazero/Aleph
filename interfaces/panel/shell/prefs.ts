// Personal preferences: read and write them, and apply appearance to the
// running app. The rules (defaults, repair, the theme tokens) live in Rust
// (`PanelModel.prefs`, `shared_ui_logic::prefs`); this file only wires them.
import { host, setColorScheme, setTheme, Storage, type Theme } from "nsh";
import { PanelModel } from "aleph/panel";

// One per device, not per Gateway: same as `shared_ui_logic::prefs::PREFS_STORAGE_KEY`.
export const PREFS_KEY = "aleph.panel.prefs.v1";

export type ThemeMode = "system" | "light" | "dark";
export type Accent = "mauve" | "ocean" | "forest" | "sunset" | "rose";
export type FontScale = "compact" | "default" | "cozy" | "large" | "largest";
export type Roundness = "sharp" | "slight" | "default" | "round" | "extra";
export type Density = "compact" | "cozy" | "spacious";

export type Appearance = {
  mode: ThemeMode;
  accent: Accent;
  fontScale: FontScale;
  roundness: Roundness;
  density: Density;
};

export type Binding = { key: string } | "cleared";

// The shape `PanelModel.prefs` returns (`src/lib.rs`).
export type Prefs = {
  appearance: Appearance;
  keybindings: Record<string, Binding>;
  stored: string;
  repair: "corrupt" | "unknown_version" | null;
  theme: Theme;
  fontFactor: number | null;
  density: Density;
};

// Row padding and gaps per density. NSH fixes its spacing unit at build
// time, so density is a class choice rather than a theme token.
export const DENSITY_CLASSES: Record<Density, { row: string; gap: string }> = {
  compact: { row: "px-2 py-1", gap: "gap-1" },
  cozy: { row: "px-3 py-1.5", gap: "gap-1.5" },
  spacious: { row: "px-3 py-2", gap: "gap-2" },
};

/** Read stored preferences; a repaired form is written back at once. */
export async function loadPrefs(): Promise<Prefs> {
  const stored = await Storage.getItem(PREFS_KEY);
  const prefs = PanelModel.prefs(stored) as Prefs;
  if (stored !== null && prefs.stored !== stored) await Storage.setItem(PREFS_KEY, prefs.stored);
  return prefs;
}

/** Save a stored form the model returned and read it back. */
export async function savePrefs(stored: string): Promise<Prefs> {
  await Storage.setItem(PREFS_KEY, stored);
  return PanelModel.prefs(stored) as Prefs;
}

/**
 * Apply colours, corners and light/dark to this app, and hand the text size
 * to the host. The answer says whether the host could apply the text size.
 */
export async function applyPrefs(prefs: Prefs): Promise<{ fontScale: "applied" | "unsupported" }> {
  setTheme(prefs.theme);
  setColorScheme(prefs.appearance.mode);
  if (!host.available()) return { fontScale: "unsupported" };
  try {
    await host.call("aleph.fontScale", prefs.fontFactor);
    return { fontScale: "applied" };
  } catch {
    return { fontScale: "unsupported" };
  }
}
