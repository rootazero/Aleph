# Panel Route Diagnostic & Fix Plan (2026-11-04)

## 背景（来自前次会话）

前次调查（2026-11-03）识别了两个独立的问题：

### ✅ 已修复：Pairing UI 循环依赖
- **修复**：`interfaces/webchat/src/components/token_wall.rs` line 33，移除 `connected` 条件
- **效果**：打破了"需要连接→需要 credential→需要 pairing UI→需要连接"的循环
- **验证**：protected_pairing 阶段最终 PASS（虽然前 21 次 inspect 仍报 unavailable）

### ❌ 未解决：Panel 路由不可用
- **症状**：project_a 路由后，Panel DOM markers `{panelShell:false, panelTabA:false, panelTabB:false, panelContent:false}` 全部为 false
- **已验证**：
  - ✓ Gateway 成功返回 project-a snapshot（receipt seq 6 `result:"snapshot_ok"`）
  - ✓ Monitor 成功导航到 `/panel/project-a/overview` 路由
  - ✓ Monitor selectors 正确指向 `data-panel-shell`、`data-panel-plugin` 等 attributes
  - ✗ Panel 组件不渲染
- **根本原因**：`explicit_community_panel()` 函数在五个步骤之一返回 None

## 任务目标

在新会话中：

1. **添加诊断日志**到 `explicit_community_panel()` 的五个步骤，精确定位失败点
2. **重建 Panel/Shell**，运行第二次 native test
3. **分析诊断输出**，识别具体的失败原因
4. **实施最小修复**，解决 routing 问题
5. **验证完整流程**：pairing + reload + discovery + routing 全部 PASS

## 实施步骤

### Phase 1：诊断代码注入

**文件**：`interfaces/webchat/src/app.rs` 主 content routing 函数

**任务**：在 `explicit_community_panel()` 中添加诊断日志：

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

**检查点**：确保诊断日志通过 `leptos::logging::log!` 输出到浏览器 console 和 Monitor 日志捕获。

### Phase 2：构建与验证

1. **编译 Panel WASM**：`cargo build --locked -p aleph-panel --lib --no-default-features --features panel-modular --profile wasm-release --target wasm32-unknown-unknown`
2. **运行 wasm-bindgen + wasm-opt**
3. **构建 Gateway/Reaper/Shell**
4. **生成 provenance**，验证 preflight PASS
5. **运行完整测试套件**：Node tests、check_docs、git diff --check

### Phase 3：原生测试执行

执行 `node qa/native_ipc/native_project_runner.mjs --execute`，收集：

1. **诊断输出**：browser console 中的 `🔍 [routing]` 日志
2. **Panel DOM inspect**：`{panelShell, panelTabA, panelTabB, panelContent}` 值
3. **完整事件序列**：每个 phase 的 PASS/BLOCKED 状态
4. **Helper receipts**：Gateway 响应的 select/discovery 操作

### Phase 4：根本原因分析

根据诊断输出，判断失败点：

**如果 Step 1 FAIL**（discovery.status() != Ready）：
- 时序问题：导航时间早于 discovery 完成
- 修复：在路由 effect 中添加 `watch` 等待 discovery ready

**如果 Step 2 FAIL**（explicit_key 解析失败）：
- 路径格式问题（但前面已验证正确）
- 修复：检查路径格式或 split 逻辑

**如果 Step 3 FAIL**（snapshot() == None）：
- snapshot 未被存储到 Signal
- 可能原因：apply_snapshot() 调用但 snapshot 仍为 None
- 修复：在 select_panel_project() 后显式等待 snapshot 更新

**如果 Step 4 FAIL**（from_snapshot 错误）：
- snapshot 格式/内容问题
- 修复：检查 snapshot schema 版本或字段缺失

**如果 Step 5 FAIL**（select_explicit 错误）：
- plugin_id 或 tab_id 在 snapshot 中不存在
- 或 visibility 检查失败
- 修复：确保 Gateway 返回的 descriptor 与导航的 key 匹配

### Phase 5：最小修复实施

根据 Phase 4 的诊断结果，选择对应修复方案（具体实施由诊断结果决定）。

## 硬约束

- 诊断日志仅允许使用 `leptos::logging::log!`（无 `console::log`、`eprintln`）
- 不新增 UI、不修改 API 契约、不改 Gateway
- 不改动已验证正确的 selector、navigation 逻辑、credential 流程
- 完整流程验证：6 phase PASS 包括 project_a routing success
- 所有测试 95/95 PASS，check_docs OK，git diff --check OK

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
