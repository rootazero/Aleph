# Liquid Glass 重构设计方案

> 日期：2026-01-25
> 状态：已确认，待实现

## 概述

重构 macOS 对话窗口的 Liquid Glass 实现，达到真正的效果升华。采用全 Metal 渲染方案，实现多层次动效：呼吸动效 + 融合流动 + 全息液态。

## 设计目标

| 维度 | 选择 |
|------|------|
| 流动感 | 全层次：呼吸 + 融合 + 全息 |
| 窗口形态 | 磁流体分离（独立又可融合） |
| 色彩系统 | 自适应（壁纸/强调色动态变化） |
| 性能方案 | Metal Shader 60fps |
| 融合触发 | 距离 + 时间 + 交互 三合一 |

---

## 整体架构

```
┌─────────────────────────────────────────────────────────┐
│                 UnifiedConversationWindow                │
├─────────────────────────────────────────────────────────┤
│  ┌───────────────────────────────────────────────────┐  │
│  │            LiquidGlassMetalView (MTKView)         │  │
│  │  ┌─────────────────────────────────────────────┐  │  │
│  │  │  Layer 1: AuroraBackgroundShader            │  │  │
│  │  │  - Simplex noise 流体场                      │  │  │
│  │  │  - 壁纸色采样 + 系统强调色混合               │  │  │
│  │  │  - 呼吸脉动动画                             │  │  │
│  │  ├─────────────────────────────────────────────┤  │  │
│  │  │  Layer 2: MetaballFusionShader              │  │  │
│  │  │  - 气泡位置/大小数据 → GPU                   │  │  │
│  │  │  - 距离场计算 + 融合阈值                     │  │  │
│  │  │  - 边缘光泽 + 高光反射                       │  │  │
│  │  ├─────────────────────────────────────────────┤  │  │
│  │  │  Layer 3: GlassRefractionShader             │  │  │
│  │  │  - 玻璃折射效果                             │  │  │
│  │  │  - 边缘菲涅尔高光                           │  │  │
│  │  └─────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────┐  │
│  │              SwiftUI Overlay Layer                │  │
│  │  - 文字内容（NSTextView/Text）                    │  │
│  │  - 交互控件（按钮、输入框）                       │  │
│  │  - 向 Metal 层传递气泡位置/交互状态               │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

**核心思路**：Metal 负责所有视觉效果渲染，SwiftUI 只负责文字和交互。两层通过共享数据结构同步（气泡位置、滚动偏移、悬停状态等）。

---

## Shader 设计

### 1. Aurora Background Shader（极光背景）

**效果目标**：窗口底部有缓慢流动的极光，颜色从壁纸和系统强调色中提取，呈现"活着"的感觉。

**色彩采样系统**：
- 壁纸采样：CGWindowListCreateImage → 提取 5 个主色调（K-means）
- 系统强调色：NSColor.controlAccentColor
- 混合权重：强调色 40% + 壁纸色 60%
- 每 5 秒重新采样，平滑过渡

**流体场算法**：
- Simplex Noise 3D (x, y, time)
- Fractal Brownian Motion (4 octaves)
- 生成 0~1 的流场值 → 映射到采样色彩的渐变插值

**呼吸动画**：
- 整体亮度：1.0 + 0.15 * sin(time * 0.5)
- 边缘发光：根据距离窗口边缘衰减
- 周期：约 4 秒一个完整呼吸

**Shader 伪代码**：
```metal
fragment float4 auroraFragment(float2 uv, float time, array<float4, 5> colors) {
    float noise = fbm(float3(uv * 2.0, time * 0.1), 4);
    float4 color = mix(colors[0], colors[1], noise);
    color = mix(color, colors[2], fbm(..., time * 0.15));

    // 呼吸脉动
    float breath = 1.0 + 0.15 * sin(time * 0.5);
    color.rgb *= breath;

    // 边缘柔和衰减
    float edge = smoothstep(0.0, 0.3, min(uv.x, min(uv.y, min(1-uv.x, 1-uv.y))));
    color.a *= edge * 0.6;

    return color;
}
```

### 2. Metaball Fusion Shader（气泡融合）

**效果目标**：消息气泡像磁流体一样，靠近时边缘自然融合，分开时各自完整，带有液态光泽。

**Metaball 原理**：
```
传统矩形气泡           Metaball 融合气泡
┌───────────┐          ╭───────────╮
│  消息 1   │          │  消息 1   │
└───────────┘          ╰─────┬─────╯
┌───────────┐                │ ← 融合区
│  消息 2   │          ╭─────┴─────╮
└───────────┘          │  消息 2   │
                       ╰───────────╯
