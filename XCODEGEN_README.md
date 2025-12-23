# XcodeGen 工作流指南

本项目使用 [XcodeGen](https://github.com/yonaskolb/XcodeGen) 来自动生成 Xcode 项目文件，避免手动维护复杂的 `.xcodeproj` 文件。

## 为什么使用 XcodeGen？

- **版本控制友好**：只需提交 `project.yml`，不再需要提交庞大的 `.xcodeproj` 文件
- **减少合并冲突**：YAML 配置文件比 Xcode 项目文件更容易合并
- **自动文件发现**：添加新文件后，运行 `xcodegen` 即可自动包含到项目中
- **一致性保证**：团队成员都使用相同的配置生成项目

## 安装 XcodeGen

```bash
# 使用 Homebrew 安装
brew install xcodegen

# 或者使用 Mint
mint install yonaskolb/XcodeGen
```

## 工作流程

### 1. 添加新文件时

当你创建新的 Swift 文件、资源文件或其他项目文件时：

```bash
# 在 Aether/Sources/ 中创建新文件
touch Aether/Sources/NewFeature.swift

# 运行 XcodeGen 重新生成项目
xcodegen generate

# 打开 Xcode（项目会自动包含新文件）
open Aether.xcodeproj
```

### 2. 修改项目配置时

如果需要修改项目设置（例如添加 Framework、修改 Build Settings 等）：

```bash
# 编辑 project.yml 文件
vim project.yml

# 重新生成项目
xcodegen generate

# 验证更改
open Aether.xcodeproj
```

### 3. 添加新 Target 时

在 `project.yml` 中添加新的 target 配置：

```yaml
targets:
  NewTarget:
    type: framework
    platform: macOS
    sources:
      - path: Aether/NewTarget
```

然后运行：

```bash
xcodegen generate
```

## project.yml 结构说明

### 基本配置

```yaml
name: Aether                  # 项目名称
options:
  bundleIdPrefix: Rootazero   # Bundle ID 前缀
  deploymentTarget:
    macOS: "13.0"             # 最低支持的 macOS 版本
```

### Target 配置

每个 target 包含：

- `type`: 类型（application, framework, bundle.unit-test 等）
- `platform`: 平台（macOS, iOS 等）
- `sources`: 源代码路径
- `resources`: 资源文件路径
- `settings`: 构建设置
- `dependencies`: 依赖项

### 预构建脚本

项目配置了两个预构建脚本：

1. **Build Rust Core**: 自动编译 Rust 核心库
2. **Generate UniFFI Bindings**: 生成 Swift 绑定文件

这些脚本会在 Xcode 构建时自动运行。

## 重要文件说明

### 源代码目录结构

```
Aether/
├── Sources/              # Swift 源文件（自动包含）
│   ├── AetherApp.swift
│   ├── AppDelegate.swift
│   ├── HaloWindow.swift
│   └── Generated/        # UniFFI 生成的绑定
│       └── aether.swift
├── Resources/            # 资源文件
│   └── Info.plist
├── Frameworks/           # 框架和库
│   └── libaethecore.dylib
├── Assets.xcassets/      # 资源目录
├── Info.plist            # 主 Info.plist
└── Aether.entitlements   # 权限配置
```

### Git 版本控制

`.gitignore` 已配置为：
- ✅ **提交**: `project.yml`, `Aether/Sources/`, `Aether/Resources/`
- ❌ **忽略**: `*.xcodeproj`, `DerivedData/`, `xcuserdata/`

## 常见任务

### 重新生成项目（干净构建）

```bash
# 删除旧项目并重新生成
rm -rf Aether.xcodeproj
xcodegen generate
```

### 验证配置文件

```bash
# 检查 project.yml 语法
xcodegen generate --spec project.yml
```

### 查看生成的项目信息

```bash
# 列出所有 targets
xcodebuild -list -project Aether.xcodeproj

# 查看特定 target 的设置
xcodebuild -showBuildSettings -project Aether.xcodeproj -target Aether
```

## 团队协作指南

### 拉取代码后

```bash
# 拉取最新代码
git pull

# 重新生成项目（以防 project.yml 有更新）
xcodegen generate

# 打开项目
open Aether.xcodeproj
```

### 提交代码前

```bash
# 确保 project.yml 是最新的
git add project.yml

# 不要提交 .xcodeproj 文件（已在 .gitignore 中）
git status  # 应该看不到 *.xcodeproj

# 提交更改
git commit -m "Add new feature"
```

## 故障排除

### 问题：文件未出现在 Xcode 中

**解决方案**：
```bash
# 确保文件在正确的目录下
ls Aether/Sources/

# 重新生成项目
xcodegen generate

# 重启 Xcode
```

### 问题：构建设置丢失

**解决方案**：
检查 `project.yml` 中的 `settings` 部分是否正确配置。

### 问题：UniFFI 绑定未生成

**解决方案**：
```bash
# 手动运行绑定生成脚本
cd Aether/core
cargo run --bin uniffi-bindgen generate src/aether.udl \
  --language swift \
  --out-dir ../Sources/Generated/

# 重新生成项目
cd ../..
xcodegen generate
```

## 高级配置

### 添加自定义 Build Settings

在 `project.yml` 的 target settings 中添加：

```yaml
targets:
  Aether:
    settings:
      base:
        CUSTOM_SETTING: value
      configs:
        Debug:
          DEBUG_SETTING: debug_value
        Release:
          DEBUG_SETTING: release_value
```

### 添加依赖 Framework

```yaml
targets:
  Aether:
    dependencies:
      - framework: path/to/Framework.framework
        embed: true
      - sdk: CoreFoundation.framework
```

### 添加脚本阶段

```yaml
targets:
  Aether:
    preBuildScripts:
      - name: "Custom Script"
        script: |
          echo "Running custom script"
        basedOnDependencyAnalysis: false
```

## 参考资源

- [XcodeGen 官方文档](https://github.com/yonaskolb/XcodeGen/blob/master/Docs/ProjectSpec.md)
- [XcodeGen 示例项目](https://github.com/yonaskolb/XcodeGen/tree/master/Docs/Examples)
- [Aether 项目 CLAUDE.md](./CLAUDE.md) - 项目架构说明

## 快速参考

```bash
# 安装 XcodeGen
brew install xcodegen

# 生成项目
xcodegen generate

# 清理并重新生成
rm -rf Aether.xcodeproj && xcodegen generate

# 打开项目
open Aether.xcodeproj

# 验证配置
xcodegen generate --spec project.yml

# 查看帮助
xcodegen --help
```

---

**提示**：每次添加新文件或修改项目配置后，记得运行 `xcodegen generate`！
