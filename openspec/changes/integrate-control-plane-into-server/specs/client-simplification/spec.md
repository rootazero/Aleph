# Spec: Client Simplification

**Capability**: `client-simplification`
**Status**: Draft
**Related Change**: `integrate-control-plane-into-server`

## Overview

定义如何简化 Client（特别是 macOS Client），移除所有配置 UI，仅保留对话交互功能，并提供访问 ControlPlane 的入口。

---

## REMOVED Requirements

### Requirement: Client-side Configuration UI

**Removed**: macOS Client 不再包含配置 UI

**Rationale**: 所有配置管理功能已迁移到 ControlPlane

**Affected Files**:
- `clients/macos/Aleph/Sources/BehaviorSettingsView.swift`
- `clients/macos/Aleph/Sources/GuestsSettingsView.swift`
- `clients/macos/Aleph/Sources/McpSettingsView.swift`
- `clients/macos/Aleph/Sources/PluginsSettingsView.swift`
- `clients/macos/Aleph/Sources/PoliciesSettingsView.swift`
- `clients/macos/Aleph/Sources/SearchSettingsView.swift`
- `clients/macos/Aleph/Sources/SecuritySettingsView.swift`
- `clients/macos/Aleph/Sources/SkillsSettingsView.swift`

**Migration Path**: 用户通过 ControlPlane 管理配置

---

### Requirement: Client-side Configuration Write

**Removed**: macOS Client 不再写入配置文件

**Rationale**: 配置写入统一由 ControlPlane 通过 Server Functions 完成

**Affected Code**:
- 移除 `ConfigManager.save()` 调用
- 移除配置表单的提交逻辑
- 移除配置验证逻辑

**Migration Path**: 配置修改通过 ControlPlane 完成

---

## ADDED Requirements

### Requirement: ControlPlane Access Button

The macOS Client SHALL provide an entry point to access ControlPlane.

#### Scenario: Open ControlPlane from menu bar

**Given** macOS Client 正在运行

**When** 用户点击菜单栏图标 → "打开控制面板"

**Then**
- 使用 `NSWorkspace.shared.open()` 打开默认浏览器
- 导航到 `http://127.0.0.1:18789/cp`
- ControlPlane 在浏览器中加载

**Acceptance Criteria**:
- 点击后 < 1s 打开浏览器
- 如果 Server 未运行，显示友好提示
- 支持自定义端口

#### Scenario: Open ControlPlane from settings window

**Given** 用户打开设置窗口（简化版）

**When** 用户点击"高级设置"按钮

**Then**
- 打开浏览器并导航到 ControlPlane
- 设置窗口保持打开（或关闭，取决于设计）

**Acceptance Criteria**:
- 按钮位置明显
- 图标清晰（齿轮或外部链接图标）
- 支持键盘快捷键（Cmd+,）

---

### Requirement: Configuration Read-Only Display

The macOS Client SHALL display current configuration in read-only mode.

#### Scenario: Display current AI provider

**Given** macOS Client 正在运行

**When** 用户查看"当前设置"

**Then**
- 显示当前使用的 AI Provider
- 显示当前模型
- 显示"在控制面板中修改"链接

**Acceptance Criteria**:
- 只读显示
- 实时更新（监听 `config_updated` 事件）
- 清晰的提示信息

#### Scenario: Config updated notification

**Given** macOS Client 正在运行

**When** 配置在 ControlPlane 中被修改

**Then**
- Client 接收 `config_updated` 事件
- 更新显示的配置信息
- 显示通知："配置已更新"

**Acceptance Criteria**:
- 延迟 < 500ms
- 无需刷新
- 通知不打扰用户

---

### Requirement: Simplified Settings Window

The macOS Client's settings window SHALL be minimalist, containing only essential information.

#### Scenario: Open simplified settings

**Given** macOS Client 正在运行

**When** 用户打开设置窗口

**Then** 设置窗口包含：
- 当前连接状态（Server 地址、连接状态）
- 当前 AI Provider（只读）
- "打开控制面板"按钮
- 关于信息（版本号、许可证）

**Acceptance Criteria**:
- 窗口大小 < 400x300
- 加载时间 < 100ms
- 无复杂表单

---

### Requirement: Deep Link Support

The macOS Client SHALL support Deep Links to allow navigation from ControlPlane back to the Client.

#### Scenario: Open conversation from ControlPlane

**Given** 用户在 ControlPlane 中查看对话历史

**When** 用户点击"在客户端中打开"

**Then**
- 触发 Deep Link `aleph://conversation/<id>`
- macOS Client 激活并显示对话窗口
- 加载指定的对话

**Acceptance Criteria**:
- 支持 URL Scheme `aleph://`
- 支持多种 Deep Link 类型（conversation, settings, etc.）
- 安全验证（防止恶意链接）

---

## Implementation Notes

### ControlPlane Access Button

```swift
// clients/macos/Aleph/Sources/ControlPlaneLinkButton.swift
import SwiftUI

struct ControlPlaneLinkButton: View {
    @EnvironmentObject var connectionState: ConnectionState

    var body: some View {
        Button(action: openControlPlane) {
            Label("打开控制面板", systemImage: "gearshape.2")
        }
        .disabled(!connectionState.isConnected)
    }

    private func openControlPlane() {
        guard let serverUrl = connectionState.serverUrl else {
            showAlert("Server 未连接")
            return
        }

        let controlPlaneUrl = "\(serverUrl)/cp"
        if let url = URL(string: controlPlaneUrl) {
            NSWorkspace.shared.open(url)
        }
    }
}
```

