// The Settings page in the right column: appearance, shortcuts and the menu.
// Every change goes through `PanelModel` and is saved by the shell; this view
// only reports what the user pressed.
import { type JSX, Scroll, Text, View } from "nsh";
import { Button } from "nsh/base";
import { LayoutEditor } from "./LayoutEditor";
import { DENSITY_CLASSES, type Prefs } from "./prefs";
import type { PanelView } from "./view";

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
  shortcuts: JSX.Element;
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
        {props.shortcuts}

        <Text class="text-lg text-foreground">Menu</Text>
        <If when={props.view()} else={<Text class="text-muted-foreground">Connect to Aleph to arrange the menu.</Text>}>
          {(current) => <LayoutEditor view={current()} onMove={props.onMove} onHidden={props.onHidden} />}
        </If>
      </View>
    </Scroll>
  );
}
