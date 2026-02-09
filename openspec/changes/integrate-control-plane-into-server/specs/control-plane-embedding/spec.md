# Spec: Control Plane Embedding

**Capability**: `control-plane-embedding`
**Status**: Draft
**Related Change**: `integrate-control-plane-into-server`

## Overview

定义 ControlPlane（原 Dashboard）如何嵌入到 Server 二进制文件中，包括构建流程、资源嵌入和 HTTP 服务。

---

## ADDED Requirements

### Requirement: Automated Build Process

The system SHALL automatically build ControlPlane during Server compilation without manual intervention.

#### Scenario: Developer builds Server

**Given** 开发者执行 `cargo build -p alephcore --features control-plane`

**When** Cargo 运行 `build.rs` 脚本

**Then**
- `build.rs` 自动调用 `trunk build --release` 编译 ControlPlane
- 编译产物生成到 `core/ui/control_plane/dist/`
- 如果 ControlPlane 编译失败，整个构建失败并显示错误信息
- 如果 `ui/control_plane/` 目录不存在，跳过 ControlPlane 构建

**Acceptance Criteria**:
- `cargo build` 一次性完成所有编译
- 开发者无需手动运行 `trunk build`
- 构建失败时有清晰的错误提示

---

### Requirement: Asset Embedding

The system SHALL embed ControlPlane static assets (HTML/CSS/JS/WASM) into the Server binary file.

#### Scenario: Server binary contains ControlPlane assets

**Given** Server 已编译完成

**When** 检查二进制文件

**Then**
- 二进制文件包含 `core/ui/control_plane/dist/` 中的所有文件
- 文件以 `cp/` 前缀存储
- 支持 gzip 压缩以减小体积
- 二进制文件大小增加 < 5MB（压缩后）

**Acceptance Criteria**:
- 使用 `rust-embed` 宏嵌入资源
- 支持 `ControlPlaneAssets::get("cp/index.html")`
- 嵌入的资源可以在运行时访问

#### Scenario: Asset retrieval at runtime

**Given** Server 正在运行

**When** 代码调用 `ControlPlaneAssets::get("cp/index.html")`

**Then**
- 返回 `Some(Cow<'static, [u8]>)` 包含文件内容
- 如果文件不存在，返回 `None`
- 访问速度 < 1ms（内存访问）

**Acceptance Criteria**:
- 零磁盘 I/O
- 零网络请求
- 线程安全

---

### Requirement: HTTP Static File Serving

The Server SHALL provide HTTP static file serving for ControlPlane assets.

#### Scenario: Access ControlPlane index page

**Given** Server 正在运行在 `http://127.0.0.1:18789`

**When** 用户访问 `http://127.0.0.1:18789/cp`

**Then**
- 返回 HTTP 200
- Content-Type: `text/html; charset=utf-8`
- 响应体包含 `index.html` 内容
- 响应时间 < 50ms

**Acceptance Criteria**:
- 支持 SPA 路由（所有 `/cp/*` 路径返回 `index.html`）
- 正确的 MIME 类型检测
- 缓存头设置（`Cache-Control: public, max-age=31536000`）

#### Scenario: Access static assets

**Given** Server 正在运行

**When** 用户访问 `http://127.0.0.1:18789/cp/assets/main.js`

**Then**
- 返回 HTTP 200
- Content-Type: `application/javascript`
- 响应体包含 JS 文件内容
- 支持 gzip 压缩（如果客户端支持）

**Acceptance Criteria**:
- 支持所有常见文件类型（.js, .css, .wasm, .svg, .png, .woff2）
- 正确的 MIME 类型
- 压缩传输

#### Scenario: Handle missing files

**Given** Server 正在运行

**When** 用户访问不存在的文件 `http://127.0.0.1:18789/cp/nonexistent.js`

**Then**
- 返回 HTTP 404
- 响应体: "Not Found"

**Acceptance Criteria**:
- 不会崩溃
- 不会返回 `index.html`（仅对非文件路径返回）

---

### Requirement: Development Mode Support

The system SHALL support hot reload during development without recompiling the Server.

#### Scenario: Hot reload during development

**Given** 开发者正在开发 ControlPlane UI

**When** 开发者修改 `core/ui/control_plane/src/app.rs`

**Then**
- `trunk serve` 自动重新编译
- 浏览器自动刷新
- 无需重启 Server

