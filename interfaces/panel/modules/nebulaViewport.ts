import type { CanvasRenderingContext2D } from "nsh";

export type Point = [number, number];
export type Edge = { from: number; to: number; kind: string | null };
export type Scene = { points: Point[]; edges: Edge[]; communities?: number[]; insights?: string[] };
export type Viewport = { scale: number; x: number; y: number };
export type DragState = { x: number; y: number; startX: number; startY: number; moved: boolean };

const MIN_SCALE = 0.2;
const MAX_SCALE = 8;
const DRAG_THRESHOLD = 5;

export function fitViewport(points: Point[], width: number, height: number, zoom = 1): Viewport {
  if (points.length === 0 || width <= 0 || height <= 0) return { scale: zoom, x: width / 2, y: height / 2 };
  let left = Infinity;
  let right = -Infinity;
  let top = Infinity;
  let bottom = -Infinity;
  for (const [x, y] of points) {
    left = Math.min(left, x);
    right = Math.max(right, x);
    top = Math.min(top, y);
    bottom = Math.max(bottom, y);
  }
  const scale = zoom * Math.min(
    Math.max(1, width - 48) / Math.max(1, right - left),
    Math.max(1, height - 48) / Math.max(1, bottom - top),
  );
  return { scale, x: width / 2 - ((left + right) / 2) * scale, y: height / 2 - ((top + bottom) / 2) * scale };
}

export function project(points: Point[], width: number, height: number, zoom: number | Viewport): Point[] {
  const viewport = typeof zoom === "number" ? fitViewport(points, width, height, zoom) : zoom;
  return points.map(([x, y]) => [viewport.x + x * viewport.scale, viewport.y + y * viewport.scale]);
}

export function inverseProject(point: Point, viewport: Viewport): Point {
  return [(point[0] - viewport.x) / viewport.scale, (point[1] - viewport.y) / viewport.scale];
}

export function zoomAt(viewport: Viewport, anchor: Point, factor: number): Viewport {
  const scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, viewport.scale * factor));
  const ratio = scale / viewport.scale;
  return { scale, x: anchor[0] - (anchor[0] - viewport.x) * ratio, y: anchor[1] - (anchor[1] - viewport.y) * ratio };
}

export function panBy(viewport: Viewport, dx: number, dy: number): Viewport {
  return { ...viewport, x: viewport.x + dx, y: viewport.y + dy };
}

export function beginDrag(x: number, y: number): DragState {
  return { x, y, startX: x, startY: y, moved: false };
}

export function moveDrag(drag: DragState, x: number, y: number): { drag: DragState; delta: Point } {
  const moved = drag.moved || Math.hypot(x - drag.startX, y - drag.startY) >= DRAG_THRESHOLD;
  return { drag: { ...drag, x, y, moved }, delta: moved ? [x - drag.x, y - drag.y] : [0, 0] };
}

export function endDrag(drag: DragState, x: number, y: number): { dragged: boolean; point: Point } {
  return { dragged: drag.moved || Math.hypot(x - drag.startX, y - drag.startY) >= DRAG_THRESHOLD, point: [x, y] };
}

export function isVisible(point: Point, width: number, height: number, margin = 18): boolean {
  return point[0] >= -margin && point[0] <= width + margin && point[1] >= -margin && point[1] <= height + margin;
}

export function pickedNode(points: Point[], x: number, y: number, width = Infinity, height = Infinity): number {
  let closest = -1;
  let distance = 100;
  for (const [index, [nodeX, nodeY]] of points.entries()) {
    if (!isVisible([nodeX, nodeY], width, height)) continue;
    const candidate = (nodeX - x) ** 2 + (nodeY - y) ** 2;
    if (candidate < distance) {
      closest = index;
      distance = candidate;
    }
  }
  return closest;
}

export function pickOnRelease(drag: DragState, points: Point[], x: number, y: number, width = Infinity, height = Infinity): number {
  const result = endDrag(drag, x, y);
  return result.dragged ? -1 : pickedNode(points, result.point[0], result.point[1], width, height);
}

function isIncident({ from, to }: Edge, selected: number): boolean {
  return from === selected || to === selected;
}

export function drawSlots(edges: Edge[], selected: number, budget: number): Edge[] {
  if (budget <= 0) return [];
  const emphasized = edges.filter((edge) => isIncident(edge, selected)).slice(0, budget);
  const background = edges.filter((edge) => !isIncident(edge, selected)).slice(0, budget - emphasized.length);
  return [...background, ...emphasized];
}

type EdgePen = Pick<CanvasRenderingContext2D, "strokeStyle" | "beginPath" | "moveTo" | "lineTo" | "stroke">;

export function clipEdge(points: Point[], edge: Edge, width: number, height: number, margin = 12): [Point, Point] | null {
  const start = points[edge.from];
  const end = points[edge.to];
  if (!start || !end || width <= 0 || height <= 0) return null;
  const dx = end[0] - start[0];
  const dy = end[1] - start[1];
  let low = 0;
  let high = 1;
  const tests: [number, number][] = [[-dx, start[0] - margin], [dx, width - margin - start[0]], [-dy, start[1] - margin], [dy, height - margin - start[1]]];
  for (const [p, q] of tests) {
    if (p === 0) { if (q < 0) return null; continue; }
    const t = q / p;
    if (p < 0) low = Math.max(low, t); else high = Math.min(high, t);
    if (low > high) return null;
  }
  return [[start[0] + low * dx, start[1] + low * dy], [start[0] + high * dx, start[1] + high * dy]];
}

export function drawSegment(pen: EdgePen, start: Point, end: Point, kind: string | null, emphasized: boolean): void {
  const [x, y] = start;
  const [endX, endY] = end;
  if (kind === "related_similarity") {
    pen.strokeStyle = emphasized ? "#d8b4fe" : "#745c90";
    pen.beginPath();
    pen.moveTo(x, y);
    pen.lineTo(endX, endY);
    pen.stroke();
    return;
  }
  const length = Math.hypot(endX - x, endY - y);
  if (length < 10) return;
  const ux = (endX - x) / length;
  const uy = (endY - y) / length;
  const tipX = endX - ux * 6 - uy * 2;
  const tipY = endY - uy * 6 + ux * 2;
  pen.strokeStyle = emphasized ? "#7dd3fc" : "#3e506d";
  pen.beginPath();
  pen.moveTo(x - uy * 2, y + ux * 2);
  pen.lineTo(tipX, tipY);
  pen.moveTo(tipX - ux * 5 - uy * 2, tipY - uy * 5 + ux * 2);
  pen.lineTo(tipX, tipY);
  pen.lineTo(tipX - ux * 5 + uy * 2, tipY - uy * 5 - ux * 2);
  pen.stroke();
}

export function drawEdge(pen: EdgePen, points: Point[], edge: Edge, selected: number): void {
  const start = points[edge.from];
  const end = points[edge.to];
  if (start && end) drawSegment(pen, start, end, edge.kind, isIncident(edge, selected));
}

export function drawClippedEdge(pen: EdgePen, points: Point[], edge: Edge, selected: number, width: number, height: number): void {
  const clipped = clipEdge(points, edge, width, height);
  if (clipped) drawSegment(pen, clipped[0], clipped[1], edge.kind, isIncident(edge, selected));
}
