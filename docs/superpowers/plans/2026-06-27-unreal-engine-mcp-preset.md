# Unreal Engine MCP Preset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a one-click `unreal-engine` MCP preset that connects to Epic's editor-embedded MCP server, and give the preset model a `post_install` setup-guidance capability surfaced in the Panel's detail drawer.

**Architecture:** Pure-additive. `McpPreset` gains an optional `post_install` field (deserialized from `catalog.json`). A new pure helper `official_mcp::post_install_for(entry_id)` resolves guidance from the in-binary preset catalog by Hub entry id — deliberately avoiding a field on `ExtensionEntry` (12 struct-literal sites, high churn). The `extensions.disclosure` handler attaches `post_install`; the webchat detail drawer (which already lazy-loads disclosure) renders it. The fleeting install-success toast is intentionally NOT used.

**Tech Stack:** Rust (alephcore: serde, tokio, JSON-RPC), Leptos/WASM (interfaces/webchat).

## Global Constraints

- **Branch:** work only on `worktree-feat-unreal-engine-mcp-preset`. NEVER touch `main`.
- **NO cargo runs:** Per user mandate (system-load), do NOT run `cargo check`/`test`/`clippy`/`build`. Each "run test" step below is marked **DEFERRED** — write the test (regression value), implement, commit directly. Every task is authored so its commit leaves the crate compiling by construction.
- **Entropy:** strictly additive; introduce no dead code. `post_install` has a real consumer (the detail drawer) by the end of Task 5.
- **Commit format:** `<scope>: <description>`, English, e.g. `mcp: add post_install field to McpPreset`.
- **Exact UE entry values:** `id="unreal-engine"`, transport `http` `http://127.0.0.1:8000/mcp`, `required_env=[]`, `official=true`, `category="developer"`, `reachability="cn-native"` (localhost is always reachable; field is not projected to Hub).
- **`post_install` text (verbatim, used in Task 2):**
  ```
  连接的是 Epic 官方「Unreal MCP」插件，运行在 Unreal Editor 进程内。⚠️ 安装前请先在编辑器里把 server 跑起来：\n1. 用 Unreal Editor 5.8+ 打开你的项目。\n2. Edit → Plugins 搜索「Unreal MCP」，启用并重启编辑器（依赖的 Toolset Registry 会自动启用）。\n3. Edit → Editor Preferences → General → Model Context Protocol，打开「Auto Start Server」（或在编辑器控制台运行 ModelContextProtocol.StartServer），默认监听 http://127.0.0.1:8000/mcp。\n4. 确认编辑器内 server 已在监听后，再回到 Aleph 点安装/启用——这样才能探测到工具。\n注意：Epic 标记此功能为实验性；工具调用在引擎 game thread 串行执行，避免并发下发。若你改过端口/路径，请相应修改该 server 的 URL。
  ```
- **Spec:** `docs/superpowers/specs/2026-06-27-unreal-engine-mcp-preset-design.md` (read §3, §4.1 before starting).

---

### Task 1: Add `post_install` field to `McpPreset`

**Files:**
- Modify: `src/mcp/presets/mod.rs` (struct ~L14-38; tests ~L114-158)

