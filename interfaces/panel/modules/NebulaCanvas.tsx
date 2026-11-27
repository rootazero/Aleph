import { Canvas, type GestureResponderEvent, signal, Text, View } from "nsh";
import { Button } from "nsh/base";

import {
  beginDrag,
  clipEdge,
  drawSegment,
  drawSlots,
  fitViewport,
  isVisible,
  moveDrag,
  panBy,
  pickOnRelease,
  project,
  zoomAt,
  type DragState,
  type Point,
  type Scene,
  type Viewport,
} from "./nebulaViewport";

const EDGE_DENSITY = [160, 320, 640] as const;
const COMMUNITY_COLORS = ["#93c5fd", "#86efac", "#c4b5fd", "#fda4af", "#67e8f9", "#fdba74"];

export function NebulaCanvas(props: {
  scene: Scene;
  windowKey?: string;
  selected: () => number;
  onSelect: (index: number) => void;
  labels?: () => readonly string[];
  focusRequest?: () => { index: number; revision: number } | null;
}) {
  const viewport = signal<Viewport>({ scale: 1, x: 0, y: 0 });
  const density = signal(1);
  let frame = { width: 0, height: 0 };
  let fittedScene: Scene | undefined;
  let fittedWindow: string | undefined;
  let drag: DragState | null = null;
  let drawn: Point[] = [];
  let lastFocusRevision: number | null = null;

  function focus(index: number) {
    const { width, height } = frame;
    const point = props.scene.points[index];
    if (!point || width <= 0 || height <= 0) return;
    const scale = Math.max(viewport().scale, 1);
    viewport.set({ scale, x: width / 2 - point[0] * scale, y: height / 2 - point[1] * scale });
  }

  function setFrame(width: number, height: number) {
    if (width <= 0 || height <= 0) return;
    const windowChanged = props.windowKey === undefined ? fittedScene !== props.scene : fittedWindow !== props.windowKey;
    if (windowChanged) {
      viewport.set(fitViewport(props.scene.points, width, height));
    } else if (width !== frame.width || height !== frame.height) {
      viewport.set(panBy(viewport(), (width - frame.width) / 2, (height - frame.height) / 2));
    }
    if (fittedScene !== props.scene) drag = null;
    fittedScene = props.scene;
    fittedWindow = props.windowKey;
    frame = { width, height };
  }

  function grant(event: GestureResponderEvent) {
    drag = beginDrag(event.locationX, event.locationY);
    return true;
  }

  function move(event: GestureResponderEvent) {
    if (!drag) return;
    const moved = moveDrag(drag, event.locationX, event.locationY);
    drag = moved.drag;
    if (drag.moved) viewport.set(panBy(viewport(), moved.delta[0], moved.delta[1]));
  }

  function release(event: GestureResponderEvent) {
    if (!drag) return true;
    const { width, height } = frame;
    const index = pickOnRelease(drag, drawn, event.locationX, event.locationY, width, height);
    drag = null;
    if (index >= 0) props.onSelect(index);
    return true;
  }

  function zoom(factor: number, anchor?: Point) {
    const { width, height } = frame;
    viewport.set(zoomAt(viewport(), anchor ?? [width / 2, height / 2], factor));
  }

  return (
    <View class="flex-1 min-h-0 gap-1">
      <Canvas
        class="flex-1 min-h-0"
        accessibilityLabel="Memory graph"
        onStartShouldSetResponder={() => true}
        onResponderGrant={grant}
        onResponderMove={move}
        onResponderRelease={release}
        onResponderTerminate={() => { drag = null; }}
        onWheel={(event) => {
          if (event.modifiers.control) zoom(Math.exp(-event.delta.y * 0.01), [event.position.x, event.position.y]);
          else viewport.set(panBy(viewport(), -event.delta.x, -event.delta.y));
        }}
        onPinch={(event) => {
          const factor = event.delta > 0 ? 1 + event.delta : 1 / (1 - event.delta);
          zoom(factor, [event.position.x, event.position.y]);
        }}
        draw={(cx, { width, height }) => {
          setFrame(width, height);
          const selected = props.selected();
          const request = props.focusRequest?.() ?? null;
          if (request && request.revision !== lastFocusRevision) {
            lastFocusRevision = request.revision;
            focus(request.index);
          }
          const points = project(props.scene.points, width, height, viewport());
          drawn = points;
          cx.fillStyle = "#10182a";
          cx.fillRect(0, 0, width, height);
          for (const edge of drawSlots(props.scene.edges, selected, EDGE_DENSITY[density()])) {
            const clipped = clipEdge(points, edge, width, height);
            if (!clipped) continue;
            drawSegment(cx, clipped[0], clipped[1], edge.kind, selected === edge.from || selected === edge.to, edge.surprising);
          }
          const labels = props.labels?.() ?? [];
          for (const [index, [x, y]] of points.entries()) {
            if (!isVisible([x, y], width, height, 12)) continue;
            const emphasized = index === selected;
            const community = props.scene.communities?.[index] ?? 0;
            cx.fillStyle = emphasized ? "#fbbf24" : COMMUNITY_COLORS[community % COMMUNITY_COLORS.length];
            const radius = emphasized ? 6 : 3;
            cx.beginPath();
            cx.arc(x, y, radius, 0, Math.PI * 2);
            cx.fill();
            if (props.scene.bridges?.[index]) {
              cx.strokeStyle = "#f9a8d4";
              cx.beginPath();
              cx.arc(x, y, radius + 2, 0, Math.PI * 2);
              cx.stroke();
            }
            const label = labels[index];
            if (label && (emphasized || viewport().scale > 1.4)) {
              cx.fillStyle = "#dbeafe";
              cx.font = "11px sans-serif";
              cx.fillText(label, x + 8, y - 6);
            }
          }
        }}
      />
      <View class="flex-row gap-1">
        <Button accessibilityLabel="Zoom out" onPress={() => zoom(1 / 1.25)}><Text>−</Text></Button>
        <Button accessibilityLabel="Zoom in" onPress={() => zoom(1.25)}><Text>+</Text></Button>
        <Button accessibilityLabel="Fit graph" onPress={() => viewport.set(fitViewport(props.scene.points, frame.width, frame.height))}><Text>Fit graph</Text></Button>
        <Button accessibilityLabel="Show fewer relations" onPress={() => density.set(Math.max(0, density() - 1))} disabled={density() === 0}><Text>Fewer links</Text></Button>
        <Button accessibilityLabel="Show more relations" onPress={() => density.set(Math.min(EDGE_DENSITY.length - 1, density() + 1))} disabled={density() === EDGE_DENSITY.length - 1}><Text>More links</Text></Button>
      </View>
    </View>
  );
}
