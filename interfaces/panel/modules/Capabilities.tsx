// Every Panel tab the current snapshot declares, with its status and which
// renderer draws it.
import { Text, View } from "nsh";
import type { PanelTab } from "../shell/view";

export function Capabilities(props: { tabs: PanelTab[] }) {
  return (
    <View class="gap-2 p-4">
      <Each of={props.tabs} key={(tab) => tab.key}>
        {(tab) => (
          <View class="flex-row justify-between gap-3">
            <Text class="flex-1 text-foreground">{tab().label}</Text>
            <Text class="text-muted-foreground">
              {tab().status}
              {tab().renderer === "dom" ? " · classic Panel only" : ""}
            </Text>
          </View>
        )}
      </Each>
    </View>
  );
}