### Simplified Settings View

```swift
// clients/macos/Aleph/Sources/SettingsView.swift
import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var connectionState: ConnectionState
    @State private var currentProvider: String = "Loading..."

    var body: some View {
        VStack(spacing: 20) {
            // Connection Status
            HStack {
                Circle()
                    .fill(connectionState.isConnected ? Color.green : Color.red)
                    .frame(width: 10, height: 10)
                Text(connectionState.isConnected ? "已连接" : "未连接")
            }

            // Current Provider (Read-only)
            VStack(alignment: .leading) {
                Text("当前 AI Provider")
                    .font(.headline)
                Text(currentProvider)
                    .foregroundColor(.secondary)
            }

            // Open ControlPlane Button
            ControlPlaneLinkButton()
                .buttonStyle(.borderedProminent)

            Spacer()

            // About
            Text("Aleph v\(Bundle.main.appVersion)")
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .padding()
        .frame(width: 400, height: 300)
        .onAppear(perform: loadCurrentProvider)
        .onReceive(NotificationCenter.default.publisher(for: .configUpdated)) { _ in
            loadCurrentProvider()
        }
    }

    private func loadCurrentProvider() {
        // 从 Server 读取当前 Provider
        Task {
            currentProvider = await fetchCurrentProvider()
        }
    }
}
```

### Deep Link Handler

```swift
// clients/macos/Aleph/Sources/DeepLinkHandler.swift
import SwiftUI

class DeepLinkHandler: ObservableObject {
    func handle(url: URL) {
        guard url.scheme == "aleph" else { return }

        switch url.host {
        case "conversation":
            if let id = url.pathComponents.last {
                openConversation(id: id)
            }
        case "settings":
            openSettings()
        default:
            break
        }
    }

    private func openConversation(id: String) {
        // 打开对话窗口并加载指定对话
        NotificationCenter.default.post(
            name: .openConversation,
            object: nil,
            userInfo: ["id": id]
        )
    }

    private func openSettings() {
        // 打开设置窗口
        NotificationCenter.default.post(name: .openSettings, object: nil)
    }
}
```

---

## Testing Strategy

### Unit Tests

```swift
func testOpenControlPlane() {
    let button = ControlPlaneLinkButton()
    let connectionState = ConnectionState()
    connectionState.serverUrl = "http://127.0.0.1:18789"
    connectionState.isConnected = true

    button.environmentObject(connectionState)

    // 模拟点击
    button.openControlPlane()

    // 验证 NSWorkspace.shared.open 被调用
    XCTAssertTrue(mockWorkspace.openCalled)
    XCTAssertEqual(mockWorkspace.lastOpenedUrl, "http://127.0.0.1:18789/cp")
}
```

### Integration Tests

```swift
func testConfigUpdateNotification() async {
    let client = AlephClient()
    await client.connect()

    // 监听配置更新
    var configUpdated = false
    let cancellable = NotificationCenter.default
        .publisher(for: .configUpdated)
        .sink { _ in
            configUpdated = true
        }

    // 在 ControlPlane 中修改配置
    await updateConfigInControlPlane()

    // 等待通知
    try await Task.sleep(nanoseconds: 1_000_000_000) // 1s

    XCTAssertTrue(configUpdated)
}
```

---

## Migration Guide

### For Users

1. **打开控制面板**：
   - 点击菜单栏图标 → "打开控制面板"
   - 或使用快捷键 `Cmd+,`

2. **管理配置**：
   - 所有配置现在在浏览器中的 ControlPlane 管理
   - 修改后自动同步到 Client

3. **查看当前设置**：
   - Client 设置窗口显示当前配置（只读）
   - 点击"打开控制面板"进行修改

### For Developers

1. **移除设置 UI 文件**：
   ```bash
   rm clients/macos/Aleph/Sources/*SettingsView.swift
   # 保留 SettingsView.swift（简化版）
   ```

2. **添加 ControlPlane 链接**：
   ```swift
   // 在菜单栏添加
   menu.addItem(withTitle: "打开控制面板", action: #selector(openControlPlane), keyEquivalent: ",")
   ```

3. **监听配置更新**：
   ```swift
   NotificationCenter.default.addObserver(
       forName: .configUpdated,
       object: nil,
       queue: .main
   ) { _ in
       self.reloadConfig()
   }
   ```

---

## Performance Requirements

- **设置窗口加载**: < 100ms
- **ControlPlane 打开**: < 1s
- **配置更新延迟**: < 500ms
- **Deep Link 响应**: < 200ms

---

## Security Considerations

- Deep Link 必须验证来源（防止恶意链接）
- 配置读取使用安全的 RPC 调用
- 不在 Client 存储敏感信息（API Keys）
- 所有配置修改通过 ControlPlane 完成，有审计日志

---

## Related Specs

- `control-plane-embedding`: ControlPlane 如何嵌入
- `gateway-integration`: Gateway 如何提供 ControlPlane 服务
