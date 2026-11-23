// The caller's sessions (`sessions.list`, already scoped by the Gateway).
import { List, signal, Text, View } from "nsh";
import type { PanelSdk } from "../shell/sdk";

type SessionRow = {
  key: string;
  label?: string | null;
  topic?: string | null;
  message_count?: number;
  last_active_at?: string;
};

export function Sessions(props: { sdk: PanelSdk }) {
  const sessions = signal<SessionRow[] | null>(null);
  const error = signal<string | null>(null);

  props.sdk.invoke("sessions.list").then(
    (result) => sessions.set((result as { sessions?: SessionRow[] } | null)?.sessions ?? []),
    (failure: Error) => {
      if (failure.message !== "disposed") error.set(failure.message);
    },
  );

  return (
    <View class="flex-1 p-4">
      <If when={sessions()} else={<Text class="text-muted-foreground">{error() ?? "Loading sessions…"}</Text>}>
        {(rows) => (
          <List
            each={rows()}
            keyExtractor={(row) => row.key}
            empty={<Text class="text-muted-foreground">No sessions yet</Text>}
            class="flex-1"
          >
            {(row) => (
              <View class="flex-row justify-between gap-3 py-2">
                <Text class="flex-1 text-foreground">{row().label || row().topic || row().key}</Text>
                <Text class="text-muted-foreground">{row().message_count ?? 0} messages</Text>
              </View>
            )}
          </List>
        )}
      </If>
    </View>
  );
}