**Interfaces:**
- Produces: `McpPreset.post_install: Option<String>` — consumed by Tasks 2 & 3.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/mcp/presets/mod.rs`:

```rust
    #[test]
    fn post_install_defaults_to_none_when_absent() {
        // Back-compat: a preset JSON without the post_install key still parses.
        let json = r#"{
            "id": "x", "name": "X", "category": "developer",
            "description": "d", "vendor": "V", "official": true,
            "reachability": "global",
            "transports": [{ "kind": "http", "url": "https://x/mcp" }]
        }"#;
        let p: McpPreset = serde_json::from_str(json).expect("parse");
        assert!(p.post_install.is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails** — **DEFERRED (no-cargo mandate).** Intended: `cargo test -p alephcore --lib presets::tests::post_install_defaults_to_none_when_absent` → FAIL (no field `post_install`).

- [ ] **Step 3: Write minimal implementation**

In `src/mcp/presets/mod.rs`, add the field to `McpPreset` immediately after the `tags` field (currently the last field before the closing brace):

```rust
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Post-install setup guidance shown to the user (Chinese, user-facing).
    /// For presets needing out-of-band setup (e.g. a local editor-embedded
    /// server). `None` = no extra steps. `serde(default)` keeps old catalog
    /// entries (without this key) parseable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_install: Option<String>,
}
```

(Replace the existing `tags` field + closing `}` of the struct with the block above.)

- [ ] **Step 4: Run test to verify it passes** — **DEFERRED.** Intended: same command → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/presets/mod.rs
git commit -m "mcp: add post_install field to McpPreset"
```

---

### Task 2: Add the `unreal-engine` catalog entry

**Files:**
- Modify: `src/mcp/presets/catalog.json` (append final array element)
- Modify: `src/mcp/presets/mod.rs` (tests)

**Interfaces:**
- Consumes: `McpPreset.post_install` (Task 1).
- Produces: preset id `"unreal-engine"` resolvable via `presets::find("unreal-engine")` — consumed by Task 3.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/mcp/presets/mod.rs`:

```rust
    #[test]
    fn unreal_engine_preset_is_local_http_with_guidance() {
        let ue = find("unreal-engine").expect("unreal-engine present");
        assert_eq!(ue.transports.len(), 1);
        let t = &ue.transports[0];
        assert_eq!(t.kind, McpTransportType::Http);
        assert_eq!(t.url.as_deref(), Some("http://127.0.0.1:8000/mcp"));
        assert!(ue.required_env.is_empty());
        assert!(ue.official);
        let pi = ue.post_install.as_deref().expect("post_install set");
        assert!(pi.contains("Unreal MCP"));
    }
```

- [ ] **Step 2: Run test to verify it fails** — **DEFERRED.** Intended: `cargo test -p alephcore --lib presets::tests::unreal_engine_preset_is_local_http_with_guidance` → FAIL (`find` returns None).

- [ ] **Step 3: Write minimal implementation**

In `src/mcp/presets/catalog.json`, the current final element is `t8star`. Change its closing `}` to `},` and insert the new object before the closing `]`. Replace:

```json
    "tags": ["image", "tts", "model-provider", "relay"]
  }
]
```

with:

```json
    "tags": ["image", "tts", "model-provider", "relay"]
  },
  {
    "id": "unreal-engine",
    "name": "虚幻引擎 (Unreal Engine)",
    "category": "developer",
    "description": "驱动正在运行的 Unreal Editor 5.8+：生成 Actor、配置光照、材质实例、运行自动化测试等。",
    "vendor": "Epic Games",
    "official": true,
    "reachability": "cn-native",
    "transports": [
      { "kind": "http", "url": "http://127.0.0.1:8000/mcp" }
    ],
    "required_env": [],
    "tags": ["game-engine", "unreal", "developer", "local"],
    "post_install": "连接的是 Epic 官方「Unreal MCP」插件，运行在 Unreal Editor 进程内。⚠️ 安装前请先在编辑器里把 server 跑起来：\n1. 用 Unreal Editor 5.8+ 打开你的项目。\n2. Edit → Plugins 搜索「Unreal MCP」，启用并重启编辑器（依赖的 Toolset Registry 会自动启用）。\n3. Edit → Editor Preferences → General → Model Context Protocol，打开「Auto Start Server」（或在编辑器控制台运行 ModelContextProtocol.StartServer），默认监听 http://127.0.0.1:8000/mcp。\n4. 确认编辑器内 server 已在监听后，再回到 Aleph 点安装/启用——这样才能探测到工具。\n注意：Epic 标记此功能为实验性；工具调用在引擎 game thread 串行执行，避免并发下发。若你改过端口/路径，请相应修改该 server 的 URL。"
  }
]
```

- [ ] **Step 4: Run test to verify it passes** — **DEFERRED.** Intended: same command → PASS. Also re-validate JSON parse: the existing `bundled_catalog_parses_and_has_first_batch` test covers `serde_json::from_str` of the whole file.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/presets/catalog.json src/mcp/presets/mod.rs
git commit -m "mcp: add unreal-engine preset (editor-embedded MCP server)"
```

---

### Task 3: Add `official_mcp::post_install_for` resolver

**Files:**
- Modify: `src/hub/official_mcp.rs` (new pub fn + tests)

**Interfaces:**
- Consumes: `presets::find` (existing), `ALEPH_HUB_ID` (already imported in this file), `McpPreset.post_install` (Task 1).
- Produces: `pub fn post_install_for(entry_id: &str) -> Option<&'static str>` — consumed by Task 4.

- [ ] **Step 1: Write the failing test**

Add inside the existing `#[cfg(test)] mod tests` in `src/hub/official_mcp.rs`. The first test guards the UE entry's projection (passes once Task 2's entry exists — it locks the install shape); the rest drive `post_install_for`:

