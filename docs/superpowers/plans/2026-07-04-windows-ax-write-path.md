# Windows AX Write Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Windows UI Automation semantic write path (`set_value` / `perform_action`) in `aleph-desktop-windows` so the `desktop` tool's `set_value` / `ax_action` verbs work on Windows with read-back verification, matching macOS.

**Architecture:** All new code lands in one file, `desktop/windows/src/ax.rs`, behind the existing `AccessibilityCapability` trait. Pure decision logic (locator ranking, AX-action→UIA-pattern mapping) is extracted to module-level host-testable functions; COM/UIA glue lives in the `cfg(windows) mod imp` and is compile-verified. The protocol layer, `native.rs` dispatch, and trait defaults are already complete and are NOT touched (except three documentation-string corrections).

**Tech Stack:** Rust, windows-rs 0.58 (`Win32_UI_Accessibility` COM UI Automation), async-trait, tokio `spawn_blocking`.

## Global Constraints

- **R1 (brain-limb separation):** No platform API calls in `src/`. All Win32/UIA work stays in the `aleph-desktop-windows` crate. The only `src/` edits allowed here are documentation strings/comments (Task 6) — never API calls.
- **R10 (thin harness):** Zero additions to `src/harness/`. Zero new files anywhere — all Windows work is edits to the existing `desktop/windows/src/ax.rs`.
- **Cargo discipline:** Keep ALL verification scoped to the small `aleph-desktop-windows` leaf crate (`cargo test -p aleph-desktop-windows` / `cargo check -p aleph-desktop-windows`). NEVER build `alephcore` or run the workspace-wide test suite (`alephcore` compilation is memory-heavy → OOM risk). The windows leaf crate is small; scoped runs are cheap.
- **Verification split:** Pure functions get real host-runnable unit tests (TDD). Live COM/UIA `set_value`/`perform_action` are NOT unit-testable (need a running desktop app); they are compile-verified via `cargo check` and functionally gated by the manual E2E checklist at the end (spec §8.2). This follows the Rust testing rule "exclude FFI bindings from coverage."
- **Windows crate feature:** `Win32_UI_Accessibility` is already enabled in `desktop/windows/Cargo.toml:39` — it provides all UIA pattern interfaces and `UIA_*PatternId` constants. No Cargo.toml change required.
- **Err-vs-Ok contract (spec §4.2/§6):** `native.rs::ax_action_output` renders ANY `verification.state=="unverified"` as the fixed text *"Value written but read-back did not match (…)"*. Therefore return `Ok(AxActionResult)` ONLY when the action actually happened. Route "couldn't locate / not settable / unsupported action" through `Err(...)` (which `native.rs` wraps with `recovery::with_hint`). Reserve `Ok(...unverified, reason:"value_mismatch")` strictly for "wrote, but read-back differed."

---

## File Structure

- **Modify:** `desktop/windows/src/ax.rs` — the only functional change site. Add (a) module-level pure `RankCandidate` + `rank_candidates`, (b) module-level pure `AxPattern` + `ax_action_to_patterns`, (c) `imp` (cfg-windows) `value_of` / `resolve` / `collect_candidates` / `set_value` / `perform_action` / `try_pattern`, (d) enriched `query_focused`, (e) two trait-method overrides on `WindowsAccessibility`, (f) two non-windows stub functions, (g) unit tests.
- **Modify (docs only):** `src/builtin_tools/desktop/mod.rs` (DESCRIPTION line for `ax_action`), `src/builtin_tools/desktop/types.rs` (the `action` field doc-comment), `desktop/shared/src/traits/ax.rs` (module + trait doc comments).

Current top-level imports in `ax.rs` (line 35):
```rust
use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryByRoleParams, QueryTreeParams};
```
Several tasks extend this line; each task states the exact replacement.

---

## Task 1: Pure locator ranking

**Files:**
- Modify: `desktop/windows/src/ax.rs` (add module-level `RankCandidate` + `rank_candidates` after the `role_map` module, ~line 125; extend top import line 35)
- Test: `desktop/windows/src/ax.rs` `#[cfg(test)]` module (bottom of file)

**Interfaces:**
- Consumes: `aleph_protocol::...::ax::AxLocator` (`{ pid: Option<i32>, role: Option<String>, title: Option<String>, center: Option<[f64;2]> }`).
- Produces: `struct RankCandidate { role: String, title: Option<String>, center: (f64, f64) }` (derives `Clone`, `Debug`) and `fn rank_candidates(cands: &[RankCandidate], loc: &AxLocator) -> Option<usize>` — used by `imp::resolve` in Task 4/5.

- [ ] **Step 1: Extend the top-level import (line 35)** — add only what this task uses (`AxLocator`). Later tasks extend this line as they add usage, so no commit ever carries an unused import (safe even under `-D warnings`).