```

**距离场计算**：
```metal
float metaball(float2 p, float2 center, float2 size, float radius) {
    float2 d = abs(p - center) - size * 0.5 + radius;
    float box = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
    return 1.0 / (1.0 + box * box * 0.01);
}

float totalField = 0.0;
for (int i = 0; i < bubbleCount; i++) {
    totalField += metaball(uv, bubbles[i].center, bubbles[i].size, 12.0);
}
float edge = smoothstep(0.95, 1.05, totalField);
```

**融合触发条件（三合一）**：

| 因子 | 规则 |
|------|------|
| 距离因子 | gap < 20px → 完全融合；20~60px → 渐变融合；> 60px → 独立 |
| 时间因子 | Δt < 5s → 融合权重 +0.3；同一轮对话 → +0.2 |
| 交互因子 | 悬停气泡 → 凸起脱离；滚动中 → 阈值提高（更容易分离） |

**边缘光泽**：
```metal
float fresnel = pow(1.0 - dot(normal, viewDir), 3.0);
float3 gloss = float3(1.0) * fresnel * 0.4;
float topHighlight = smoothstep(0.7, 1.0, uv.y) * 0.2;
```

### 3. Glass Refraction Shader（玻璃折射与高光）

**效果目标**：气泡具有真实玻璃的光学特性——微妙折射背景、边缘菲涅尔高光、内部深度感。

**三层光学效果**：
- Layer 3: 菲涅尔边缘光 - 边缘越薄反射越强
- Layer 2: 折射扭曲 - 背景图像通过玻璃体产生轻微位移
- Layer 1: 内部深度着色 - 中心略暗（厚）→ 边缘略亮（薄）

**Shader 实现**：
```metal
fragment float4 glassFragment(
    float2 uv,
    float sdf,
    texture2d<float> background,
    float2 normal
) {
    // 1. 折射扭曲
    float refractionStrength = 0.02;
    float2 refractedUV = uv + normal * refractionStrength * sdf;
    float4 bgColor = background.sample(sampler, refractedUV);

    // 2. 内部深度着色
    float thickness = smoothstep(0.0, 0.5, sdf);
    float3 tint = mix(float3(0.95), float3(1.0), thickness);

    // 3. 菲涅尔边缘高光
    float edgeDist = 1.0 - smoothstep(0.0, 0.15, sdf);
    float fresnel = pow(edgeDist, 2.0) * 0.6;

    // 4. 顶部渐变高光
    float topLight = smoothstep(0.5, 1.0, uv.y) * 0.15;

    // 合成
    float4 result = bgColor;
    result.rgb *= tint;
    result.rgb += fresnel + topLight;
    result.a = 0.85;

    return result;
}
```

**动态高光响应**：

| 交互状态 | 高光响应 |
|---------|---------|
| 默认 | 顶部柔和高光带 |
| 悬停 | 边缘高光增强 1.5x，轻微"鼓起" |
| 点击 | 涟漪扩散效果（从点击处向外） |
| 输入框聚焦 | 边缘呼吸光环（青色强调） |
| AI 思考中 | 内部光流缓慢旋转 |

---

## Swift ↔ Metal 数据同步

### 数据结构

```swift
// 传递给 Metal 的统一缓冲区
struct LiquidGlassUniforms {
    var time: Float
    var scrollOffset: Float
    var mousePosition: SIMD2<Float>
    var hoveredBubbleIndex: Int32
    var inputFocused: Bool
    var accentColor: SIMD4<Float>
    var dominantColors: (SIMD4<Float>, SIMD4<Float>, SIMD4<Float>, SIMD4<Float>, SIMD4<Float>)
}

// 气泡数据数组
struct BubbleData {
    var center: SIMD2<Float>
    var size: SIMD2<Float>
    var cornerRadius: Float
    var isUser: Bool
    var timestamp: Float
    var fusionWeight: Float
}
```

### 同步流程

```
SwiftUI Layer                    Metal Layer
─────────────                    ───────────
     │                                │
     │ ① GeometryReader              │
     │    获取气泡 frame              │
     ▼                                │
┌─────────────┐                       │
│ ViewModel   │ ② 坐标转换             │
│ .bubbles[] │    (SwiftUI→Metal)    │
└──────┬──────┘                       │
       │                              │
       │ ③ 写入 MTLBuffer            │
       │    (每帧或变化时)            │
       ▼                              ▼
┌─────────────────────────────────────────┐
│         Shared MTLBuffer               │
└─────────────────────────────────────────┘
                    │
                    │ ④ GPU 读取
                    ▼
             ┌─────────────┐
             │  Render     │
             │  Pipeline   │
             └─────────────┘
