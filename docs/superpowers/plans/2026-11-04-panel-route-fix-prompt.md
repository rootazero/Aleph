# Panel Route Fix — New Session Prompt (2026-11-04)

## 执行目标

修复 authenticated project-a route 上 Panel surface 未出现的问题。前次会话已打破 pairing 循环依赖，但 Panel routing 仍失败。

## 当前状态（来自前次会话 b2b15fc1）

### ✅ 已完成
- **Pairing UI 修复**：`interfaces/webchat/src/components/token_wall.rs` line 33 移除 `connected` 条件
- **验证通过**：
  - Node tests 95/95 PASS
  - Panel WASM 编译无 warning
  - Gateway/Reaper/Shell 构建成功
  - Provenance 生成，artifacts.ok=true
  - check_docs OK，git diff --check OK
- **Native run 结果**：
  - helper_ready: PASS
  - protected_pairing: PASS（21 次重试后成功）
  - protected_reload: PASS
  - global_discovery: PASS
  - **project_a: BLOCKED** (panel_route_unavailable)
  - cleanup: PASS

### ❌ 核心问题
- **症状**：project_a 路由后，Panel DOM markers `{panelShell:false, panelTabA:false, panelTabB:false, panelContent:false}` 全部为 false
- **已验证不是问题的部分**：
  - ✓ Gateway 成功返回 project-a snapshot（receipt seq 6 `result:"snapshot_ok"`）
  - ✓ Monitor 成功导航到 `/panel/project-a/overview`
  - ✓ Monitor selectors 正确（`data-panel-shell`、`data-panel-plugin` 等）
  - ✓ Path parsing 正确（`explicit_key` 返回 `{plugin_id:"project-a", tab_id:"overview"}`）
- **根本原因**：`interfaces/webchat/src/app.rs::explicit_community_panel()` 函数在五个步骤之一返回 None

## 任务分解

### Phase 1：诊断注入
**目标**：精确定位 `explicit_community_panel()` 失败步骤

**文件**：`interfaces/webchat/src/app.rs`

**修改内容**：在 `explicit_community_panel()` 的五个步骤添加诊断日志：

```rust
fn explicit_community_panel(path: &str, discovery: &PanelDiscoveryState) -> Option<(Projection, Key)> {
    // Step 1: Check discovery status
    match discovery.status() {
        PanelDiscoveryStatus::Ready => {
            leptos::logging::log!("🔍 [routing] Step 1 PASS: discovery.status() = Ready");
        }
        s => {
            leptos::logging::log!("🔍 [routing] Step 1 FAIL: discovery.status() = {:?}", s);
            return None;
        }
    }

    // Step 2: Parse path
    let key = match PanelProjection::explicit_key(path) {
        Some(k) => {
            leptos::logging::log!("🔍 [routing] Step 2 PASS: explicit_key({}) = {:?}", path, k);
            k
        }
        None => {
            leptos::logging::log!("🔍 [routing] Step 2 FAIL: explicit_key({}) returned None", path);
            return None;
        }
    };

    // Step 3: Get snapshot
    let snapshot = match discovery.snapshot() {
        Some(s) => {
            leptos::logging::log!("🔍 [routing] Step 3 PASS: snapshot exists, revision={}", s.revision);
            s.clone()
        }
        None => {
            leptos::logging::log!("🔍 [routing] Step 3 FAIL: snapshot() returned None");
            return None;
        }
    };

    // Step 4: From snapshot
    let projection = match PanelProjection::from_snapshot(snapshot.clone()) {
        Ok(p) => {
            leptos::logging::log!("🔍 [routing] Step 4 PASS: from_snapshot succeeded");
            p
        }
        Err(e) => {
            leptos::logging::log!("🔍 [routing] Step 4 FAIL: from_snapshot error: {:?}", e);
            return None;
        }
    };

    // Step 5: Select explicit
    match projection.select_explicit(&key, snapshot.revision) {
        Ok(descriptor) => {
            leptos::logging::log!("🔍 [routing] Step 5 PASS: select_explicit succeeded, descriptor.key={:?}", descriptor.key);
            Some((projection, descriptor.key))
        }
        Err(e) => {
            leptos::logging::log!("🔍 [routing] Step 5 FAIL: select_explicit error: {:?}", e);
            None
        }
    }
}
```