**Acceptance Criteria**:
- 支持 `trunk serve` 独立运行
- 支持 `--proxy-backend=http://127.0.0.1:18789` 代理 API 请求
- 修改后 < 2s 看到效果

---

### Requirement: Feature Flag Control

The system SHALL control ControlPlane embedding through a feature flag to support optional compilation.

#### Scenario: Build without ControlPlane

**Given** 开发者执行 `cargo build -p alephcore`（不带 `--features control-plane`）

**When** 编译完成

**Then**
- 不编译 ControlPlane
- 不嵌入静态资源
- 二进制文件更小
- 访问 `/cp` 返回 404

**Acceptance Criteria**:
- `control-plane` feature flag 存在
- 默认不启用（可选）
- 编译时间减少 ~30%

#### Scenario: Build with ControlPlane

**Given** 开发者执行 `cargo build -p alephcore --features control-plane`

**When** 编译完成

**Then**
- 自动编译 ControlPlane
- 嵌入静态资源
- 访问 `/cp` 正常工作

**Acceptance Criteria**:
- 一次性完成所有编译
- 无需额外步骤

---

## Implementation Notes

### Dependencies

```toml
[dependencies]
rust-embed = { version = "8.0", features = ["compression"] }
mime_guess = "2.0"
axum = "0.7"

[build-dependencies]
# 无需额外依赖，使用 std::process::Command
```

### File Structure

```
core/
├── build.rs                      # 自动化构建脚本
├── Cargo.toml                    # 添加 control-plane feature
├── src/
│   └── gateway/
│       └── control_plane/
│           ├── mod.rs            # 模块导出
│           ├── assets.rs         # RustEmbed 宏
│           └── server.rs         # HTTP 路由
└── ui/
    └── control_plane/            # Leptos 前端代码
        ├── Cargo.toml
        ├── Trunk.toml
        ├── index.html
        ├── src/
        └── dist/                 # 编译产物（git ignore）
```

### Build Script Example

```rust
// core/build.rs
use std::process::Command;
use std::path::Path;

fn main() {
    #[cfg(feature = "control-plane")]
    {
        println!("cargo:rerun-if-changed=ui/control_plane/src");
        println!("cargo:rerun-if-changed=ui/control_plane/Cargo.toml");

        let control_plane_dir = Path::new("ui/control_plane");
        if control_plane_dir.exists() {
            println!("cargo:warning=Building ControlPlane...");

            let status = Command::new("trunk")
                .args(&["build", "--release"])
                .current_dir(control_plane_dir)
                .status()
                .expect("Failed to execute trunk");

            if !status.success() {
                panic!("ControlPlane build failed");
            }
        }
    }
}
```

### Asset Embedding Example

```rust
// core/src/gateway/control_plane/assets.rs
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "ui/control_plane/dist/"]
#[prefix = "cp/"]
#[compression = "gzip"]
pub struct ControlPlaneAssets;
```

---

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_exists() {
        let index = ControlPlaneAssets::get("cp/index.html");
        assert!(index.is_some());
    }

    #[test]
    fn test_asset_not_found() {
        let missing = ControlPlaneAssets::get("cp/nonexistent.js");
        assert!(missing.is_none());
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_serve_index() {
    let app = create_control_plane_router();
    let response = app
        .oneshot(Request::builder().uri("/cp").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
}
```

---

## Performance Requirements

- **Build Time**: ControlPlane 编译 < 60s
- **Binary Size**: 增加 < 5MB（gzip 压缩后）
- **Startup Time**: Server 启动时间增加 < 100ms
- **Response Time**: 静态文件响应 < 50ms

---

## Security Considerations

- 嵌入的资源是只读的，无法在运行时修改
- 不暴露源代码，仅暴露编译后的 WASM
- 支持 Content-Security-Policy 头
- 支持 HTTPS（如果 Gateway 启用 TLS）

---

## Migration Path

1. **Phase 1**: 创建 `core/ui/control_plane/` 并复制代码
2. **Phase 2**: 实现 `build.rs` 和 `assets.rs`
3. **Phase 3**: 实现 `server.rs` 和路由集成
4. **Phase 4**: 测试和验证
5. **Phase 5**: 归档旧的 `clients/dashboard/`

---

## Related Specs

- `gateway-integration`: Gateway 如何集成 ControlPlane 路由
- `client-simplification`: Client 如何访问 ControlPlane