```rust
    #[test]
    fn unreal_engine_projects_to_keyless_remote() {
        let e = primer_entries();
        let ue = by_id(&e, "aleph-hub:unreal-engine");
        assert_eq!(ue.kind, ExtensionKind::Mcp);
        assert_eq!(ue.trust_tier, TrustTier::Official);
        match ue.install_spec.unwrap() {
            InstallSpec::McpRemote { url, transport, .. } => {
                assert_eq!(url, "http://127.0.0.1:8000/mcp");
                assert!(matches!(transport, crate::hub::types::McpTransport::StreamableHttp));
            }
            other => panic!("expected McpRemote, got {other:?}"),
        }
        assert!(!ue.requires_config);
    }

    #[test]
    fn post_install_for_unreal_returns_guidance() {
        let g = super::post_install_for("aleph-hub:unreal-engine").expect("guidance");
        assert!(g.contains("Unreal MCP"));
    }

    #[test]
    fn post_install_for_preset_without_guidance_is_none() {
        assert!(super::post_install_for("aleph-hub:context7").is_none());
    }

    #[test]
    fn post_install_for_unprefixed_or_unknown_is_none() {
        assert!(super::post_install_for("unreal-engine").is_none()); // missing aleph-hub: prefix
        assert!(super::post_install_for("aleph-hub:nope").is_none()); // unknown slug
    }
```

- [ ] **Step 2: Run test to verify it fails** — **DEFERRED.** Intended: `cargo test -p alephcore --lib official_mcp::tests::post_install_for` → FAIL (fn not found).

- [ ] **Step 3: Write minimal implementation**

In `src/hub/official_mcp.rs`, add after `primer_entries()` (before the `#[cfg(test)]` module):

```rust
/// Setup guidance for a Hub entry, if its id maps to an in-binary MCP preset
/// that carries `post_install`. Keyed by the Hub entry id (`aleph-hub:<slug>`).
/// Returns `None` for non-preset ids, unknown slugs, or presets without
/// guidance.
pub fn post_install_for(entry_id: &str) -> Option<&'static str> {
    let slug = entry_id.strip_prefix(&format!("{ALEPH_HUB_ID}:"))?;
    presets::find(slug)?.post_install.as_deref()
}
```

- [ ] **Step 4: Run test to verify it passes** — **DEFERRED.** Intended: same command → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/hub/official_mcp.rs
git commit -m "hub: add post_install_for preset-guidance resolver"
```

---

### Task 4: Surface `post_install` in `extensions.disclosure`

**Files:**
- Modify: `src/gateway/handlers/extensions/install.rs` (`handle_disclosure`, ~L139-156)

**Interfaces:**
- Consumes: `official_mcp::post_install_for` (Task 3).
- Produces: `extensions.disclosure` response now carries a `post_install` sibling key (string or null) — consumed by Task 5.

> **Note (intentional, no new unit test):** `handle_disclosure` is async over `CatalogCache` and has no handler-level tests today (only pure helpers are tested in this file). This change is thin wiring over the already-tested `post_install_for` (Task 3). Verification is by inspection + Task 3 coverage. Do not add a heavy handler fixture.

- [ ] **Step 1: Modify the handler**

In `src/gateway/handlers/extensions/install.rs`, replace the body of `handle_disclosure` after `let disclosure = build_disclosure(&entry, &spec);`:

```rust
    let disclosure = build_disclosure(&entry, &spec);
    let post_install = crate::hub::official_mcp::post_install_for(&entry.id);
    JsonRpcResponse::success(
        req.id,
        json!({
            "disclosure": disclosure,
            "injection_findings": scan_text(&entry),
            "post_install": post_install,
        }),
    )