Replace:
```rust
use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryByRoleParams, QueryTreeParams};
```
with:
```rust
use aleph_protocol::desktop_bridge::methods::ax::{
    AxElement, AxLocator, QueryByRoleParams, QueryTreeParams,
};
```

- [ ] **Step 2: Write the failing tests** (append inside the existing `#[cfg(test)] mod tests`)

```rust
    fn cand(role: &str, title: Option<&str>, cx: f64, cy: f64) -> RankCandidate {
        RankCandidate { role: role.into(), title: title.map(Into::into), center: (cx, cy) }
    }
    fn loc(role: Option<&str>, title: Option<&str>, center: Option<[f64; 2]>) -> AxLocator {
        AxLocator { pid: None, role: role.map(Into::into), title: title.map(Into::into), center }
    }

    #[test]
    fn role_filter_excludes_non_matching() {
        let cands = [cand("AXButton", Some("OK"), 0.0, 0.0), cand("AXTextField", Some("OK"), 0.0, 0.0)];
        // Only the AXTextField candidate is eligible.
        assert_eq!(rank_candidates(&cands, &loc(Some("AXTextField"), None, None)), Some(1));
    }

    #[test]
    fn no_match_returns_none() {
        let cands = [cand("AXButton", None, 0.0, 0.0)];
        assert_eq!(rank_candidates(&cands, &loc(Some("AXTextField"), None, None)), None);
    }

    #[test]
    fn exact_title_beats_contains_case_insensitive() {
        let cands = [cand("AXTextField", Some("Email address"), 0.0, 0.0), cand("AXTextField", Some("email"), 0.0, 0.0)];
        // "email" is an exact (case-insensitive) match; "Email address" only contains it.
        assert_eq!(rank_candidates(&cands, &loc(Some("AXTextField"), Some("Email"), None)), Some(1));
    }

    #[test]
    fn center_breaks_ties_when_titles_equal_rank() {
        let cands = [cand("AXButton", None, 100.0, 100.0), cand("AXButton", None, 10.0, 10.0)];
        // No title given → both rank 0; nearest center to (0,0) wins.
        assert_eq!(rank_candidates(&cands, &loc(Some("AXButton"), None, Some([0.0, 0.0]))), Some(1));
    }

    #[test]
    fn no_role_filter_considers_all() {
        let cands = [cand("AXButton", Some("Save"), 0.0, 0.0), cand("AXMenuItem", Some("Save"), 0.0, 0.0)];
        // role=None → first exact-title match wins.
        assert_eq!(rank_candidates(&cands, &loc(None, Some("Save"), None)), Some(0));
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p aleph-desktop-windows -- rank_candidates role_filter no_match exact_title center_breaks no_role_filter`
Expected: FAIL to compile — `cannot find function rank_candidates` / `cannot find type RankCandidate`.

- [ ] **Step 4: Implement `RankCandidate` + `rank_candidates`** (insert at module level, right after the closing `}` of `mod role_map` near line 125)

```rust
/// A flattened UIA element summary used purely for locator ranking. Holds only
/// the Send-safe scalar fields `rank_candidates` needs, so the ranking decision
/// is a pure function testable without COM.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct RankCandidate {
    /// Mapped `"AX*"` role string (via `control_type_to_ax_role`).
    pub role: String,
    /// Element name/title, if any.
    pub title: Option<String>,
    /// Bounding-rect center in physical screen pixels.
    pub center: (f64, f64),
}

/// Pick the best candidate for an [`AxLocator`], mirroring the macOS Swift
/// locator: role is a hard filter; title ranks exact (0) < contains (1) <
/// no-match (2), case-insensitive; `center` euclidean distance breaks ties.
/// Returns `None` when the role filter leaves no candidate.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn rank_candidates(cands: &[RankCandidate], loc: &AxLocator) -> Option<usize> {
    let mut best: Option<(usize, (u8, f64))> = None;
    for (i, c) in cands.iter().enumerate() {
        if let Some(role) = &loc.role {
            if &c.role != role {
                continue;
            }
        }
        let title_rank = match (&loc.title, &c.title) {
            (Some(want), Some(have)) => {
                let (want, have) = (want.to_lowercase(), have.to_lowercase());
                if have == want {
                    0
                } else if have.contains(&want) {
                    1
                } else {
                    2
                }
            }
            (Some(_), None) => 2,
            (None, _) => 0,
        };
        let dist = match loc.center {
            Some([x, y]) => {
                let (dx, dy) = (c.center.0 - x, c.center.1 - y);
                (dx * dx + dy * dy).sqrt()
            }
            None => 0.0,
        };
        let key = (title_rank, dist);
        if best.as_ref().is_none_or(|(_, bk)| key < *bk) {
            best = Some((i, key));
        }
    }
    best.map(|(i, _)| i)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p aleph-desktop-windows -- rank_candidates role_filter no_match exact_title center_breaks no_role_filter`
