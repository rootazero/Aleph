# Server 开发与发布

> 从 [CLAUDE.md](../../CLAUDE.md) 拆分的详细 Server 开发指南。

---

## Server 架构概览

Aleph Server 是一个自包含的 Rust 二进制程序（`aleph`），包含：
- **Gateway + Control Plane**: HTTP/WebSocket 统一服务 - 端口 18790（WebSocket 路径 `/ws`，Control Plane UI 路径 `/cp`）
- **Agent Loop**: AI 代理执行引擎
- **Tool System**: 工具调用和执行
- **Memory System**: 向量数据库和事实存储

## 开发环境设置

### 1. 依赖安装

```bash
# Rust 工具链
rustup default stable
rustup target add wasm32-unknown-unknown

# WASM 工具
cargo install wasm-bindgen-cli

# 可选：Trunk (用于 UI 开发)
cargo install trunk
```

### 2. 环境变量配置

```bash
# ~/.aleph/config.toml 或环境变量
export ANTHROPIC_API_KEY="your-api-key"
export ANTHROPIC_BASE_URL="https://api.anthropic.com"  # 可选
```

### 3. 数据库初始化

```bash
# Server 首次启动时会自动创建数据库
# 位置：~/.aleph/
mkdir -p ~/.aleph
```

## 开发流程

### 快速启动（开发模式）

```bash
# 1. 启动 Server（所有功能始终编译）
cargo run --bin aleph

# 2. 后台运行
cargo run --bin aleph -- --daemon

# 3. 指定端口
cargo run --bin aleph -- --port 8080
```

### 完整开发流程

```bash
# 1. 修改 Core 代码
vim core/src/gateway/...

# 2. 运行测试
cargo test

# 3. 构建并运行
cargo run --bin aleph

# 4. 查看日志
tail -f ~/.aleph/aleph.log  # 如果使用 --daemon
```

## Control Plane UI 开发流程

Control Plane UI 是嵌入在 Server 二进制中的 Web 管理界面，使用 Leptos (WASM) 构建。

### UI 开发环境构建

```bash
# 1. 构建 WASM 库
cd apps/panel
cargo build --lib --target wasm32-unknown-unknown --release

# 2. 生成 JS 绑定
wasm-bindgen --target web \
  --out-dir dist \
  --out-name aleph-panel \
  /Volumes/TBU4/Workspace/Aleph/target/wasm32-unknown-unknown/release/aleph_panel.wasm

# 3. 编译 Tailwind CSS
npm run build:css  # 编译 styles/tailwind.css -> dist/tailwind.css

# 4. 更新 index.html（确保引用正确的文件名）
# 编辑 dist/index.html，引用：
# - /aleph-panel.js
# - /aleph-panel_bg.wasm
# - /tailwind.css

# 5. 构建 Server（会自动嵌入 dist/ 中的资源）
cd ../../..
cargo build --bin aleph
```

### UI 快速重建

```bash
# 修改 Leptos 代码后：
cd apps/panel && \
cargo build --lib --target wasm32-unknown-unknown --release && \
wasm-bindgen --target web --out-dir dist --out-name aleph-panel \
  /Volumes/TBU4/Workspace/Aleph/target/wasm32-unknown-unknown/release/aleph_panel.wasm && \
npm run build:css && \
cd ../../.. && \
cargo build --bin aleph
```

### 资源嵌入机制

Control Plane UI 使用 `rust-embed` 在**编译时**嵌入资源：

```rust
#[derive(RustEmbed)]
#[folder = "apps/panel/dist/"]
pub struct ControlPlaneAssets;
```

**关键特性**：
- 编译时嵌入：所有 HTML/CSS/JS/WASM 文件打包进二进制
- 单文件分发：只需分发 `aleph` 可执行文件
- 零运行时依赖：不需要额外的静态文件目录
- 自动跳过构建：如果 `dist/` 存在，`build.rs` 会跳过 UI 构建

### WASM 初始化机制

**重要**: Control Plane 使用库目标（lib）而非二进制目标（bin）构建 WASM。初始化代码在 `lib.rs` 中：

```rust
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn main() {
    use leptos::prelude::*;
    console_error_panic_hook::set_once();
    mount_to_body(app::App);
}
```

**说明**：
- `#[wasm_bindgen(start)]` 使函数在 WASM 模块加载时自动执行
- 无需在 HTML 中手动调用初始化函数
- `main.rs` 不会被编译（因为没有二进制目标）
- 所有初始化逻辑必须在 `lib.rs` 中

### Tailwind CSS 编译

Control Plane UI 使用 Tailwind CSS v3 进行样式管理，CSS 在构建时编译并嵌入二进制：

```bash
# 安装依赖（首次）
cd apps/panel
npm install

# 编译 CSS
npm run build:css

# 输出：dist/tailwind.css (约 40KB minified)
```

**配置文件**：
- `tailwind.config.js`: 配置内容扫描路径（Rust 源文件 + HTML）
- `styles/tailwind.css`: 源 CSS 文件（包含 @tailwind 指令）
- `dist/tailwind.css`: 编译后的 CSS（嵌入到二进制）

**关键特性**：
- 本地编译：无需 CDN，完全离线可用
- 自动扫描：从 Rust 源文件中提取 Tailwind 类名
- 生产优化：minified，仅包含使用的类
- 嵌入二进制：通过 rust-embed 打包进可执行文件

### macOS Native App

