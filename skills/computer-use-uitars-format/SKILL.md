---
name: computer-use-uitars-format
description: "Drive the desktop with UI-TARS-style normalized coordinates and Pythonic action scripts — the same wire format a UI-TARS-finetuned VLM emits natively. Use when the user supplies a UI-TARS / Doubao-VL model (so coords are in [0,1000]×[0,1000] not pixels), or wants resolution-independent automation that survives display changes. Trigger phrases: 'UI-TARS format', 'normalized coordinates', 'Pythonic action', 'resolution-independent click', '归一化坐标'."
---

# Computer-use — UI-TARS coordinate + script format

This skill teaches the model to drive Aleph's `desktop` tool with the UI-TARS
contract instead of the default pixel-based JSON. Two opt-in surfaces:

1. **Normalized coordinates** — `coord_space:"normalized"` rescales
   `x`/`y`/`start_x`/`end_x`/`region` from `[0, factor_w] × [0, factor_h]`
   into the current display's physical pixels. Default factor is
   `[1000, 1000]` (UI-TARS V1.0/V1.5). Works for `click`, `double_click`,
   `drag`, `hover`, `scroll`, `screenshot` regions, and `batch`.
2. **Pythonic action script** — set `script` to one or more `Action: ...`
   lines and the tool expands them into a sequential batch. Supported verbs
   match UI-TARS: `click`, `left_double`, `right_single`, `drag`, `hover`,
   `type`, `hotkey`, `scroll`, `wait`, `finished`, `call_user`. Box formats
   `(x,y)`, `[x1,y1,x2,y2]`, `<point>x y</point>`, `<bbox>x1 y1 x2 y2</bbox>`
   all parse.

Both are **additive** — every existing pixel-space invocation still works
exactly as before. Mix freely.

## When to use this skill

- The active model is a UI-TARS / Doubao-VL fine-tune that emits Pythonic
  action calls in its `Thought: ... \nAction: ...` reply format.
- The same automation must run unchanged across multiple machines with
  different resolutions or DPRs (Retina, 4K external, etc.).
- The model is reasoning about visual proportions of the screen ("click the
  centre", "drag from top-left quadrant to bottom-right") rather than
  pixel-accurate UI elements.

If pixel coordinates from an `ax_snapshot` or `gui_locate` result are already
available — stick with pixel space. Don't round-trip through normalization
just for symmetry.

## How to use

### Normalized click

```json
{
  "action": "click",
  "coord_space": "normalized",
  "x": 500,
  "y": 500
}
```

On a 1920×1080 display this clicks (960, 540). On a 2560×1440 display it
clicks (1280, 720). The model never sees the resolution.

### Custom factor (e.g. Doubao 1.5 emits 1024×1024 coords)

```json
{
  "action": "click",
  "coord_space": "normalized",
  "coord_factors": [1024, 1024],
  "x": 512,
  "y": 512
}
```

### Batch with inherited coord_space

```json
{
  "action": "batch",
  "coord_space": "normalized",
  "actions": [
    {"action": "click", "x": 300, "y": 400},
    {"action": "type_text", "text": "hello world"},
    {"action": "key_combo", "keys": ["enter"]}
  ]
}
```

Sub-actions inherit `coord_space`/`coord_factors` from the batch when they
don't override.

### Pythonic action script

```json
{
  "action": "script",
  "coord_space": "normalized",
  "script": "Thought: open the menu.\nAction: click(start_box='(500,30)')\nAction: wait()\nAction: type(content='hello\\n')"
}
```

Tip: chain calls on a single line with `;`:

```json
{
  "action": "script",
  "coord_space": "normalized",
  "script": "click(start_box='(500,30)'); wait(); type(content='hi')"
}
```

`Thought:` lines are dropped silently — emit them freely as reasoning.

### Mixed bbox / point notation

```text
click(start_box='<point>500 500</point>')
click(start_box='<bbox>400 400 600 600</bbox>')   # centre = (500, 500)
click(start_box='[400, 400, 600, 600]')           # equivalent
```

Bboxes always use the **centre point** of the box as the click target.

## Verbs and what they map to

| UI-TARS verb | Aleph desktop action | Notes |
|---|---|---|
| `click`, `left_single`, `tap` | `click` | left button by default |
| `left_double`, `double_click` | `double_click` | |
| `right_single`, `right_click` | `click` with `button="right"` | |
| `drag`, `select` | `drag` | needs both `start_box` and `end_box` |
| `hover`, `move_to` | `hover` | |
| `type` | `type_text` | escapes `\n`, `\t`, `\\`, `\'`, `\"` |
| `hotkey` | `key_combo` | space-separated keys: `'ctrl shift a'` |
| `scroll` | `scroll` | direction → ±500 px on the matching axis |
| `wait` | `wait_visual` | polls until the screen settles |
| `finished` | reports done; Aleph loop terminates per `TerminateReason` |
| `call_user` | escalates back to the user — no action executed |

## Things that don't change

- Approval policy gating still runs against the resolved pixel coordinates.
- The session lock and Escape-abort listener behave identically.
- Hard-block safety (`rm -rf /`, fork bombs, lock-screen hotkeys) still
  inspects the typed text and key combos in every sub-action.
- All existing pixel-mode skills keep working — no migration required.

## When NOT to use

- The model is Claude / Sonnet / GPT-4o with JSON tool-calling — those emit
  pixel coords directly off `ax_snapshot.center`, which is more accurate
  than any normalized rescale.
- You're driving a single fixed display where pixel coords are stable.
- The exact UI element is already known via `gui_locate` — feed its pixel
  `center` straight into a pixel-mode click.
