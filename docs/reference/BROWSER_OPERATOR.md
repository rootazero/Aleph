# BROWSER_OPERATOR.md — Browser-driving strategy in Aleph

> Status: Reference. Updated 2026-05-27.
>
> Companion to the `desktop_browser_operator` builtin tool.

## Why this exists

UI-TARS-desktop's `AbstractBrowserControlStrategy` factors browser
automation into three named strategies — DOM, Hybrid, VisualGrounding —
implemented as TypeScript classes the agent loop instantiates.

In Aleph, R7 (LLM Sovereignty), R8 (Everything is a Tool), and R10 (Thin
Harness) push the same factoring *out of code* and *into the prompt*. The
strategy lives as data returned by `desktop_browser_operator`; the model
reads the manifest and dispatches to existing tools directly. No new
runtime layer. No `Strategy` class hierarchy. No orchestrator that picks
or hot-swaps strategies — the LLM does that.

This file documents the three strategies, the Aleph tools that implement
them, and the decision rules for picking one.

## The three strategies

### DOM

Use when the browser is a managed profile under Aleph's
`browser/manager`, the page uses standard HTML/ARIA, and the agent can
trust selectors to address elements.

```
browser_open      → attach to a profile + open URL
browser_snapshot  → DOM accessibility snapshot (element refs)
browser_click     → click by ref (no coordinates)
browser_type      → type into a ref
browser_select    → pick <option>
browser_wait_for  → wait for selector / URL / network idle
```

Fast. Robust to viewport changes. Fails on:
- Cross-origin frames the manager can't introspect
- Canvas / WebGL widgets
- Shadow-DOM components that hide from the snapshot
- Anti-bot-protected pages that hide elements from automation

### Hybrid (default)

Use when the page is mostly DOM-friendly but has visual islands.
Combines structured DOM action with pixel-level fallback.

```
browser_open
browser_snapshot               → try DOM first
  on miss:
    desktop_gui_locate         → find by visible label (AX tree + OCR)
    desktop click {x,y}        → from gui_locate's `center`
browser_wait_for OR
  desktop wait_visual          → after each mutation
browser_screenshot             → visual ground truth, periodic
```

Slower than pure DOM by one extra screenshot per action. Best safety net
when you don't know what the page will throw at you. Recommended default
for *any* user-facing task ("book the flight", "fill this form") where
the page hasn't been pre-vetted.

### Vision

Use when:
- Driving a third-party browser via system input (not the managed
  profile)
- The page actively defeats DOM automation (canvas-only chat, heavy
  obfuscation)
- You're inside an Electron app whose webview is opaque

```
desktop screenshot             → ground truth
desktop_gui_locate
  { target_text, force_ocr: true }  → pure OCR grounding
desktop click {x,y}
desktop wait_visual
```

Slowest path. Coordinates can drift on viewport resize. Use sparingly.

## Picking a strategy at runtime

The model picks. There's no `if/else` in Rust that selects the strategy
— the prompt template encourages this decision tree:

1. **Is the page pre-vetted and known to be DOM-friendly?** → `dom`
2. **Is this user-facing automation against an unknown site?** → `hybrid`
3. **Has the DOM path failed twice on the same target?** → escalate to
   `hybrid`, then `vision`.
4. **Is the target inside a canvas / webview / OS dialog?** → `vision`.

The `desktop_browser_operator` tool returns a `flow` array and `notes`
that re-state this for the model.

## Why no `BrowserGUIAgent` class

UI-TARS ships `BrowserGUIAgent` — a class that owns a browser session, a
strategy, and a screenshot cache. In Aleph that responsibility is split
three ways and re-assembled by the model:

| Concern | Aleph tool |
|--------|-----------|
| Session ownership | `browser_open` + the `browser/manager` profile pool |
| Strategy / flow | `desktop_browser_operator` manifest |
| Screenshot ground truth | `browser_screenshot`, `desktop` (screenshot action) |
| Grounding | `browser_snapshot`, `desktop_ax_snapshot`, `desktop_gui_locate` |
| Acting | `browser_*` for DOM, `desktop` for pixel |
| Settling | `browser_wait_for`, `desktop` (wait_visual action) |

Each tool is independently useful. There is no `BrowserGUIAgent` because
there is no business logic to assemble them — that's prompt territory.

## Relationship to existing Aleph tools

- `browser/` (Rust crate) hosts the managed-profile manager and the
  `browser_*` builtin tools. Strategy DOM lives here.
- `desktop/shared/` provides `ScreenCapability` / `AccessibilityCapability`.
  Strategy Vision lives on top of these. `wait_visual` and
  `gui_locate` are pure orchestration over them.
- `desktop_browser_operator` (this PR) is the cataloguer; it knows about
  both worlds but doesn't itself drive anything.

## Skill recipes

Per R9, the actual "how to book a hotel" knowledge lives in skill
prompts, not in this code. See:

- `skills/computer-use-book-flight/SKILL.md`
- `skills/computer-use-book-hotel/SKILL.md`
- `skills/computer-use-draw/SKILL.md`

Each recipe calls `desktop_browser_operator` first to pin a strategy,
then runs the see → locate → act → verify loop.

## R-line mapping

- **R3 (Core Minimalism)** — no new heavy runtime layer; the manifest is
  a few hundred lines of pure data + a tool that hands it out.
- **R7 (LLM Sovereignty)** — strategy selection stays in the model. No
  rules engine. No "pick best strategy" function.
- **R8 (Everything is a Tool)** — the strategy itself is exposed as a
  tool call.
- **R9 (Intelligence Lives in the Prompt)** — see the `notes` field in
  the manifest output; that's where the operational knowledge lives.
- **R10 (Thin Harness)** — adds 0 lines to `src/harness/`.