```

(`post_install` is `Option<&str>` → serializes to a string or `null`. No new `use` needed — the call is fully qualified.)

- [ ] **Step 2: Verify by inspection** — confirm the only changed function is `handle_disclosure`; `handle_install` is untouched (its success toast is fleeting; see spec §3.1).

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/extensions/install.rs
git commit -m "gateway: attach post_install to extensions.disclosure response"
```

---

### Task 5: Render `post_install` in the Panel detail drawer (Phase 2 — needs WASM rebuild)

> **Visibility note:** the Panel is embedded into `aleph-server` at compile time (`rust_embed`). These changes only become visible after `just wasm` + rebuilding the server (CLAUDE.md embed chain) — a manual step the user runs later, NOT in this session.

This task is atomic (parse-layer arity change + its single caller) so the `interfaces/webchat` crate compiles at the commit boundary.

**Files:**
- Modify: `interfaces/webchat/src/api/extensions.rs` (new pure `parse_disclosure_result` + `disclosure()` return arity + tests)
- Modify: `interfaces/webchat/src/components/extensions/detail_drawer.rs` (signal + caller + render)

**Interfaces:**
- Consumes: `extensions.disclosure` `post_install` sibling key (Task 4).
- Produces: `parse_disclosure_result(&Value) -> Result<(DisclosurePayload, Vec<InjectionFinding>, Option<String>), String>`; `ExtensionsApi::disclosure(...) -> Result<(DisclosurePayload, Vec<InjectionFinding>, Option<String>), String>`.

- [ ] **Step 1: Write the failing tests**

Add to `#[cfg(test)] mod tests` in `interfaces/webchat/src/api/extensions.rs`:

```rust
    #[test]
    fn parse_disclosure_picks_up_post_install_sibling() {
        let v = json!({
            "disclosure": { "tier": "official", "risk": "network", "one_line": "x" },
            "injection_findings": [],
            "post_install": "启动编辑器 server"
        });
        let (_d, _f, pi) = parse_disclosure_result(&v).unwrap();
        assert_eq!(pi.as_deref(), Some("启动编辑器 server"));
    }

    #[test]
    fn parse_disclosure_absent_post_install_is_none() {
        let v = json!({ "disclosure": { "tier": "official", "risk": "network", "one_line": "x" } });
        let (_d, _f, pi) = parse_disclosure_result(&v).unwrap();
        assert!(pi.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail** — **DEFERRED.** Intended: `cargo test -p aleph-webchat --lib api::extensions::tests::parse_disclosure` → FAIL (fn not found).

- [ ] **Step 3: Add the pure parser**

In `interfaces/webchat/src/api/extensions.rs`, add after `parse_install_result` (before `pub struct ExtensionsApi;`):

```rust
/// Pure: parse an `extensions.disclosure` response into
/// (disclosure, findings, optional post-install guidance). `post_install` is a
/// sibling key, `null`/absent when the entry has no setup guidance.
pub fn parse_disclosure_result(
    v: &Value,
) -> Result<(DisclosurePayload, Vec<InjectionFinding>, Option<String>), String> {
    let disclosure = serde_json::from_value(v.get("disclosure").cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("parse disclosure: {e}"))?;
    let findings =
        serde_json::from_value(v.get("injection_findings").cloned().unwrap_or(json!([])))
            .unwrap_or_default();
    let post_install = v.get("post_install").and_then(Value::as_str).map(str::to_owned);
    Ok((disclosure, findings, post_install))
}
```

- [ ] **Step 4: Rewire `disclosure()` to use it and widen its return**

Replace the existing `pub async fn disclosure(...)` method with:

```rust
    pub async fn disclosure(
        state: &DashboardState,
        id: String,
    ) -> Result<(DisclosurePayload, Vec<InjectionFinding>, Option<String>), String> {
        let r = state
            .rpc_call("extensions.disclosure", json!({ "id": id }))
            .await?;
        parse_disclosure_result(&r)
    }