```

### 性能优化策略

| 策略 | 实现方式 |
|------|---------|
| Triple Buffering | 3 个 MTLBuffer 轮换，避免 CPU/GPU 竞争 |
| 脏标记更新 | 只在气泡变化时更新数组，滚动时只更新 offset |
| 预计算融合权重 | Swift 侧计算好 fusionWeight，减少 GPU 分支判断 |
| LOD 降级 | > 20 个气泡时简化融合计算，仅计算相邻气泡的融合 |

---

## 壁纸色采样系统

### 采样流程

1. 获取窗口下方的屏幕区域截图：`CGWindowListCreateImage`
2. 降采样到 32x32 减少计算量：`CIFilter.lanczosScaleTransform`
3. K-means 聚类提取 5 个主色（迭代次数: 10）
4. 按"活力度"排序（饱和度 × 亮度）
5. 与系统强调色混合：`colors[0] = accentColor * 0.4 + dominant[0] * 0.6`

### 采样触发条件

- windowMoved - 窗口移动后
- windowResized - 窗口大小改变
- spaceChanged - 切换桌面空间
- periodic - 每 5 秒定时
- wallpaperChanged - 系统壁纸改变通知

### 系统事件监听

```swift
// 壁纸变化
DistributedNotificationCenter.default().addObserver(
    forName: NSNotification.Name("com.apple.desktop.background.changed"), ...
)

// 强调色变化
NSApp.publisher(for: \.effectiveAppearance)

// 窗口移动
NotificationCenter.default.addObserver(
    forName: NSWindow.didMoveNotification, ...
)
```

### 降级策略

| 场景 | 降级方案 |
|------|---------|
| 截图权限被拒绝 | 使用系统强调色 + 预设渐变 |
| K-means 失败 | 使用上次成功的颜色 |
| 颜色过于单调 | 注入 20% 强调色增加活力 |
| 颜色对比度不足 | 自动调整亮度范围 |

---

## 交互响应与动画参数

### 交互状态机

```
                 ┌─────────┐
                 │  Idle   │
                 └────┬────┘
                      │
       ┌──────────────┼──────────────┐
       ▼              ▼              ▼
 ┌──────────┐  ┌───────────┐  ┌───────────┐
 │ Hovering │  │ Scrolling │  │ Pressing  │
 └────┬─────┘  └─────┬─────┘  └─────┬─────┘
      │              │              │
      │              │              ▼
      │              │        ┌───────────┐
      │              │        │  Ripple   │
      │              │        │  Effect   │
      │              │        └───────────┘
      │              │
      └──────────────┴──────────────┘
                     │
                     ▼
               ┌──────────┐
               │ Settling │ ← 惯性衰减
               └──────────┘
```

### 动画参数表

| 效果 | 参数 | 值 |
|------|------|-----|
| 极光流动 | 速度 | 0.1 (time scale) |
| | 振幅 | 2.0 (uv scale) |
| | octaves | 4 |
| 呼吸脉动 | 周期 | 4 秒 |
| | 亮度变化范围 | ±15% |
| | 边缘发光范围 | ±10% |
| 悬停凸起 | 上升高度 | 4px (视觉) |
| | 阴影增强 | 1.5x |
| | 过渡时间 | 0.2s (ease-out) |
| 融合/分离 | 融合阈值 | 20px 开始 |
| | 完全融合 | 8px 以内 |
| | 过渡曲线 | ease-in-out |
| | 时间因子衰减 | 5s → 0 权重 |
| 点击涟漪 | 扩散速度 | 200px/s |
| | 最大半径 | 气泡对角线长度 |
| | 透明度衰减 | 0.5s → 0 |
| 输入框聚焦光环 | 光环宽度 | 3px |
| | 脉动周期 | 2s |
| | 颜色 | accentColor |
| AI 思考光流 | 旋转速度 | 0.3 rad/s |
| | 光带数量 | 3 |
| | 不透明度 | 0.3 |

### 滚动物理

```swift
struct ScrollPhysics {
    var velocity: CGFloat = 0

    // 滚动越快，融合阈值越高（越难融合）
    var fusionThresholdMultiplier: Float {
        let speed = abs(velocity)
        return 1.0 + Float(min(speed / 500, 1.0)) * 0.5
    }

