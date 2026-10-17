# Stage 6 owner lifecycle fix report

Date: 2026-10-16
Frozen source baseline: `eae123546b0ff17d2bc550c58e702516fb6cb6e1`

## Scope

Production source changes are limited to:

- `interfaces/webchat/src/components/right_pane/slots/browser.rs`
- `interfaces/webchat/src/components/right_pane/pane.rs`
- `interfaces/webchat/src/platform/wide/views/browser/standalone.rs`

No protocol, Core, transport implementation, dependency, TUI, research document, or production `dist` file was intentionally changed. The existing working tree already contains out-of-scope generated `interfaces/webchat/dist/*` changes and the execution plan; they were left untouched.

## Mechanism

`InlineBrowser` previously stored the render `Owner` that also owned the `StandaloneBrowserViewer` transport. A terminal notice or explicit tab request called `cleanup()` on that owner while `Show` still held its `OwnedView`, disposing signals retained by the render branch. The fix adds an optional `on_transport_ready: Callback<Owner>` prop to `StandaloneBrowserViewer`. The component creates a child `Owner`, runs `transport::start` inside that child, and reports only that child to `InlineBrowser`. `stop_viewer` therefore stops the transport owner once; the render owner remains alive until `Show` removes the branch naturally. Existing notice ordering, terminal fallback, one-shot grant, explicit fresh-tab request, and transport/session semantics are unchanged.

`RegistryPane` now wraps only the slot render `SlotRenderCtx` snapshot construction in `untrack(snapshot)`. The outer `rendered_active` visibility and active registry Memos remain tracked, while activity changes in the full snapshot cannot remount a stable slot identity.

A native regression test now verifies that stopping the transport child twice invokes its cleanup exactly once, leaves the render owner alive, and lets render-owner cleanup happen later.

## Validation

- `rustfmt --edition 2021 --check interfaces/webchat/src/components/right_pane/slots/browser.rs interfaces/webchat/src/components/right_pane/pane.rs interfaces/webchat/src/platform/wide/views/browser/standalone.rs`: exit 0.
- `git diff --check`: exit 0.
- `python qa/browser_live/check_memory.py`: exit 0; `available_bytes=17009823744 required_bytes=4294967296 ALLOW` immediately before Cargo.
- `cargo test -p aleph-panel --lib right_pane::slots::browser`: exit 0; 5 passed, 0 failed, 1589 filtered out. This compiled the full `aleph-panel` library and exercised the browser owner/fallback/identity tests.

`cargo fmt --all -- --check` was also attempted and exited nonzero because of existing unrelated formatting failures outside this change; it did not modify files. Scoped rustfmt passed.

## Remaining validation and risk

The final native/factory build and compiled Stage 6 browser harness were deliberately not run here; the controller must independently run them against the final bundle. This report does not claim Stage 6 UI pass. The native owner test validates cleanup ordering and idempotence, but it cannot by itself prove the full WASM reactive mount/notice/socket lifecycle. The full compiled UI test must still verify Ready to Live identity, terminal fallback, fresh-tab issuance, zero page errors, and the corrected protocol fixture.
