# Composer Palette: Float + Glass Reskin

**Date:** 2026-06-17
**Scope:** Panel (`interfaces/webchat`) — chat composer slash-command palette and @-mention palette.

## Problem

Typing `/` in the chat composer opens the slash-command popup. Two defects:

1. **Not a true floating overlay — it squeezes the chat.** The popup renders
   *in document flow* and pushes the chat content upward (e.g. the empty-state
   text "我们从哪里开始？" jumps up when the popup appears).
2. **Missed the glass refactor.** After the panel-wide glass material refactor,
   this popup still uses the pre-glass styling (`bg-surface-raised shadow-lg`),
   looking plain next to every other popover/menu.

The `@`-mention palette (`MentionPaletteView`, shown in team mode) shares the
*identical* defect and styling. Both are fixed.

## Root Cause

`SlashPaletteView` and `MentionPaletteView` are rendered as **in-flow** children
of the composer stack (`composer/mod.rs`, the `max-w-3xl mx-auto` div tracked by
`stack_ref`). A `ResizeObserver` (`mod.rs:91`) maps that stack's `content_rect`
height into the `--composer-clearance` CSS variable, which pads the chat scroll
area so content clears the floating composer bar.

When a palette opens in flow, the stack grows taller → `--composer-clearance`
grows → the chat scroll content's bottom padding grows → visible content is
pushed up. That is the squeeze.

## Approach

Match the established in-repo glass-popover pattern already used by
`nav_menu.rs:127` (its upward-opening menu) and `model_picker.rs:129`. Two
mechanisms:

1. **Take the palettes out of flow.** Absolutely-positioned children do not
   contribute to a parent's `content_rect`, so an `absolute` palette no longer
   grows `stack_ref` → `--composer-clearance` stays fixed → nothing moves.
   Anchor each palette `bottom-full` so it floats upward, overlaying the chat
   messages above (outer composer overlay is `z-10`; palette is `z-50`).

2. **Reskin to glass.** Replace `rounded-2xl border border-border
   bg-surface-raised shadow-lg` with the glass recipe. The `.glass` class
   supplies backdrop blur + specular bezel (`::before`) + grain (`::after`);
   `.glass > *` already lifts rows above those overlays.

## Changes (3 edits, 2 files — markup/CSS only, zero logic)

### `interfaces/webchat/src/views/chat/composer/mod.rs`

Wrap the lower input cluster — injection-guard banner + project/model row +
composer card — in a single `<div class="relative">`. This becomes the
positioning anchor. Move both palette components (`SlashPaletteView` and the
team-mode `MentionPaletteView` `<Show>`) to the top of that wrapper.

`AttachmentPreviewBar` and `QueuedPromptBar` stay in flow above the wrapper —
they are persistent UI and *should* keep contributing to clearance.

### `interfaces/webchat/src/views/chat/composer/palette.rs` (`SlashPaletteView`)
### `interfaces/webchat/src/views/chat/mention_palette.rs` (`MentionPaletteView`)

Change only each root `<div>`'s class to:

```
glass animate-pop-in absolute bottom-full inset-x-0 mb-2 z-50
rounded-xl border border-border bg-surface-overlay/85 shadow-xl
max-h-[200px] overflow-y-auto
```

(`rounded-2xl` → `rounded-xl` to match the other glass menus.) Inner rows,
breadcrumb header, selection highlight, and the `For` loop are unchanged.

## Why this placement

Anchoring `bottom-full` to the cluster wrapper makes the palette float just
above the project/model row — preserving its original vertical position — while
the input box and pickers stay fixed. The two palettes are mutually exclusive at
runtime, so sharing the same `absolute bottom-full` anchor is safe.

## Edge Cases

- With an attachment/queue bar present, the floating palette overlays it on top
  (`z-50`) — acceptable and rare during `/` or `@` entry.
- `mb-2` on the absolute element gives the same gap above the cluster that
  `nav_menu` uses.

## Testing

Logic is untouched, so the existing `build_palette_entries` unit tests stay
green; no new unit tests (CSS is not unit-testable headless). Verification is
manual E2E in the deployed `.app`:

1. Open an empty chat, type `/` → palette floats with glass material; the
   empty-state text "我们从哪里开始？" does **not** move.
2. In team mode, type `@` → mention palette floats with the same treatment.
