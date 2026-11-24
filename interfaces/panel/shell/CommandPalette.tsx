// The command palette: a search box over every command the shell knows,
// ranked by `PanelModel.rankCommands` (the same scoring as the Leptos
// palette). Running a command closes the palette first.
import { cleanup, computed, keymap, Overlay, signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";
import { PanelModel } from "aleph/panel";
import type { Command } from "./commands";

export function CommandPalette(props: { commands: () => Command[]; keyOf: (id: string) => string | null; onClose: () => void }) {
  const query = signal("");
  const selected = signal(0);

  const rows = computed(() => {
    const commands = props.commands();
    const items = commands.map((command) => ({ id: command.id, label: command.label, keywords: command.keywords ?? [] }));
    const byId = new Map(commands.map((command) => [command.id, command]));
    return PanelModel.rankCommands(query(), items)
      .map((id) => byId.get(id))
      .filter((command): command is Command => command !== undefined);
  });

  const run = (command: Command | undefined) => {
    if (!command) return;
    props.onClose();
    command.run();
  };

  const move = (step: number) => {
    const count = rows().length;
    if (count > 0) selected.set((selected() + step + count) % count);
  };
  // Arrow keys move the selection even while the search box has the focus.
  cleanup(keymap.bind({ up: () => move(-1), down: () => move(1) }, { when: "always" }));

  return (
    <Overlay class="inset-0 items-center bg-black/30 pt-16" onPressOutside={props.onClose} onCancel={props.onClose}>
      <View class="w-[32rem] max-w-full gap-1 rounded-(--radius) border border-border bg-popover p-2">
        <Input
          value={query()}
          onChange={({ value }) => {
            query.set(value);
            selected.set(0);
          }}
          onSubmit={() => run(rows()[selected()])}
          placeholder="Type a command"
          accessibilityLabel="Command search"
          autoFocus
          class="rounded-(--radius) border border-input px-3 py-2 text-foreground"
        />
        <Each of={rows()} key={(command) => command.id}>
          {(command, index) => {
            const key = () => props.keyOf(command().id);
            return (
              <Button
                onPress={() => run(command())}
                accessibilityLabel={command().label}
                class={`flex-row items-center rounded-(--radius) px-3 py-1.5 ${index() === selected() ? "bg-accent" : "active:bg-muted"}`}
              >
                <Text class="flex-1 text-popover-foreground">{command().label}</Text>
                <If when={key()}>{(shown) => <Text class="text-muted-foreground">{keymap.label(shown())}</Text>}</If>
              </Button>
            );
          }}
        </Each>
        <If when={rows().length === 0}>
          <Text class="px-3 py-1.5 text-muted-foreground">No matching command</Text>
        </If>
      </View>
    </Overlay>
  );
}
