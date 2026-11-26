// The caller's sessions (`sessions.list`, already scoped by the Gateway). A
// row opens that session in the Chat tab.
import { List, signal, Text, View } from "nsh";
import { Button } from "nsh/base";
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
  const navigationFailed = signal(false);

  props.sdk.invoke("sessions.list").then(
    (result) => sessions.set((result as { sessions?: SessionRow[] } | null)?.sessions ?? []),
    (failure: Error) => {
      if (failure.message !== "disposed") error.set(failure.message);
    },
  );

  return (
    <View class="flex-1 p-4">
      <If when={navigationFailed()}>
        <Text class="text-destructive">Chat is hidden. Show it in Settings to open this conversation.</Text>
      </If>
      <If when={sessions()} else={<Text class="text-muted-foreground">{error() ?? "Loading sessions…"}</Text>}>
        {(rows) => (
          <List
            each={rows()}
            keyExtractor={(row) => row.key}
            empty={<Text class="text-muted-foreground">No sessions yet</Text>}
            class="flex-1"
          >
            {(row) => (
              <Button
                onPress={() => navigationFailed.set(!props.sdk.navigate("aleph:chat", { session_key: row().key }))}
                accessibilityLabel={`Open chat ${row().label || row().topic || row().key}`}
                class="flex-row justify-between gap-3 rounded-(--radius) px-2 py-2 active:bg-muted"
              >
                <Text class="flex-1 text-foreground">{row().label || row().topic || row().key}</Text>
                <Text class="text-muted-foreground">{row().message_count ?? 0} messages</Text>
              </Button>
            )}
          </List>
        )}
      </If>
    </View>
  );
}
