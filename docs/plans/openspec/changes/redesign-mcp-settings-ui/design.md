# Design: Redesign MCP Settings UI

## Overview

本文档详细描述 MCP 设置界面重构的架构设计，采用 macOS 标准的 Master-Detail（主从视图）布局，兼容 `claude_desktop_config.json` 格式。

## 1. 数据模型设计

### 1.1 服务器配置结构 (Rust)

```rust
// config/mod.rs

/// MCP 服务器配置（兼容 claude_desktop_config.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 服务器唯一标识符（内置服务使用固定 ID）
    pub id: String,

    /// 显示名称
    pub name: String,

    /// 服务器类型
    pub server_type: McpServerType,

    /// 是否启用
    pub enabled: bool,

    /// 传输方式（目前仅支持 stdio）
    pub transport: McpTransport,

    /// 可执行文件路径（外部服务器）
    pub command: Option<String>,

    /// 命令行参数
    pub args: Vec<String>,

    /// 环境变量
    pub env: HashMap<String, String>,

    /// Halo 中的触发命令（如 /mcp/git）
    pub trigger_command: Option<String>,

    /// 权限设置
    pub permissions: McpPermissions,

    /// SF Symbol 图标名称
    pub icon: String,

    /// 主题颜色（Hex）
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpServerType {
    /// 内置服务（Rust 原生实现）
    Builtin,
    /// 外部扩展（用户安装）
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    /// 标准输入/输出（子进程通信）
    Stdio,
    // Future: Http, WebSocket
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPermissions {
    /// 是否需要用户确认
    pub requires_confirmation: bool,

    /// 允许访问的文件路径（仅 fs 服务）
    pub allowed_paths: Vec<String>,

    /// 允许执行的命令（仅 shell 服务）
    pub allowed_commands: Vec<String>,
}
```

### 1.2 服务器状态 (Rust)

```rust
/// MCP 服务器运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    /// 运行中
    Running,
    /// 已停止
    Stopped,
    /// 初始化中
    Initializing,
    /// 错误（附带错误信息）
    Error,
}

/// 服务器状态信息（用于 UI 显示）
pub struct McpServerStatusInfo {
    pub status: McpServerStatus,
    pub message: Option<String>,
    pub last_error: Option<String>,
}
```

### 1.3 UniFFI 接口扩展

```idl
// aleph.udl

dictionary McpServerConfig {
    string id;
    string name;
    McpServerType server_type;
    boolean enabled;
    string? command;
    sequence<string> args;
    record<string, string> env;
    string? trigger_command;
    McpPermissions permissions;
    string icon;
    string color;
};

enum McpServerType {
    "Builtin",
    "External",
};

dictionary McpPermissions {
    boolean requires_confirmation;
    sequence<string> allowed_paths;
    sequence<string> allowed_commands;
};

dictionary McpServerStatusInfo {
    McpServerStatus status;
    string? message;
    string? last_error;
};

enum McpServerStatus {
    "Running",
    "Stopped",
    "Initializing",
    "Error",
};

interface AlephCore {
    // ... existing methods ...

    // MCP Server Management
    sequence<McpServerConfig> list_mcp_servers();
    McpServerConfig? get_mcp_server(string id);
    McpServerStatusInfo get_mcp_server_status(string id);

    [Throws=AlephException]
    void add_mcp_server(McpServerConfig config);

    [Throws=AlephException]
    void update_mcp_server(McpServerConfig config);

    [Throws=AlephException]
    void delete_mcp_server(string id);

    [Throws=AlephException]
    void start_mcp_server(string id);

    [Throws=AlephException]
    void stop_mcp_server(string id);

    sequence<string> get_mcp_server_logs(string id, u32 max_lines);

    string export_mcp_config_json();

    [Throws=AlephException]
    void import_mcp_config_json(string json);
};
```

## 2. UI 架构设计

### 2.1 组件层次结构

