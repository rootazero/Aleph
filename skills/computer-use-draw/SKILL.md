---
name: computer-use-draw
description: "Drive a desktop drawing app or web canvas to produce a sketch from a text description. Use when the user says \"draw X\", \"paint Y\", \"sketch …\", or asks to use Figma/Excalidraw/Procreate/Photoshop/Paint to make something. Combines vision-mode browser/desktop driving with stroke planning."
---

# Computer-use recipe — Draw

## Trigger phrases

- "draw a CAT in Figma"
- "use Excalidraw to sketch …"
- "open paint and make …"
- "画图" / "画一张 …"

## Preconditions

1. Pick the canvas:
   - Web canvas (Excalidraw, tldraw) → `browser_open` + `mode: "vision"`
   - Desktop app (Paint, Procreate via macOS app) → `launch_app`
   - Figma → `browser_open` + try `mode: "hybrid"` (Figma exposes some
     a11y) then escalate to vision.
2. Confirm the target app & file/page with the user.
3. Drawing always needs `mode: "vision"` for the canvas itself — DOM
   doesn't describe pixel strokes.

## The loop

```
1. desktop_browser_operator { mode: "vision" }

2. Open the canvas:
   - browser_open { url: "https://excalidraw.com" }, OR
   - desktop launch_app { bundle_id: "com.figma.Desktop" }

3. desktop wait_visual { timeout_ms: 5000 }

4. Locate the toolbar:
   desktop_gui_locate { target_text: "Rectangle" | "Pencil" | "Brush" }
   → click to select tool.

5. Plan strokes from the description:
   - Decompose into 3-7 primitive shapes (circle, rect, line, freehand).
   - For each shape, compute (start_x, start_y, end_x, end_y) in the
     CANVAS coordinate system, not the full screen — grab the canvas
     bounds first via `desktop screenshot` and visual estimation, OR
     `desktop_ax_snapshot` for the canvas element when available.

6. For each stroke:
   a. Select the tool via desktop_gui_locate.
   b. desktop drag { start_x, start_y, end_x, end_y, duration_ms: 600 }
      — duration_ms > 200 lets the canvas register a deliberate stroke.
   c. For freehand: chunked drag, ~5 segments per intended curve.
   d. desktop wait_visual { timeout_ms: 800 }

7. After every 2-3 strokes, take a screenshot and show the user.
   Allow them to redirect ("make the body bigger", "delete that line").

8. When the user says it's good, save:
   desktop_gui_locate { target_text: "Save" | "Export" } → click.
   For web canvas, prefer `desktop key_combo { keys: ["meta", "s"] }`.

9. Confirm the saved file's location with the user before exiting.
```

## Verification checkpoints

- After step 4: a tool indicator highlights in the toolbar after click.
  If not, `desktop_gui_locate` likely returned the label instead of the
  button — retry with `prefer_role: "AXButton"` or `force_ocr: true`.
- After every 3 strokes: visible accumulation of marks on canvas.

## Failure recovery

| Symptom | Action |
|---------|--------|
| Stroke lands on wrong layer | Locate "Undo" or `key_combo ["meta", "z"]`. |
| Toolbar covered by sidebar | `desktop drag` to move the sidebar, or close it via gui_locate. |
| Canvas does not respond to drag | Click the canvas first to focus it, then retry. |
| App not installed | `desktop launch_app` returns failure → fall back to the web alternative. |

## Boundaries

- Don't produce content the user didn't ask for — stick to the
  described subject.
- Don't replicate copyrighted character art; redirect to abstract or
  user-original references.
- Don't sign the user's name on the work.

## Related

- [BROWSER_OPERATOR.md](../../docs/reference/BROWSER_OPERATOR.md)
- The `desktop_browser_operator` tool's "vision" mode manifest.
