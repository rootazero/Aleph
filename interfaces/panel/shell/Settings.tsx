// The Settings page in the right column: appearance, shortcuts and the menu.
// Every change goes through `PanelModel` and is saved by the shell; this view
// only reports what the user pressed.
import { keymap, Scroll, signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { PanelModel } from "aleph/panel";
import { APPLE, type ResolvedKey } from "./commands";
import { LayoutEditor } from "./LayoutEditor";
import { type Binding, DENSITY_CLASSES, type Prefs } from "./prefs";
import type { PanelView } from "./view";

/** One command's row on the Shortcuts list. */
export type ShortcutRow = {
  id: string;
  label: string;
  resolved: ResolvedKey;
  /** The platform refused the key (a reserved shortcut such as ⌘Q). */
  failure: string | null;
};

type Axis = {
  axis: keyof Prefs["appearance"];
  title: string;
  options: { value: string; label: string }[];
};

const AXES: Axis[] = [
  {
    axis: "mode",
    title: "Mode",
    options: [
      { value: "system", label: "System" },
      { value: "light", label: "Light" },
      { value: "dark", label: "Dark" },
    ],
  },
  {
    axis: "accent",
    title: "Accent",
    options: [
      { value: "mauve", label: "Mauve" },
      { value: "ocean", label: "Ocean" },
      { value: "forest", label: "Forest" },
      { value: "sunset", label: "Sunset" },
      { value: "rose", label: "Rose" },
    ],
  },
  {
    axis: "fontScale",
    title: "Text size",
    options: [
      { value: "compact", label: "Small" },
      { value: "default", label: "Default" },
      { value: "cozy", label: "Medium" },
      { value: "large", label: "Large" },
      { value: "largest", label: "Largest" },
    ],
  },
  {
    axis: "roundness",
    title: "Corners",
    options: [
      { value: "sharp", label: "Sharp" },
      { value: "slight", label: "Slight" },
      { value: "default", label: "Default" },
      { value: "round", label: "Round" },
      { value: "extra", label: "Extra round" },
    ],
  },
  {
    axis: "density",
    title: "Density",
    options: [
      { value: "compact", label: "Compact" },
      { value: "cozy", label: "Cozy" },
      { value: "spacious", label: "Spacious" },
    ],
  },
];

export function Settings(props: {
  prefs: () => Prefs;
  fontScaleSupported: () => boolean;
  onAppearance: (axis: string, value: string) => void;
  view: () => PanelView | null;
  onMove: (key: string, forward: boolean) => void;
  onHidden: (key: string, hidden: boolean) => void;
  shortcuts: () => ShortcutRow[];
  labelOf: (id: string) => string;
  onBinding: (id: string, binding: Binding | null) => void;
}) {
  const gap = () => DENSITY_CLASSES[props.prefs().density].gap;
  return (
    <Scroll class="flex-1">
      <View class={`p-4 ${gap()}`}>
        <Text class="text-lg text-foreground">Appearance</Text>
        <Each of={AXES} key={(axis) => axis.axis}>
          {(axis) => (
            <View class={gap()}>
              <Text class="text-muted-foreground">{axis().title}</Text>
              <View class={`flex-row flex-wrap ${gap()}`}>
                <Each of={axis().options} key={(option) => option.value}>
                  {(option) => {
                    const chosen = () => props.prefs().appearance[axis().axis] === option().value;
                    return (
                      <Button
                        accessibilityLabel={`${axis().title} ${option().value}`}
                        onPress={() => props.onAppearance(axis().axis, option().value)}
                        class={`rounded-(--radius) border px-3 py-1 ${chosen() ? "border-primary" : "border-border"}`}
                      >
                        <Text class={chosen() ? "text-foreground" : "text-muted-foreground"}>
                          {chosen() ? `${option().label} ✓` : option().label}
                        </Text>
                      </Button>
                    );
                  }}
                </Each>
              </View>
              <If when={axis().axis === "fontScale" && !props.fontScaleSupported()}>
                <Text class="text-muted-foreground">Not supported here</Text>
              </If>
            </View>
          )}
        </Each>

        <Text class="text-lg text-foreground">Shortcuts</Text>
        <Each of={props.shortcuts()} key={(row) => row.id}>
          {(row) => <ShortcutEditor row={row} labelOf={props.labelOf} onBinding={props.onBinding} />}
        </Each>

        <Text class="text-lg text-foreground">Menu</Text>
        <If when={props.view()} else={<Text class="text-muted-foreground">Connect to Aleph to arrange the menu.</Text>}>
          {(current) => <LayoutEditor view={current()} onMove={props.onMove} onHidden={props.onHidden} />}
        </If>
      </View>
    </Scroll>
  );
}

function problemText(row: ShortcutRow, labelOf: (id: string) => string): string | null {
  const problem = row.resolved.problem;
  if (problem === "invalid") return "Invalid shortcut";
  if (problem) return `Conflicts with ${labelOf(problem.conflict)}`;
  if (row.failure) return `Reserved by the system: ${row.failure}`;
  return null;
}

function ShortcutEditor(props: {
  row: () => ShortcutRow;
  labelOf: (id: string) => string;
  onBinding: (id: string, binding: Binding | null) => void;
}) {
  const draft = signal("");
  const invalid = signal(false);
  const current = () => {
    const key = props.row().resolved.key;
    return key ? keymap.label(key) : "None";
  };
  // The text is stored as typed (`mod+j`), so it means the same key on the
  // user's other platforms; it is only checked here.
  const submit = () => {
    const text = draft().trim().toLowerCase();
    if (!text) return;
    if (PanelModel.normalizeKeystroke(text, APPLE) === null) {
      invalid.set(true);
      return;
    }
    invalid.set(false);
    draft.set("");
    props.onBinding(props.row().id, { key: text });
  };
  return (
    <View class="gap-1">
      <View class="flex-row items-center gap-2">
        <Text class="flex-1 text-foreground">{props.row().label}</Text>
        <Text class="text-muted-foreground">{current()}</Text>
        <Input
          value={draft()}
          onChange={({ value }) => draft.set(value)}
          onSubmit={submit}
          placeholder="mod+k"
          accessibilityLabel={`Shortcut for ${props.row().label}`}
          class="w-28 rounded-(--radius) border border-input px-2 py-1 text-foreground"
        />
        <Button
          accessibilityLabel={`Clear shortcut for ${props.row().label}`}
          onPress={() => props.onBinding(props.row().id, "cleared")}
          class="px-2"
        >
          <Text>Clear</Text>
        </Button>
        <If when={props.row().resolved.source !== "default"}>
          <Button
            accessibilityLabel={`Reset shortcut for ${props.row().label}`}
            onPress={() => props.onBinding(props.row().id, null)}
            class="px-2"
          >
            <Text>Reset</Text>
          </Button>
        </If>
      </View>
      <If when={invalid() ? "Invalid shortcut" : problemText(props.row(), props.labelOf)}>
        {(text) => <Text class="text-destructive">{text()}</Text>}
      </If>
    </View>
  );
}