```
McpSettingsView (Root)
├── HSplitView
│   ├── McpServerListView (Sidebar, 200-300pt width)
│   │   ├── Section: "Built-in Core"
│   │   │   └── ForEach: McpServerRow (builtin servers)
│   │   ├── Section: "Extensions"
│   │   │   └── ForEach: McpServerRow (external servers)
│   │   └── AddServerButton
│   │
│   └── McpServerDetailView (Detail, remaining width)
│       ├── McpServerHeader (icon, name, status, toggle)
│       ├── McpCommandSection (command, args) - if External
│       ├── McpEnvVarSection (key-value editor)
│       ├── McpPermissionsSection (confirmation, paths)
│       ├── McpActionBar (logs, json toggle, save)
│       └── McpJsonEditor (if json mode enabled)
│
└── Sheet: McpServerLogView (log viewer)
```

### 2.2 SwiftUI 组件设计

#### McpServerListView (侧边栏)

```swift
struct McpServerListView: View {
    @Binding var selectedServerId: String?
    let servers: [McpServerConfig]
    let statuses: [String: McpServerStatusInfo]
    let onAddServer: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            List(selection: $selectedServerId) {
                Section(header: Text("Built-in Core")) {
                    ForEach(builtinServers, id: \.id) { server in
                        McpServerRow(
                            server: server,
                            status: statuses[server.id]
                        )
                        .tag(server.id)
                    }
                }

                Section(header: Text("Extensions")) {
                    ForEach(externalServers, id: \.id) { server in
                        McpServerRow(
                            server: server,
                            status: statuses[server.id]
                        )
                        .tag(server.id)
                    }
                }
            }
            .listStyle(.sidebar)

            Divider()

            Button(action: onAddServer) {
                Label("Add Server", systemImage: "plus")
            }
            .buttonStyle(.borderless)
            .padding(12)
        }
        .frame(minWidth: 200, maxWidth: 300)
    }
}
```

#### McpServerRow (列表项)

```swift
struct McpServerRow: View {
    let server: McpServerConfig
    let status: McpServerStatusInfo?

    var body: some View {
        HStack(spacing: 8) {
            // Icon
            Image(systemName: server.icon)
                .foregroundColor(Color(hex: server.color))
                .frame(width: 20)

            // Name
            Text(server.name)
                .lineLimit(1)

            Spacer()

            // Status indicator
            StatusDot(status: status?.status ?? .stopped)
        }
        .padding(.vertical, 4)
    }
}

struct StatusDot: View {
    let status: McpServerStatus

    var body: some View {
        Circle()
            .fill(statusColor)
            .frame(width: 8, height: 8)
    }

    var statusColor: Color {
        switch status {
        case .running: return .green
        case .stopped: return .gray
        case .initializing: return .yellow
        case .error: return .red
        }
    }
}
```

#### McpServerDetailView (详情面板)

```swift
struct McpServerDetailView: View {
    @ObservedObject var viewModel: McpServerDetailViewModel
    @State private var isJsonMode = false
    @State private var showingLogs = false

    var body: some View {
        VStack(spacing: 0) {
            // Header
            McpServerHeader(
                server: viewModel.server,
                status: viewModel.status,
                isEnabled: $viewModel.isEnabled
            )

            Divider()

            if isJsonMode {
                // JSON Editor
                McpJsonEditor(json: $viewModel.jsonConfig)
            } else {
                // GUI Form
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        // Command section (only for external)
                        if viewModel.server.serverType == .external {
                            McpCommandSection(
                                command: $viewModel.command,
                                args: $viewModel.args
                            )
                        }

                        // Environment variables
                        McpEnvVarSection(envVars: $viewModel.envVars)

                        // Permissions
                        McpPermissionsSection(
                            requiresConfirmation: $viewModel.requiresConfirmation,
                            allowedPaths: $viewModel.allowedPaths
                        )
                    }
                    .padding()
                }
            }

            Divider()

            // Action bar
            HStack {
                Button("Show Logs") {
                    showingLogs = true
                }

                Spacer()

                Toggle(isOn: $isJsonMode) {
                    Image(systemName: "curlybraces")
                }
                .toggleStyle(.button)
                .help("Switch to JSON mode")

                Button("Save") {
                    viewModel.save()
                }
                .buttonStyle(.borderedProminent)
                .disabled(!viewModel.hasChanges)
            }
            .padding()
        }
        .sheet(isPresented: $showingLogs) {
            McpServerLogView(serverId: viewModel.server.id)
        }
    }
}
```