```

- [ ] **Step 5: Run tests to verify they pass** — **DEFERRED.** Intended: same command → PASS.

- [ ] **Step 6: Update the only caller (detail drawer) — signal, destructure, render**

In `interfaces/webchat/src/components/extensions/detail_drawer.rs`:

(a) After `let disc_loading = RwSignal::new(false);` add:

```rust
    let post_install = RwSignal::new(Option::<String>::None);
```

(b) Replace the disclosure `Effect::new(...)` block with (adds `post_install` reset + 3-tuple destructure):

```rust
    // Lazy-load disclosure when an entry is selected.
    Effect::new(move || {
        if let Some(entry) = store.selected.get() {
            disclosure.set(None);
            post_install.set(None);
            disc_loading.set(true);
            let id = entry.id.clone();
            spawn_local(async move {
                match ExtensionsApi::disclosure(&state, id).await {
                    Ok((d, _findings, pi)) => {
                        disclosure.set(Some(d));
                        post_install.set(pi);
                        disc_loading.set(false);
                    }
                    Err(_) => {
                        disc_loading.set(false);
                    }
                }
            });
        }
    });
```

(c) Render the guidance box inside the scrollable column, immediately after the closing `</div>` of the "what it can reach" block (the `<div>` that ends the disclosure section, before the scrollable container's closing `</div>`). Insert:

```rust
                                // setup guidance (post_install) — persistent, pre/post install
                                {move || post_install.get().map(|pi| view! {
                                    <div class="p-2 rounded border border-border text-xs text-text-secondary whitespace-pre-line">
                                        "⚙️ "{pi}
                                    </div>
                                })}
```

(`whitespace-pre-line` preserves the `\n` line breaks; the `⚙️` prefix avoids introducing a new i18n key, per spec §3.1.)

- [ ] **Step 7: Verify by inspection** — `disclosure()`'s only caller is this drawer (grep `ExtensionsApi::disclosure`); the 3-tuple destructure is updated, so `aleph-webchat` compiles.

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/api/extensions.rs interfaces/webchat/src/components/extensions/detail_drawer.rs
git commit -m "webchat: render preset post_install guidance in detail drawer"
```

---

## Post-implementation (manual, user-run — NOT in the no-cargo session)

To make the Panel change visible: `just wasm` then rebuild/replace the running `aleph-server` binary (CLAUDE.md embed chain). To smoke-test end-to-end: start an Unreal Editor 5.8+ with the Unreal MCP plugin + Auto Start Server, then in the Panel open Aleph Hub → "虚幻引擎" → the detail drawer shows the ⚙️ setup steps; Install connects to `http://127.0.0.1:8000/mcp`.

## Self-Review

- **Spec coverage:** G1 (UE entry) → Task 2. G2 (`post_install` model + surface) → Tasks 1, 3, 4, 5. Non-goals N1/N2/N3 → not implemented (correct). §4.1 eager-connect → reflected in the verbatim `post_install` text (editor-first ordering) in Task 2.
- **Placeholder scan:** none — all code blocks are concrete; deferred run-steps are intentional per the global no-cargo constraint.
- **Type consistency:** `post_install` is `Option<String>` (preset / parsed) and `Option<&'static str>` (resolver/handler, serializes identically). `disclosure()` and `parse_disclosure_result` both return the same 3-tuple. `post_install_for` id key matches the `format!("{ALEPH_HUB_ID}:{}", p.id)` projection in `map_entry`.