Expected: PASS (5 tests). (This scoped run compiles the leaf crate only — never `alephcore`.)

- [ ] **Step 6: Commit**

```bash
git add desktop/windows/src/ax.rs
git commit -m "desktop(windows): add pure AX locator ranking"
```

---

## Task 2: Pure AX-action → UIA-pattern mapping

**Files:**
- Modify: `desktop/windows/src/ax.rs` (add module-level `AxPattern` + `ax_action_to_patterns` after `rank_candidates`)
- Test: `desktop/windows/src/ax.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: `aleph_desktop::DesktopError` (already imported at line 34).
- Produces: `enum AxPattern { Invoke, Toggle, SelectionItem, ExpandCollapse, Legacy }` (derives `Clone, Copy, Debug, PartialEq, Eq`) and `fn ax_action_to_patterns(action: &str) -> Result<Vec<AxPattern>>` — used by `imp::perform_action` (Task 5).

- [ ] **Step 1: Write the failing tests** (append inside `#[cfg(test)] mod tests`)

```rust
    #[test]
    fn axpress_maps_to_invoke_fallback_chain() {
        assert_eq!(
            ax_action_to_patterns("AXPress").unwrap(),
            vec![AxPattern::Invoke, AxPattern::Toggle, AxPattern::SelectionItem, AxPattern::Legacy]
        );
    }

    #[test]
    fn axconfirm_same_as_axpress() {
        assert_eq!(ax_action_to_patterns("AXConfirm").unwrap(), ax_action_to_patterns("AXPress").unwrap());
    }

    #[test]
    fn axshowmenu_maps_to_expand_collapse() {
        assert_eq!(ax_action_to_patterns("AXShowMenu").unwrap(), vec![AxPattern::ExpandCollapse]);
    }

    #[test]
    fn unknown_action_is_not_implemented() {
        let err = ax_action_to_patterns("AXFoo").unwrap_err();
        assert!(matches!(err, aleph_desktop::DesktopError::NotImplemented(_)));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aleph-desktop-windows -- axpress axconfirm axshowmenu unknown_action`
Expected: FAIL to compile — `cannot find function ax_action_to_patterns` / `cannot find type AxPattern`.

- [ ] **Step 3: Implement `AxPattern` + `ax_action_to_patterns`** (insert at module level after `rank_candidates`)

