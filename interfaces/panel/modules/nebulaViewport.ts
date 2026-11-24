import type { CanvasRenderingContext2D } from "nsh";

export type Point = [number, number];
export type Edge = { from: number; to: number; kind: string | null };
export type Scene = { points: Point[]; edges: Edge[] };

export function project(points: Point[], width: number, height: number, zoom: number): Point[] {
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
  const cx = (left + right) / 2;
  const cy = (top + bottom) / 2;
  const scale = zoom * Math.min(
    Math.max(1, width - 32) / Math.max(1, right - left),
    Math.max(1, height - 32) / Math.max(1, bottom - top),
  );
  return points.map(([x, y]) => [width / 2 + (x - cx) * scale, height / 2 + (y - cy) * scale]);
}

export function pickedNode(points: Point[], x: number, y: number): number {
  let closest = -1;
  let distance = 100;
  for (const [index, [nodeX, nodeY]] of points.entries()) {
    const candidate = (nodeX - x) ** 2 + (nodeY - y) ** 2;
    if (candidate < distance) {
      closest = index;
      distance = candidate;
    }
  }
  return closest;
}

function isIncident({ from, to }: Edge, selected: number): boolean {
  return from === selected || to === selected;
}

export function drawSlots(edges: Edge[], selected: number, budget: number): Edge[] {
  const emphasized = edges.filter((edge) => isIncident(edge, selected)).slice(0, budget);
  const background = edges.filter((edge) => !isIncident(edge, selected)).slice(0, budget - emphasized.length);
  return [...background, ...emphasized];
}

type EdgePen = Pick<CanvasRenderingContext2D, "strokeStyle" | "beginPath" | "moveTo" | "lineTo" | "stroke">;

export function drawEdge(pen: EdgePen, points: Point[], edge: Edge, selected: number): void {
  const [x, y] = points[edge.from];
  const [endX, endY] = points[edge.to];
  const emphasized = isIncident(edge, selected);
  if (edge.kind === "related_similarity") {
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
