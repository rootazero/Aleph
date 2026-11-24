import { Canvas, type GestureResponderEvent, signal, Text, View } from "nsh";
import { Button } from "nsh/base";

import { drawEdge, drawSlots, pickedNode, project, type Point, type Scene } from "./nebulaViewport";

export function NebulaCanvas(props: { scene: Scene; selected: () => number; onSelect: (index: number) => void }) {
  const zoom = signal(1);
  let drawn: Point[] = [];
  function pick(event: GestureResponderEvent) {
    const closest = pickedNode(drawn, event.locationX, event.locationY);
    if (closest >= 0) props.onSelect(closest);
    return true;
  }

  return (
    <View class="gap-1">
      <Canvas
        style={{ height: 190 }}
        accessibilityLabel="Memory graph"
        onStartShouldSetResponder={() => true}
        onResponderGrant={pick}
        draw={(cx, { width, height }) => {
          const selected = props.selected();
          const points = project(props.scene.points, width, height, zoom());
          drawn = points;
          cx.fillStyle = "#10182a";
          cx.fillRect(0, 0, width, height);
          for (const edge of drawSlots(props.scene.edges, selected, 320)) {
            drawEdge(cx, points, edge, selected);
          }
          for (const [index, [x, y]] of points.entries()) {
            cx.fillStyle = index === selected ? "#fbbf24" : "#93c5fd";
            cx.beginPath();
            cx.arc(x, y, index === selected ? 6 : 3, 0, Math.PI * 2);
            cx.fill();
          }
        }}
      />
      <View class="flex-row gap-1">
        <Button accessibilityLabel="Zoom out" onPress={() => zoom.update((value) => Math.max(0.5, value / 1.25))}><Text>−</Text></Button>
        <Button accessibilityLabel="Zoom in" onPress={() => zoom.update((value) => Math.min(4, value * 1.25))}><Text>+</Text></Button>
        <Button onPress={() => zoom.set(1)}><Text>Fit graph</Text></Button>
      </View>
    </View>
  );
}