#### McpEnvVarSection (环境变量编辑器)

```swift
struct McpEnvVarSection: View {
    @Binding var envVars: [(key: String, value: String)]
    @State private var visibleValues: Set<Int> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Environment Variables", systemImage: "key")
                .font(.headline)

            Text("API keys and secrets for this server")
                .font(.caption)
                .foregroundColor(.secondary)

            // Key-Value table
            VStack(spacing: 4) {
                ForEach(envVars.indices, id: \.self) { index in
                    HStack {
                        // Key field
                        TextField("KEY", text: $envVars[index].key)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)

                        // Value field (secure or plain)
                        if visibleValues.contains(index) {
                            TextField("Value", text: $envVars[index].value)
                                .textFieldStyle(.roundedBorder)
                        } else {
                            SecureField("Value", text: $envVars[index].value)
                                .textFieldStyle(.roundedBorder)
                        }

                        // Toggle visibility
                        Button {
                            toggleVisibility(index)
                        } label: {
                            Image(systemName: visibleValues.contains(index)
                                ? "eye.slash" : "eye")
                        }
                        .buttonStyle(.borderless)

                        // Delete
                        Button {
                            envVars.remove(at: index)
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .foregroundColor(.secondary)
                        }
                        .buttonStyle(.borderless)
                    }
                }
            }

            Button {
                envVars.append(("", ""))
            } label: {
                Label("Add Variable", systemImage: "plus")
            }
            .buttonStyle(.borderless)
        }
        .padding()
        .background(Color(nsColor: .controlBackgroundColor))
        .cornerRadius(8)
    }

    func toggleVisibility(_ index: Int) {
        if visibleValues.contains(index) {
            visibleValues.remove(index)
        } else {
            visibleValues.insert(index)
        }
    }
}
```

## 3. 配置格式兼容性

### 3.1 claude_desktop_config.json 格式

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/username/Desktop"]
    },
    "git": {
      "command": "uvx",
      "args": ["mcp-server-git", "--repository", "/path/to/repo"]
    },
    "postgres": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"],
      "env": {
        "PGPASSWORD": "secret"
      }
    }
  }
}
```

### 3.2 Aleph config.toml 格式

```toml
[mcp]
enabled = true

# 内置服务配置
[mcp.builtin]
fs_enabled = true
git_enabled = true
shell_enabled = false
system_info_enabled = true

[mcp.builtin.fs]
allowed_roots = ["~/Documents", "~/Projects"]

[mcp.builtin.git]
allowed_repos = ["~/Projects/*"]

[mcp.builtin.shell]
allowed_commands = ["ls", "pwd", "git"]
timeout_seconds = 30

# 外部服务器配置（兼容 claude_desktop_config.json）
[[mcp.servers]]
id = "linear"
name = "Linear"
command = "npx"
args = ["-y", "@linear/mcp-server"]
env = { LINEAR_API_KEY = "lin_api_xxx" }
icon = "ticket"
color = "#5E6AD2"
requires_confirmation = true

[[mcp.servers]]
id = "postgres"
name = "PostgreSQL"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"]
env = { PGPASSWORD = "secret" }
icon = "cylinder"
color = "#336791"
requires_confirmation = true
```

### 3.3 导入/导出逻辑

```rust
impl AlephCore {
    /// 导出为 claude_desktop_config.json 格式
    pub fn export_mcp_config_json(&self) -> String {
        let config = self.lock_config();
        let mut servers = serde_json::Map::new();

        for server in &config.mcp.servers {
            let mut server_obj = serde_json::Map::new();
            server_obj.insert("command".to_string(), json!(server.command));
            server_obj.insert("args".to_string(), json!(server.args));
            if !server.env.is_empty() {
                server_obj.insert("env".to_string(), json!(server.env));
            }
            servers.insert(server.id.clone(), serde_json::Value::Object(server_obj));
        }

        let export = json!({ "mcpServers": servers });
        serde_json::to_string_pretty(&export).unwrap_or_default()
    }