```rust
/// UIA control patterns the AX write path can invoke, in fallback order.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxPattern {
    Invoke,
    Toggle,
    SelectionItem,
    ExpandCollapse,
    Legacy,
}

/// Map a macOS-style AX action name onto an ordered UIA-pattern fallback chain.
/// Covers the actions the tool DESCRIPTION advertises; everything else is an
/// honest `NotImplemented` the model can read and recover from.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn ax_action_to_patterns(action: &str) -> Result<Vec<AxPattern>> {
    match action {
        "AXPress" | "AXConfirm" => Ok(vec![
            AxPattern::Invoke,
            AxPattern::Toggle,
            AxPattern::SelectionItem,
            AxPattern::Legacy,
        ]),
        "AXShowMenu" => Ok(vec![AxPattern::ExpandCollapse]),
        other => Err(DesktopError::NotImplemented(format!("ax.perform_action:{other}"))),
    }
}
```
(`DesktopError` and `Result` are already imported at line 34: `use aleph_desktop::{DesktopError, Result};`.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aleph-desktop-windows -- axpress axconfirm axshowmenu unknown_action`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add desktop/windows/src/ax.rs
git commit -m "desktop(windows): add AX-action to UIA-pattern mapping"
```

---

## Task 3: Element value read-back (A3) + focused-element enrichment

**Files:**
- Modify: `desktop/windows/src/ax.rs` — add `value_of` in `mod imp` (cfg-windows); enrich `imp::query_focused`; extend `imp`'s `use` block.

**Interfaces:**
- Produces: `fn value_of(el: &IUIAutomationElement) -> Option<String>` (in `imp`) — used by `imp::resolve` (Task 4) and `query_focused`.

**Verification note:** This task adds live-COM code with no unit test (per Global Constraints). It is compile-verified in Task 5's consolidated `cargo check`; functional behavior is checked by manual E2E item #5 (spec §8.2). Do not add a live-COM `#[test]`.

- [ ] **Step 1: Extend the `imp` `use` block** (inside `#[cfg(windows)] mod imp`, the `use windows::...::Accessibility::{...}` list near line 207)

Replace:
```rust
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    };
```
with:
```rust
    use windows::core::BSTR;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationLegacyIAccessiblePattern,
        IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_LegacyIAccessiblePatternId,
        UIA_ValuePatternId,
    };
```

- [ ] **Step 2: Add `value_of`** (inside `mod imp`, right after the `node_of` function near line 338)

```rust
    /// Read an element's textual value: `ValuePattern.CurrentValue`, falling back
    /// to `LegacyIAccessible.CurrentValue`. Empty strings normalize to `None`.
    /// Called on-demand for the located/focused element only — never for every
    /// node of a full tree walk (one COM call per node would slow snapshots).
    pub(super) fn value_of(el: &IUIAutomationElement) -> Option<String> {
        // SAFETY: read-only UIA pattern getters; missing pattern surfaces as Err.
        unsafe {
            if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if let Ok(v) = vp.CurrentValue() {
                    let s = v.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            if let Ok(lp) = el
                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                    UIA_LegacyIAccessiblePatternId,
                )
            {
                if let Ok(v) = lp.CurrentValue() {
                    let s = v.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
        None
    }
```

- [ ] **Step 3: Enrich `query_focused`** (replace the existing `imp::query_focused`, near line 370)

Replace:
```rust
    pub(super) fn query_focused() -> Result<Option<AxElement>> {
        let _com = ComGuard::new();
        let uia = automation()?;
        // SAFETY: documented UIA call; a missing focus surfaces as `Err`.
        unsafe { uia.GetFocusedElement() }.map_or(Ok(None), |el| Ok(Some(node_of(&el))))
    }
```
with:
```rust
    pub(super) fn query_focused() -> Result<Option<AxElement>> {
        let _com = ComGuard::new();
        let uia = automation()?;
        // SAFETY: documented UIA call; a missing focus surfaces as `Err`.
        unsafe { uia.GetFocusedElement() }.map_or(Ok(None), |el| {
            let mut node = node_of(&el);
            node.value = value_of(&el);
            Ok(Some(node))
        })
    }
```

- [ ] **Step 4: Commit** (compile is validated in Task 5's consolidated check)

```bash
git add desktop/windows/src/ax.rs
git commit -m "desktop(windows): read element value via ValuePattern (A3)"
```

---

## Task 4: `set_value` via UI Automation ValuePattern

**Files:**
- Modify: `desktop/windows/src/ax.rs` — add `imp::collect_candidates`, `imp::resolve`, `imp::set_value`; add the `set_value` trait override on `WindowsAccessibility`; add the non-windows stub `set_value`.

**Interfaces:**
- Consumes: `rank_candidates`/`RankCandidate` (Task 1), `value_of` (Task 3), `SetValueParams`/`AxLocator`/`AxActionResult`/`AxVerification`.
- Produces: `fn resolve(uia: &IUIAutomation, loc: &AxLocator) -> Result<Option<(IUIAutomationElement, AxElement)>>` (in `imp`) — reused by Task 5; `imp::set_value(loc, value) -> Result<AxActionResult>`.

**Verification note:** Live COM; no unit test. Compile-verified in Task 5; functionally gated by manual E2E items #1, #3, #4.

- [ ] **Step 1: Extend imports (top-level + `imp`)**

First, the top-level import (line ~35) — add `AxActionResult` + `SetValueParams` (used by the trait override in Step 4):
```rust
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, AxLocator, QueryByRoleParams, QueryTreeParams, SetValueParams,
};
```

Then, inside `#[cfg(windows)] mod imp` (near line 199), replace:
```rust
    use aleph_protocol::desktop_bridge::methods::ax::AxElement;
```
with:
```rust
    use aleph_protocol::desktop_bridge::methods::ax::{
        AxActionResult, AxElement, AxLocator, AxVerification,
    };
    use super::{rank_candidates, RankCandidate};
```

- [ ] **Step 2: Add a resolve-depth constant** (module level, near the existing `ROLE_SCAN_DEPTH` at line 129)

```rust
/// Depth bound for locator resolution walks (`set_value` / `perform_action`).
#[cfg_attr(not(windows), allow(dead_code))]
const RESOLVE_DEPTH: u32 = 12;
```

- [ ] **Step 3: Add `collect_candidates` + `resolve` + `set_value`** (inside `mod imp`, after `value_of`)

```rust
    /// Flatten the control-view subtree into `(RankCandidate, element)` pairs,
    /// bounded by `depth_remaining` and the global `MAX_NODES` budget. Each
    /// element handle is cloned (refcount bump) so it outlives the walk and can
    /// be acted on once ranking picks it.
    fn collect_candidates(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        depth_remaining: u32,
        count: &mut usize,
        out: &mut Vec<(RankCandidate, IUIAutomationElement)>,
    ) {
        let node = node_of(el);
        let center = node
            .bounds
            .as_ref()
            .map_or((0.0, 0.0), |b| (b.x + b.width / 2.0, b.y + b.height / 2.0));
        out.push((
            RankCandidate { role: node.role, title: node.title, center },
            el.clone(),
        ));
        *count += 1;
        if depth_remaining == 0 || *count >= MAX_NODES {
            return;
        }
        // SAFETY: walker child/sibling traversal; windows-rs surfaces "no
        // element" as Err, terminating each loop naturally.
        let mut next = unsafe { walker.GetFirstChildElement(el) };
        while let Ok(child) = next {
            collect_candidates(walker, &child, depth_remaining - 1, count, out);
            if *count >= MAX_NODES {
                break;
            }
            next = unsafe { walker.GetNextSiblingElement(&child) };
        }
    }

    /// Resolve an [`AxLocator`] to the best-matching live element plus a
    /// value-enriched summary. Returns `None` when nothing matches the role
    /// filter (caller converts that to an `Err` with a recovery hint).
    pub(super) fn resolve(
        uia: &IUIAutomation,
        loc: &AxLocator,
    ) -> Result<Option<(IUIAutomationElement, AxElement)>> {
        let hwnd = resolve_root_hwnd(loc.pid)?;
        // SAFETY: `hwnd` is a validated visible/foreground window handle.
        let root = unsafe { uia.ElementFromHandle(hwnd) }
            .map_err(|e| DesktopError::PlatformError(format!("ElementFromHandle failed: {e}")))?;
        // SAFETY: standard "what a user sees" control-view walker.
        let walker = unsafe { uia.ControlViewWalker() }
            .map_err(|e| DesktopError::PlatformError(format!("ControlViewWalker failed: {e}")))?;
        let mut cands: Vec<(RankCandidate, IUIAutomationElement)> = Vec::new();
        let mut count = 0usize;
        collect_candidates(&walker, &root, RESOLVE_DEPTH, &mut count, &mut cands);

        let summaries: Vec<RankCandidate> = cands.iter().map(|(c, _)| c.clone()).collect();
        let Some(idx) = rank_candidates(&summaries, loc) else {
            return Ok(None);
        };
        let (cand, el) = &cands[idx];
        let summary = AxElement {
            role: cand.role.clone(),
            title: cand.title.clone(),
            value: value_of(el),
            bounds: unsafe { el.CurrentBoundingRectangle() }.ok().map(rect_to_region),
            pid: unsafe { el.CurrentProcessId() }.unwrap_or(0),
            children: Vec::new(),
        };
        Ok(Some((el.clone(), summary)))
    }

    /// Write `value` into the located element's UIA ValuePattern and read it
    /// back for verification. Only returns `Ok` when a write actually occurred;
    /// "not located / not settable" is an `Err` (see the Err-vs-Ok contract).
    pub(super) fn set_value(loc: AxLocator, value: String) -> Result<AxActionResult> {
        let _com = ComGuard::new();
        let uia = automation()?;
        let (el, mut summary) = resolve(&uia, &loc)?.ok_or_else(|| {
            DesktopError::NotAvailable("no element matched role/title; try `ax_snapshot`".into())
        })?;
        // SAFETY: pattern getter; unsupported pattern surfaces as Err.
        let vp = unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
            .map_err(|_| {
                DesktopError::NotAvailable(
                    "element does not support a settable value; fall back to click + type_text"
                        .into(),
                )
            })?;
        // SAFETY: read-only property getter.
        if unsafe { vp.CurrentIsReadOnly() }.map(|b| b.as_bool()).unwrap_or(false) {
            return Err(DesktopError::NotAvailable(
                "element is read-only; fall back to click + type_text".into(),
            ));
        }
        // SAFETY: documented ValuePattern write.
        unsafe { vp.SetValue(&BSTR::from(value.as_str())) }
            .map_err(|e| DesktopError::PlatformError(format!("ValuePattern.SetValue failed: {e}")))?;
        // SAFETY: read-back for verification.
        let readback = unsafe { vp.CurrentValue() }.map(|b| b.to_string()).unwrap_or_default();
        let verification = if readback == value {
            AxVerification { state: "verified".into(), reason: None, actual_preview: None }
        } else {
            AxVerification {
                state: "unverified".into(),
                reason: Some("value_mismatch".into()),
                actual_preview: Some(readback.chars().take(200).collect()),
            }
        };
        summary.value = Some(readback.chars().take(200).collect());
        Ok(AxActionResult {
            performed: true,
            path: "accessibility".into(),
            matched: Some(summary),
            verification: Some(verification),
        })
    }
```

- [ ] **Step 4: Override the `set_value` trait method** (in the `#[async_trait] impl AccessibilityCapability for WindowsAccessibility` block, after `query_by_role`, near line 175)

```rust
    async fn set_value(
        &self,
        params: aleph_protocol::desktop_bridge::methods::ax::SetValueParams,
    ) -> Result<AxActionResult> {
        let AxLocatorAndValue { locator, value } =
            AxLocatorAndValue { locator: params.locator, value: params.value };
        run_blocking(move || imp::set_value(locator, value)).await
    }
```
Then add this tiny destructuring helper just above the `impl` block (avoids capturing `params` fields across the closure boundary awkwardly):
```rust
struct AxLocatorAndValue {
    locator: AxLocator,
    value: String,
}
```

> NOTE for the implementer: the helper struct is optional sugar. If you prefer, inline it:
> ```rust
> let (locator, value) = (params.locator, params.value);
> run_blocking(move || imp::set_value(locator, value)).await
> ```
> Pick ONE; do not include both. The inline form is simpler — prefer it and skip the helper struct.

- [ ] **Step 5: Add the non-windows stub** (in `#[cfg(not(windows))] mod imp`, after `query_by_role`, near line 451)

```rust
    pub(super) fn set_value(
        _loc: aleph_protocol::desktop_bridge::methods::ax::AxLocator,
        _value: String,
    ) -> Result<aleph_protocol::desktop_bridge::methods::ax::AxActionResult> {
        unavailable()
    }
```

- [ ] **Step 6: Commit**

```bash
git add desktop/windows/src/ax.rs
git commit -m "desktop(windows): implement set_value via UIA ValuePattern with read-back (B1/D1)"
```

---

## Task 5: `perform_action` via UIA pattern dispatch + consolidated compile check

**Files:**
- Modify: `desktop/windows/src/ax.rs` — add `imp::try_pattern`, `imp::perform_action`; add the `perform_action` trait override; add the non-windows stub `perform_action`; extend `imp` imports for the pattern interfaces.

**Interfaces:**
- Consumes: `resolve` (Task 4), `ax_action_to_patterns`/`AxPattern` (Task 2), `PerformActionParams`.
- Produces: `imp::perform_action(loc, action) -> Result<AxActionResult>`.

**Verification note:** Live COM; functionally gated by manual E2E item #2. This task ends with the ONE consolidated `cargo check` that compiles all COM code from Tasks 3–5.

- [ ] **Step 1: Extend imports (top-level + `imp`)**

First, the top-level import (line ~35) — add `PerformActionParams` (used by the trait override in Step 3):
```rust
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, AxLocator, PerformActionParams, QueryByRoleParams,
    QueryTreeParams, SetValueParams,
};
```

Then extend the `imp` action-pattern imports — replace the `use windows::Win32::UI::Accessibility::{...}` block (edited in Task 3) with:
```rust
    use windows::core::BSTR;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
        IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
        IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
        IUIAutomationValuePattern, UIA_ExpandCollapsePatternId, UIA_InvokePatternId,
        UIA_LegacyIAccessiblePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
        UIA_ValuePatternId,
    };
```
Also extend the `use super::{...}` line (added in Task 4) to bring in the pattern mapper:
```rust
    use super::{ax_action_to_patterns, rank_candidates, AxPattern, RankCandidate};
```

- [ ] **Step 2: Add `try_pattern` + `perform_action`** (inside `mod imp`, after `set_value`)

```rust
    /// Try to invoke one UIA pattern on `el`. `Ok(true)` = performed;
    /// `Ok(false)` = element does not expose this pattern (caller tries the
    /// next); `Err` = the pattern's action call itself failed.
    fn try_pattern(el: &IUIAutomationElement, pattern: AxPattern) -> Result<bool> {
        // SAFETY: each arm gets a pattern (Err if unsupported → Ok(false)) then
        // issues its documented action call.
        unsafe {
            match pattern {
                AxPattern::Invoke => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                        p.Invoke().map_err(|e| DesktopError::PlatformError(format!("Invoke: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::Toggle => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) {
                        p.Toggle().map_err(|e| DesktopError::PlatformError(format!("Toggle: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::SelectionItem => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
                        p.Select().map_err(|e| DesktopError::PlatformError(format!("Select: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::ExpandCollapse => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
                        p.Expand().map_err(|e| DesktopError::PlatformError(format!("Expand: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::Legacy => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId) {
                        p.DoDefaultAction().map_err(|e| DesktopError::PlatformError(format!("DoDefaultAction: {e}")))?;
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// Perform a macOS-style AX action on the located element by trying its
    /// UIA-pattern fallback chain. Unknown action → `NotImplemented` (from
    /// `ax_action_to_patterns`); no pattern usable → `NotAvailable`.
    pub(super) fn perform_action(loc: AxLocator, action: String) -> Result<AxActionResult> {
        let patterns = ax_action_to_patterns(&action)?;
        let _com = ComGuard::new();
        let uia = automation()?;
        let (el, summary) = resolve(&uia, &loc)?.ok_or_else(|| {
            DesktopError::NotAvailable("no element matched role/title; try `ax_snapshot`".into())
        })?;
        for pattern in patterns {
            if try_pattern(&el, pattern)? {
                return Ok(AxActionResult {
                    performed: true,
                    path: "accessibility".into(),
                    matched: Some(summary),
                    verification: None,
                });
            }
        }
        Err(DesktopError::NotAvailable(
            "element exposes no actionable pattern; try click at its center".into(),
        ))
    }
```

- [ ] **Step 3: Override the `perform_action` trait method** (in the `WindowsAccessibility` impl block, after the `set_value` override)

```rust
    async fn perform_action(
        &self,
        params: aleph_protocol::desktop_bridge::methods::ax::PerformActionParams,
    ) -> Result<AxActionResult> {
        let (locator, action) = (params.locator, params.action);
        run_blocking(move || imp::perform_action(locator, action)).await
    }
```

- [ ] **Step 4: Add the non-windows stub** (in `#[cfg(not(windows))] mod imp`, after the `set_value` stub)

```rust
    pub(super) fn perform_action(
        _loc: aleph_protocol::desktop_bridge::methods::ax::AxLocator,
        _action: String,
    ) -> Result<aleph_protocol::desktop_bridge::methods::ax::AxActionResult> {
        unavailable()
    }
```

- [ ] **Step 5: Consolidated compile + pure-test check (the single leaf-crate build)**

Run: `cargo test -p aleph-desktop-windows`
Expected: compiles cleanly (all Task 3–5 COM code) AND all 9 pure unit tests from Tasks 1–2 PASS. If the windows-rs generic method name differs in this toolchain (e.g. `GetCurrentPatternAs` vs a variant), the compiler names the exact fix — adjust the getter call, do not change the logic.

- [ ] **Step 6: Commit**

```bash
git add desktop/windows/src/ax.rs
git commit -m "desktop(windows): implement perform_action via UIA pattern dispatch (B1)"
```

---

## Task 6: Documentation corrections (A1)

**Files:**
- Modify: `src/builtin_tools/desktop/mod.rs:553` (ax_action DESCRIPTION line)
- Modify: `src/builtin_tools/desktop/types.rs:29-33` (the `action` field doc-comment)
- Modify: `desktop/shared/src/traits/ax.rs:3-5, 18-20` (module + trait doc comments)

**Verification note:** These are string-constant / comment edits only — they cannot change compilation semantics (the DESCRIPTION is a `r#"..."#` raw string; touching its interior is safe as long as the `"#` terminator is not introduced). Per the cargo discipline, do NOT build `alephcore` to "verify" a comment edit. Verify by re-reading the diff: the raw-string delimiters are intact and no stray `"#` was added.

- [ ] **Step 1: Correct the `ax_action` DESCRIPTION** (`src/builtin_tools/desktop/mod.rs`)

Replace:
```
- ax_action: Trigger a native accessibility action (ax_action_name, e.g. "AXPress", "AXShowMenu") on an element located the same way. More reliable than a synthetic click for buttons/menus when the app exposes AX actions. macOS only today; other platforms report the capability as unavailable.
```
with:
```
- ax_action: Trigger a native accessibility action (ax_action_name, e.g. "AXPress", "AXShowMenu") on an element located the same way. More reliable than a synthetic click for buttons/menus when the app exposes AX actions. Available on macOS (AX) and Windows (UI Automation: AXPress→Invoke/Toggle/Select, AXShowMenu→Expand); Linux reports the capability as unavailable — fall back to click.
```

- [ ] **Step 2: Correct the `action` field doc-comment** (`src/builtin_tools/desktop/types.rs`, lines 28-33)

Replace:
```rust
    /// The desktop operation to perform.
    ///
    /// Supported actions: "screenshot", "ocr", "click", "`double_click`", "drag",
    /// "hover", "`cursor_position`", "`mouse_button`", "`type_text`", "`key_combo`",
    /// "scroll", "`launch_app`", "`quit_app`", "`window_list`", "`focus_window`",
    /// "`clipboard_read`", "`clipboard_write`", "`screen_record`".
    pub action: String,
```
with:
```rust
    /// The desktop operation to perform. See the tool DESCRIPTION for the full
    /// per-action reference; the complete verb set is:
    ///
    /// Perception: "screenshot", "ocr", "screen_record", "wait_visual",
    /// "`display_list`". Pointer: "click", "`double_click`", "drag", "hover",
    /// "`cursor_position`", "`mouse_button`", "scroll". Keyboard/clipboard:
    /// "`type_text`", "`key_combo`", "`key_button`", "paste", "`clipboard_read`",
    /// "`clipboard_write`". Window/app: "`window_list`", "`focus_window`",
    /// "`move_window`", "`resize_window`", "`launch_app`", "`quit_app`",
    /// "`restart_app`". Semantic (macOS + Windows UIA): "`set_value`",
    /// "`ax_action`". Meta: "batch", "script".
    pub action: String,
```

- [ ] **Step 3: Correct the trait doc comments** (`desktop/shared/src/traits/ax.rs`)

Replace lines 2-5:
```rust
//! Platform implementations that support the macOS Accessibility API
//! implement this trait and return `Some(&self.ax)` from
//! [`crate::DesktopPlatform::ax`].
```
with:
```rust
//! Platform implementations that expose an accessibility tree (macOS via the
//! Accessibility API, Windows via UI Automation) implement this trait and
//! return `Some(&self.ax)` from [`crate::DesktopPlatform::ax`].
```
Then replace lines 16-20 (the doc block above the trait):
```rust
/// Query the OS accessibility (AX) element tree.
///
/// All methods are async because the underlying RPC call to the Swift
/// helper is I/O-bound.  On non-macOS platforms the `DesktopPlatform`
/// default returns `None` from `ax()`, so these methods are never called.
```
with:
```rust
/// Query the OS accessibility (AX) element tree.
///
/// All methods are async because the backing implementation is I/O-bound
/// (macOS marshals over the Swift-helper RPC; Windows runs UI Automation COM
/// on a blocking thread). Platforms without an accessibility tree (currently
/// Linux) return `None` from `ax()`, so these methods are never called there;
/// `set_value` / `perform_action` also keep a `NotImplemented` default so a
/// platform can offer read-only AX without a write path.
```

- [ ] **Step 4: Verify the diffs are comment/string-only and delimiter-safe**

Run: `git diff --stat`
Expected: three files changed, all hunks inside doc comments / the DESCRIPTION raw string. Manually confirm no `"#` was introduced inside `mod.rs`'s raw string.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/desktop/mod.rs src/builtin_tools/desktop/types.rs desktop/shared/src/traits/ax.rs
git commit -m "desktop: document Windows AX write path (A1 doc drift)"
```

---

## Manual E2E Checklist (spec §8.2 — run after Task 5 on this Windows machine)

Live UIA cannot be unit-tested; verify functionally with a running `aleph-server` on the local desktop:

1. **Notepad set_value:** open Notepad → `desktop{action:"set_value", role:"AXTextField", text:"hello 世界"}` → assert result `verification.state == "verified"` and the text is visible.
2. **Calculator AXPress:** open Calculator → `desktop_ax_snapshot` to locate a digit → `desktop{action:"ax_action", ax_action_name:"AXPress", element_title:"Five"}` → assert `performed:true` and the display updates.
3. **Browser address bar set_value:** focus the address bar → `set_value` a URL → assert `verified`.
4. **Not-settable path:** `set_value` targeting static text → assert an error message containing "fall back to click + type_text" (the `Err` path, not a false success).
5. **Observe value read-back (A3):** any focus-changing action with `observe:"state"` → assert `post_state.focused_element.value` is populated (non-null).

---

## Self-Review

**Spec coverage:**
- B1 (Windows set_value) → Task 4. ✓
- B1 (Windows perform_action) → Task 5. ✓
- Locator resolver (§4.1) → Task 1 (pure ranking) + Task 4 (`resolve` COM). ✓
- A3 (value read-back) → Task 3 (`value_of` + `query_focused`) + Task 4 (`resolve` summary). ✓
- D1 (verification) → Task 4 (`AxVerification` on read-back). ✓
- A1 (three doc corrections) → Task 6. ✓
- Testing strategy (§8) → pure unit tests (Tasks 1–2), single scoped build (Task 5), manual E2E checklist. ✓
- Out-of-scope items (§9: no click/type verification, no protocol/native.rs change, no A2) → honored; not touched. ✓

**Placeholder scan:** No TBD/TODO. Task 4 Step 4 offers two equivalent forms — the plan explicitly says "prefer the inline form, skip the helper struct" (a decision, not a placeholder). All code steps show complete code.

**Type consistency:** `RankCandidate` / `rank_candidates` (Task 1) consumed identically in Task 4. `AxPattern` / `ax_action_to_patterns` (Task 2) consumed identically in Task 5. `resolve` signature (Task 4) matches its call in Task 5. `AxActionResult` / `AxVerification` field names (`performed`, `path`, `matched`, `verification`, `state`, `reason`, `actual_preview`) match the protocol types verbatim. `value_of` signature (Task 3) matches calls in Tasks 3 & 4.
