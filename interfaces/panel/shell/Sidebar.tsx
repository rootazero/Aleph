// The fixed left column: one entry per menu slot (the user's layout, from
// `PanelModel.view`), then the connection state and Settings at the bottom.
// Settings is not a tab: it stays reachable with no Gateway and no layout.
import { Text, View } from "nsh";
import { Button } from "nsh/base";
import { DENSITY_CLASSES, type Density } from "./prefs";
import type { ConnectionState } from "./sdk";
import type { PanelView } from "./view";

const CONNECTION_LABELS: Record<ConnectionState["state"], string> = {
  connecting: "Connecting",
  connected: "Connected",
  disconnected: "Disconnected",
  unavailable: "Unavailable",
};

const CONNECTION_DOTS: Record<ConnectionState["state"], string> = {
  connecting: "bg-muted-foreground",
  connected: "bg-primary",
  disconnected: "bg-destructive",
  unavailable: "bg-destructive",
};

export function Sidebar(props: {
  view: () => PanelView | null;
  connection: () => ConnectionState;
  settingsOpen: () => boolean;
  compact: () => boolean;
  density: () => Density;
  onOpen: (key: string) => void;
  onSettings: () => void;
}) {
  const label = (key: string) => props.view()?.tabs.find((tab) => tab.key === key)?.label ?? key;
  // Compact width keeps a rail of initials; the accessibility label stays full.
  const shown = (text: string) => (props.compact() ? text.slice(0, 1).toUpperCase() : text);
  const row = () => `rounded-(--radius) ${DENSITY_CLASSES[props.density()].row}`;
  const selected = (on: boolean) => (on ? "bg-sidebar-accent" : "active:bg-muted");

  return (
    <View
      class={`${props.compact() ? "w-14" : "w-52"} border-r border-sidebar-border bg-sidebar p-2 ${DENSITY_CLASSES[props.density()].gap}`}
    >
      <View class={`flex-1 ${DENSITY_CLASSES[props.density()].gap}`}>
        <Each of={props.view()?.slots ?? []} key={(slot) => `${slot.key}#${slot.state}`}>
          {(slot) => (
            <If
              when={slot().state === "live"}
              else={<Text class={`${row()} text-muted-foreground`}>{shown(`${slot().key} · unavailable`)}</Text>}
            >
              <Button
                onPress={() => props.onOpen(slot().key)}
                accessibilityLabel={`Open ${label(slot().key)}`}
                class={`${row()} ${selected(!props.settingsOpen() && slot().key === props.view()?.layout.active)}`}
              >
                <Text class="text-sidebar-foreground">{shown(label(slot().key))}</Text>
              </Button>
            </If>
          )}
        </Each>
      </View>
      <View
        class={`flex-row items-center gap-2 ${row()}`}
        accessibilityLabel={`Connection: ${CONNECTION_LABELS[props.connection().state]}`}
      >
        <View class={`size-2 rounded-full ${CONNECTION_DOTS[props.connection().state]}`} />
        <If when={!props.compact()}>
          <Text class="text-muted-foreground">{CONNECTION_LABELS[props.connection().state]}</Text>
        </If>
      </View>
      <Button onPress={props.onSettings} accessibilityLabel="Settings" class={`${row()} ${selected(props.settingsOpen())}`}>
        <Text class="text-sidebar-foreground">{shown("Settings")}</Text>
      </Button>
    </View>
  );
}
