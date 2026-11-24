// Reorder and hide tabs. Every change goes through `PanelModel` and is saved
// by the shell; this view only reports what the user pressed.
import { Text, View } from "nsh";
import { Button } from "nsh/base";
import type { PanelView } from "./view";

export function LayoutEditor(props: {
  view: PanelView;
  onMove: (key: string, forward: boolean) => void;
  onHidden: (key: string, hidden: boolean) => void;
}) {
  const label = (key: string) => props.view.tabs.find((tab) => tab.key === key)?.label ?? key;
  return (
    <View class="gap-1 border-b border-border px-3 py-2">
      <Each of={props.view.layout.order} key={(key) => key}>
        {(key) => {
          const hidden = () => props.view.layout.hidden.includes(key());
          return (
            <View class="flex-row items-center gap-2">
              <Text class={hidden() ? "flex-1 text-muted-foreground" : "flex-1 text-foreground"}>{label(key())}</Text>
              <Button accessibilityLabel={`Move ${label(key())} up`} onPress={() => props.onMove(key(), true)} class="px-2">
                <Text>↑</Text>
              </Button>
              <Button accessibilityLabel={`Move ${label(key())} down`} onPress={() => props.onMove(key(), false)} class="px-2">
                <Text>↓</Text>
              </Button>
              <Button
                accessibilityLabel={`${hidden() ? "Show" : "Hide"} ${label(key())}`}
                onPress={() => props.onHidden(key(), !hidden())}
                class="px-2"
              >
                <Text>{hidden() ? "Show" : "Hide"}</Text>
              </Button>
            </View>
          );
        }}
      </Each>
    </View>
  );
}