```bash
# Build macOS app (requires Xcode + xcodegen)
cd apps/macos-native && scripts/build-macos.sh

# Development build
cd apps/macos-native && xcodegen generate && xcodebuild -scheme Aleph -configuration Debug build

# Run tests
cd apps/macos-native && xcodebuild -scheme Aleph -configuration Debug test -destination 'platform=macOS'
```

### Pure Server Install (no desktop)

```bash
# Install via curl (downloads latest release)
curl -fsSL https://raw.githubusercontent.com/user/aleph/main/scripts/install.sh | bash

# Or build from source
cargo build --bin aleph --release
```

## 发布流程

### 1. 准备发布

```bash
# 确保所有测试通过
cargo test --workspace

# 确保 UI 已构建（如果需要）
ls apps/panel/dist/
# 应包含：index.html, aleph-panel.js, aleph-panel_bg.wasm, tailwind.css
```

### 2. 构建 Release 版本

```bash
# 构建 Release（所有功能始终编译）
cargo build --bin aleph --release

# 查看二进制大小
ls -lh target/release/aleph
```

### 3. 验证构建

```bash
# 验证二进制可执行
./target/release/aleph --version

# 验证嵌入的资源
strings target/release/aleph | grep "index.html"

# 测试运行
./target/release/aleph --help
```

### 4. 分发方式

**方式 1: 直接分发二进制**
```bash
# 复制到系统路径
sudo cp target/release/aleph /usr/local/bin/

# 或创建符号链接
sudo ln -s $(pwd)/target/release/aleph /usr/local/bin/aleph
```

**方式 2: 使用 cargo install**
```bash
# 从本地路径安装
cargo install --path core --bin aleph

# 安装后位置：~/.cargo/bin/aleph
```

**方式 3: 发布到 crates.io**
```bash
# 1. 更新版本号
vim core/Cargo.toml  # 修改 version

# 2. 发布
cd core
cargo publish --dry-run  # 预检查
cargo publish            # 正式发布

# 3. 用户安装
cargo install alephcore --bin aleph
```

**方式 4: 创建安装包**
```bash
# macOS: 创建 .pkg 或 .dmg
# Linux: 创建 .deb 或 .rpm
# 使用 cargo-bundle 或 cargo-deb
cargo install cargo-deb
cargo deb --package alephcore
```

### 5. 部署配置

```bash
# 创建配置文件
mkdir -p ~/.aleph
cat > ~/.aleph/config.toml << EOF
[agent.main]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[gateway]
bind = "127.0.0.1"
port = 18790
EOF

# 设置环境变量
export ANTHROPIC_API_KEY="your-api-key"

# 启动服务
aleph --daemon --log-file ~/.aleph/server.log
```

### 6. 系统服务配置

**macOS (launchd)**
```xml
<!-- ~/Library/LaunchAgents/com.aleph.server.plist -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.aleph.server</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/aleph</string>
        <string>--daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
```

```bash
# 加载服务
launchctl load ~/Library/LaunchAgents/com.aleph.server.plist

# 启动服务
launchctl start com.aleph.server

# 查看状态
launchctl list | grep aleph
```

**Linux (systemd)**
```ini
# /etc/systemd/system/aleph.service
[Unit]
Description=Aleph AI Server
After=network.target

[Service]
Type=simple
User=aleph
ExecStart=/usr/local/bin/aleph
Restart=on-failure
Environment="ANTHROPIC_API_KEY=your-api-key"

[Install]
WantedBy=multi-user.target
```

```bash
# 重载配置
sudo systemctl daemon-reload

# 启动服务
sudo systemctl start aleph

# 开机自启
sudo systemctl enable aleph

# 查看状态
sudo systemctl status aleph
```

## 故障排查

### Control Plane UI 问题

**问题：Trunk 构建失败**
```bash
# 解决方案：使用 wasm-bindgen 手动构建（见上文）
# Trunk 在工作区环境中可能遇到目标识别问题
```

**问题：路由显示 404**
```bash
# 原因：WASM 中的路由基础路径配置错误
# 解决方案：确保 Leptos Router 使用根路径 "/"
# 检查 index.html 中的资源路径是否为绝对路径
```

**问题：Server 构建时 UI 构建失败**
```bash
# build.rs 已配置为优雅降级：
# - 如果 dist/ 存在 → 跳过构建
# - 如果 Trunk 失败 → 警告但不中断 Server 构建
# Server 可以独立运行，UI 为可选功能
```

### Server 运行问题

**问题：端口被占用**
```bash
# 查找占用进程
lsof -i :18790

# 杀死进程
kill -9 <PID>

# 或使用不同端口
aleph --port 8080
```

**问题：API 密钥未配置**
```bash
# 检查环境变量
echo $ANTHROPIC_API_KEY

# 或检查配置文件
cat ~/.aleph/config.toml

# 设置环境变量
export ANTHROPIC_API_KEY="your-api-key"
```

**问题：数据库损坏**
```bash
# 备份并重建
mv ~/.aleph/sessions.db ~/.aleph/sessions.db.bak
mv ~/.aleph/memory.db ~/.aleph/memory.db.bak

# 重启 Server（会自动创建新数据库）
aleph
```

## 性能优化

### 编译优化

```toml
# Cargo.toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"
```

### 运行时优化

```bash
# 增加 Tokio 线程数
TOKIO_WORKER_THREADS=8 aleph

# 调整日志级别
RUST_LOG=info aleph  # 生产环境
RUST_LOG=debug aleph # 调试模式
```