    // 快速滚动时气泡间距视觉上增大
    var bubbleSpacing: Float {
        return 1.0 + Float(min(abs(velocity) / 1000, 0.3))
    }
}
```

---

## 文件结构

```
platforms/macos/Aleph/Sources/
├── UI/
│   ├── LiquidGlass/                          # 新增：液态玻璃系统
│   │   ├── Metal/
│   │   │   ├── Shaders/
│   │   │   │   ├── LiquidGlassShaders.metal  # 主 shader 文件
│   │   │   │   ├── AuroraBackground.metal    # 极光背景
│   │   │   │   ├── MetaballFusion.metal      # 气泡融合
│   │   │   │   ├── GlassRefraction.metal     # 玻璃折射
│   │   │   │   └── NoiseUtils.metal          # Simplex noise 工具
│   │   │   │
│   │   │   ├── LiquidGlassRenderer.swift     # Metal 渲染器
│   │   │   ├── LiquidGlassMetalView.swift    # MTKView 封装
│   │   │   └── ShaderTypes.h                 # Swift/Metal 共享类型
│   │   │
│   │   ├── ColorSampling/
│   │   │   ├── WallpaperColorSampler.swift   # 壁纸色采样
│   │   │   ├── DominantColorExtractor.swift  # K-means 主色提取
│   │   │   └── ColorTransitionManager.swift  # 颜色平滑过渡
│   │   │
│   │   ├── Physics/
│   │   │   ├── BubbleFusionCalculator.swift  # 融合权重计算
│   │   │   ├── InteractionPhysics.swift      # 交互物理响应
│   │   │   └── ScrollPhysics.swift           # 滚动惯性处理
│   │   │
│   │   └── LiquidGlassConfiguration.swift    # 配置参数
│   │
│   ├── Conversation/
│   │   ├── UnifiedConversationWindow.swift   # 修改：集成 Metal 层
│   │   ├── UnifiedConversationView.swift     # 修改：移除旧背景
│   │   ├── LiquidGlassConversationView.swift # 新增：顶层容器
│   │   │
│   │   ├── Bubbles/
│   │   │   ├── MessageBubbleView.swift       # 修改：透明背景
│   │   │   ├── BubbleGeometryReporter.swift  # 新增：上报几何信息
│   │   │   └── BubbleInteractionHandler.swift# 新增：交互事件
│   │   │
│   │   └── Input/
│   │       ├── InputAreaView.swift           # 修改：透明背景
│   │       └── InputGlowEffect.swift         # 新增：聚焦光环
```

---

## 实现计划

### Phase 1: Metal 基础设施
- [1.1] ShaderTypes.h 定义共享数据结构
- [1.2] LiquidGlassRenderer 骨架
- [1.3] LiquidGlassMetalView (MTKView 封装)
- [1.4] 集成到 UnifiedConversationWindow
- **验收**：窗口显示纯色 Metal 渲染

### Phase 2: 极光背景
- [2.1] NoiseUtils.metal (Simplex noise + FBM)
- [2.2] AuroraBackground.metal 基础版
- [2.3] 呼吸动画
- [2.4] WallpaperColorSampler + 颜色注入
- **验收**：窗口显示动态极光，颜色跟随壁纸

### Phase 3: 气泡融合
- [3.1] BubbleGeometryReporter (SwiftUI → Metal)
- [3.2] MetaballFusion.metal 基础 SDF
- [3.3] 融合权重计算 (距离 + 时间)
- [3.4] 边缘光泽渲染
- [3.5] 移除旧气泡背景，验证透明叠加
- **验收**：气泡靠近时边缘自然融合

### Phase 4: 玻璃光学
- [4.1] GlassRefraction.metal
- [4.2] 菲涅尔边缘高光
- [4.3] 顶部光带
- **验收**：气泡具有真实玻璃质感

### Phase 5: 交互响应
- [5.1] 悬停检测 + 凸起效果
- [5.2] 点击涟漪
- [5.3] 输入框聚焦光环
- [5.4] 滚动物理 (融合阈值动态调整)
- [5.5] AI 思考时的光流效果
- **验收**：所有交互状态都有流畅响应

### Phase 6: 优化与降级
- [6.1] Triple buffering 实现
- [6.2] LOD 降级 (气泡过多时)
- [6.3] 低电量模式降级
- [6.4] 辅助功能适配 (减少动态效果)
- [6.5] 性能 profiling + 调优
- **验收**：各场景下稳定 60fps

---

## 风险与应对

| 风险 | 应对策略 |
|------|---------|
| Metal shader 编译错误难调试 | 准备 Core Animation 降级方案；使用 Xcode GPU Frame Capture；逐步添加功能，每步验证 |
| SwiftUI 坐标系与 Metal 不匹配 | 封装统一的坐标转换工具；在 Reporter 层做好归一化 |
| 文字清晰度下降 | 文字层完全独立于 Metal；必要时增加文字背景微弱遮罩 |
| 壁纸采样权限 | 优雅降级到预设色板；首次使用时请求权限 |
| 老旧 Mac 性能 | 检测 GPU 能力，自动降级；M1 以下禁用部分效果 |