**验证点**：
- 编译无 warning
- 诊断日志通过 browser console 和 Monitor 捕获
- Preflight PASS

### Phase 2：原生测试
**目标**：收集诊断输出，确定失败步骤

**执行**：`node qa/native_ipc/native_project_runner.mjs --execute`

**收集数据**：
1. Browser console 中的 `🔍 [routing]` 日志
2. Panel DOM inspect 结果
3. 完整事件序列
4. Helper receipts

### Phase 3：根因分析与修复
**根据诊断输出选择修复方案**：

#### 场景 A：Step 1 FAIL（discovery.status() != Ready）
**原因**：路由时 discovery 尚未完成

**修复方案**：
```rust
// 在 MainContent 的路由 effect 中添加 watch
create_effect(move |_| {
    // 等待 discovery ready
    if matches!(state.panel_discovery.get().status(), PanelDiscoveryStatus::Ready) {
        // 现在才尝试 routing
        ...
    }
});
```

#### 场景 B：Step 3 FAIL（snapshot() == None）
**原因**：snapshot 未被存储到 Signal

**修复方案 B1**（立即等待 snapshot）：
```rust
// 在 select_panel_project() 后显式等待 snapshot
pub fn select_panel_project(&self, project_id: String) {
    // ... existing select logic ...
    
    // 显式等待 snapshot 更新
    self.panel_discovery.update(|d| {
        // 轮询直到 snapshot 存在或超时
        ...
    });
}
```

**修复方案 B2**（延迟路由触发）：
```rust
// 在 panel_navigation 的 effect 中添加 snapshot 检查
create_effect(move |_| {
    let discovery = state.panel_discovery.get();
    if discovery.snapshot().is_some() {
        // 现在才导航
        ...
    }
});
```

#### 场景 C：Step 4 FAIL（from_snapshot 错误）
**原因**：snapshot 格式/内容问题

**修复方案**：检查 snapshot schema 版本或字段缺失

#### 场景 D：Step 5 FAIL（select_explicit 错误）
**原因**：plugin_id/tab_id 不在 snapshot 中，或 visibility 检查失败

**修复方案**：确保 Gateway fixture 返回的 descriptor 与导航 key 匹配

### Phase 4：完整验证
**必须通过的验证**：
1. ✅ Node tests 95/95 PASS
2. ✅ check_docs OK
3. ✅ git diff --check OK
4. ✅ Panel WASM 编译无 warning
5. ✅ Preflight PASS
6. ✅ Native run 所有 6 个 phase PASS
7. ✅ Panel DOM markers 全部 true
8. ✅ Protected pairing/reload/discovery 仍 PASS

## 硬约束
- 诊断日志仅允许使用 `leptos::logging::log!`
- 不新增 UI、不修改 API 契约、不改 Gateway
- 不改动已验证正确的 selector、navigation、credential 流程
- 完整流程验证：6 phase PASS 包括 project_a routing success

## 验证里程碑
1. ✅ 诊断日志成功注入，编译无 warning
2. ✅ Preflight PASS，webviewLaunches=0
3. ✅ Native run 完成，收集诊断输出
4. ✅ 分析输出，确定失败步骤
5. ✅ 实施修复后，project_a routing 成功（Panel DOM markers 全部 true）
6. ✅ Protected_pairing/reload/discovery 仍 PASS
7. ✅ 所有测试验证通过

## 交付物
- 修改后的 `interfaces/webchat/src/app.rs`（带诊断日志）
- 新的 native run output JSON
- 诊断分析文档（明确指出失败步骤和原因）
- 最终修复代码（如需）
- 完整的验证报告

## 预期成果
**成功**：所有 6 个 phase PASS，包括 project_a routing success，Panel 界面完整出现

**失败**：精确定位问题根源，为后续迭代提供明确方向

---

**工作方式**：主 agent 直接执行，不使用子代理，不启动 CLI 模型。完成后输出完整结果。
