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

/** The stored preferences text, or `null` when there is none or it cannot be read. */
export async function loadStored(): Promise<string | null> {
  try {
    return await Storage.getItem(PREFS_KEY);
  } catch {
    return null;
  }
}

/** The preferences a stored text (or none) gives, repaired if needed. */
export function readPrefs(stored: string | null): Prefs {
  return PanelModel.prefs(stored) as Prefs;
}

// Writes run one after another, so the last edit is the one that stays
// stored even when an earlier write is still pending.
let pendingWrite: Promise<unknown> = Promise.resolve();

/**
 * Store a stored form the model returned. Resolves `false` when the device
 * refused it (storage full, unavailable); the edit still shows until the
 * Panel restarts.
 */
export function persistPrefs(stored: string): Promise<boolean> {
  const write = pendingWrite.then(() => Storage.setItem(PREFS_KEY, stored)).then(
    () => true,
    () => false,
  );
  pendingWrite = write;
  return write;
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