    /// 从 claude_desktop_config.json 格式导入
    pub fn import_mcp_config_json(&self, json: String) -> Result<()> {
        let parsed: serde_json::Value = serde_json::from_str(&json)?;
        let servers = parsed.get("mcpServers").ok_or(AlephError::InvalidConfig)?;

        // Convert and merge...
        Ok(())
    }
}
```

## 4. 服务器生命周期管理

### 4.1 状态机

```
                    ┌─────────────┐
                    │   Stopped   │
                    └──────┬──────┘
                           │ start()
                           ↓
                    ┌─────────────┐
         ┌─────────│ Initializing│─────────┐
         │         └─────────────┘         │
         │ success       │ error          │ timeout
         ↓               ↓                 ↓
  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
  │   Running   │  │    Error    │  │    Error    │
  └──────┬──────┘  └─────────────┘  └─────────────┘
         │ stop()
         ↓
  ┌─────────────┐
  │   Stopped   │
  └─────────────┘
```

### 4.2 日志收集

```rust
pub struct McpServerProcess {
    process: Option<Child>,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
    max_log_lines: usize,
}

impl McpServerProcess {
    fn capture_stderr(&mut self) {
        if let Some(stderr) = self.process.as_mut().and_then(|p| p.stderr.take()) {
            let buffer = self.log_buffer.clone();
            let max_lines = self.max_log_lines;

            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    let mut buf = buffer.lock().unwrap();
                    if buf.len() >= max_lines {
                        buf.pop_front();
                    }
                    buf.push_back(line);
                }
            });
        }
    }
}
```

## 5. 安全考虑

### 5.1 环境变量安全

- API Key 等敏感值使用 `SecureField` 显示
- 导出 JSON 时提示用户注意安全
- 考虑使用 macOS Keychain 存储敏感值（Future）

### 5.2 命令执行安全

- 内置服务无需外部命令
- 外部服务器命令需用户显式配置
- `requires_confirmation` 默认为 `true`
- 可配置允许的路径白名单

### 5.3 沙箱限制

- macOS App Sandbox 限制了文件访问
- 需要用户授权特定目录
- 使用 `NSOpenPanel` 让用户选择路径

## 6. 本地化要求

新增的本地化字符串：

```
// en.lproj/Localizable.strings
"settings.mcp.server_list.builtin" = "Built-in Core";
"settings.mcp.server_list.extensions" = "Extensions";
"settings.mcp.server_list.add" = "Add Server";
"settings.mcp.detail.status.running" = "Running";
"settings.mcp.detail.status.stopped" = "Stopped";
"settings.mcp.detail.status.initializing" = "Initializing...";
"settings.mcp.detail.status.error" = "Error";
"settings.mcp.detail.command" = "Command";
"settings.mcp.detail.args" = "Arguments";
"settings.mcp.detail.env_vars" = "Environment Variables";
"settings.mcp.detail.env_vars_description" = "API keys and secrets for this server";
"settings.mcp.detail.permissions" = "Permissions";
"settings.mcp.detail.requires_confirmation" = "Ask for confirmation before tool execution";
"settings.mcp.detail.auto_approve" = "Auto-approve all tool calls (dangerous)";
"settings.mcp.detail.allowed_paths" = "Allowed Paths";
"settings.mcp.detail.show_logs" = "Show Logs";
"settings.mcp.detail.json_mode" = "JSON Mode";
"settings.mcp.detail.import" = "Import Config";
"settings.mcp.detail.export" = "Export Config";
```

## 7. 测试策略

### 7.1 单元测试 (Rust)

- 配置解析测试
- JSON 导入/导出测试
- 服务器状态转换测试

### 7.2 UI 测试 (Swift)

- 列表选择状态测试
- 表单数据绑定测试
- JSON 模式切换测试

### 7.3 集成测试

- 内置服务启动/停止
- 外部服务器生命周期
- 配置持久化

## 8. 未来扩展

### 8.1 Keychain 集成

使用 macOS Keychain 存储 API Key，而非明文存储在 config.toml。

### 8.2 服务器市场

提供一个官方/社区维护的 MCP 服务器列表，一键安装。

### 8.3 运行时自动检测

自动检测系统是否安装 Node.js/Python/uvx，给出安装建议。
