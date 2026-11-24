// The command table (shell, menu and module commands) and its keyboard
// bindings. Which key a command gets — defaults, the user's overrides,
// conflicts, invalid text — is decided in Rust (`PanelModel.resolveBindings`,
// `shared_ui_logic::prefs::commands`); this file binds the outcome.
import { keymap } from "nsh";
import { PanelModel } from "aleph/panel";
import type { CommandSpec } from "./sdk";
import type { PanelView } from "./view";

export type Command = CommandSpec & { group: "shell" | "menu" | "module" };

// The shape `PanelModel.resolveBindings` returns (`src/lib.rs`).
export type ResolvedKey = {
  id: string;
  key: string | null;
  source: "default" | "user" | "cleared";
  problem: null | "invalid" | { conflict: string };
};

// Whether `mod` means Command here: NSH labels it ⌘ on Apple platforms and
// in a browser on one. Rust needs the same answer to normalise keystrokes.
export const APPLE = keymap.label("mod+k").includes("⌘");

const SLOT_SHORTCUTS = 9;

/**
 * The shell's own commands. `web` drops the default `mod+1…9`: browsers keep
 * those for switching tabs and the page never sees them.
 */
export function shellCommands(
  actions: {
    palette(): void;
    settings(): void;
    toggleScheme(): void;
    reconnect(): void;
    openSlot(index: number): void;
  },
  web: boolean,
): Command[] {
  const commands: Command[] = [
    { id: "shell.palette", label: "Command palette", keywords: ["search"], defaultKey: "mod+k", run: actions.palette, group: "shell" },
    { id: "shell.settings", label: "Settings", keywords: ["preferences"], defaultKey: "mod+,", run: actions.settings, group: "shell" },
    { id: "shell.scheme", label: "Toggle light and dark", keywords: ["theme", "mode"], run: actions.toggleScheme, group: "shell" },
    { id: "shell.reconnect", label: "Reconnect to Aleph", keywords: ["connect"], run: actions.reconnect, group: "shell" },
  ];
  for (let index = 0; index < SLOT_SHORTCUTS; index += 1) {
    commands.push({
      id: `shell.slot.${index + 1}`,
      label: `Open menu entry ${index + 1}`,
      defaultKey: web ? undefined : `mod+${index + 1}`,
      run: () => actions.openSlot(index),
      group: "shell",
    });
  }
  return commands;
}

/** One "Open X" per live menu entry, community tabs included. */
export function menuCommands(view: PanelView | null, open: (key: string) => void): Command[] {
  if (!view) return [];
  return view.slots
    .filter((slot) => slot.state === "live")
    .map((slot) => {
      const label = view.tabs.find((tab) => tab.key === slot.key)?.label ?? slot.key;
      return { id: `open:${slot.key}`, label: `Open ${label}`, keywords: [slot.key], run: () => open(slot.key), group: "menu" as const };
    });
}

/** Each command's key for the stored preferences. */
/** What key resolution needs of a command: no callback. */
export type KeyedCommand = Pick<Command, "id" | "label" | "defaultKey">;

export function resolveKeys(commands: KeyedCommand[], stored: string): ResolvedKey[] {
  const listed = commands.map((command) => ({ id: command.id, defaultKey: command.defaultKey ?? null }));
  return PanelModel.resolveBindings(listed, stored, APPLE) as ResolvedKey[];
}

/**
 * Bind every key that resolved without a problem, one `keymap.bind` each: a
 * key the platform reserves (⌘Q) throws, and only that command loses its key.
 * `failures` maps such a command to the platform's message.
 */
export function bindCommands(commands: Command[], resolved: ResolvedKey[]): { unbind(): void; failures: Map<string, string> } {
  const byId = new Map(commands.map((command) => [command.id, command]));
  const unbinders: (() => void)[] = [];
  const failures = new Map<string, string>();
  for (const entry of resolved) {
    const command = byId.get(entry.id);
    if (!command || !entry.key || entry.problem) continue;
    try {
      unbinders.push(keymap.bind(entry.key, () => command.run()));
    } catch (error) {
      failures.set(entry.id, (error as Error).message);
    }
  }
  return {
    unbind() {
      for (const unbind of unbinders) unbind();
    },
    failures,
  };
}
