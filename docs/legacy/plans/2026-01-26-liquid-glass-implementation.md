# Liquid Glass 重构实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 将 macOS 对话窗口重构为全 Metal 渲染的 Liquid Glass 效果，实现极光背景、气泡融合、玻璃折射三层动效。

**Architecture:** Metal 渲染管线作为底层画布（MTKView），SwiftUI 作为覆盖层处理文字和交互。三个 Shader 分层渲染：Aurora Background → Metaball Fusion → Glass Refraction。Swift 层通过共享 MTLBuffer 向 GPU 传递气泡位置和交互状态。

**Tech Stack:** Metal Shading Language, MetalKit (MTKView), SwiftUI, Core Graphics (壁纸采样), Accelerate (K-means)

---

## 文件路径约定

```
platforms/macos/Aether/Sources/
├── LiquidGlass/                              # 新建目录
│   ├── Metal/
│   │   ├── Shaders/
│   │   │   ├── LiquidGlassShaders.metal
│   │   │   └── ShaderTypes.h
│   │   ├── LiquidGlassRenderer.swift
│   │   └── LiquidGlassMetalView.swift
│   ├── ColorSampling/
│   │   ├── WallpaperColorSampler.swift
│   │   └── DominantColorExtractor.swift
│   ├── Physics/
│   │   └── BubbleFusionCalculator.swift
│   └── LiquidGlassConfiguration.swift
├── MultiTurn/
│   ├── UnifiedConversationWindow.swift       # 修改
│   ├── UnifiedConversationView.swift         # 修改
│   └── Views/
│       ├── MessageBubbleView.swift           # 修改
│       ├── InputAreaView.swift               # 修改
│       └── BubbleGeometryReporter.swift      # 新建
```

---

## Task 1: 创建 Shader 类型定义文件

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/Metal/Shaders/ShaderTypes.h`

**Step 1: 创建目录结构**

Run: `mkdir -p platforms/macos/Aether/Sources/LiquidGlass/Metal/Shaders platforms/macos/Aether/Sources/LiquidGlass/ColorSampling platforms/macos/Aether/Sources/LiquidGlass/Physics`
Expected: Success, no output

**Step 2: 创建 ShaderTypes.h**

```c
//
//  ShaderTypes.h
//  Aleph
//
//  Shared types between Swift and Metal shaders for Liquid Glass rendering.
//

#ifndef ShaderTypes_h
#define ShaderTypes_h

#include <simd/simd.h>

// MARK: - Vertex Data

typedef struct {
    vector_float2 position;
    vector_float2 texCoord;
} LiquidGlassVertex;

// MARK: - Uniforms

typedef struct {
    float time;
    float scrollOffset;
    vector_float2 mousePosition;
    vector_float2 viewportSize;
    int32_t hoveredBubbleIndex;
    int32_t bubbleCount;
    bool inputFocused;
    float breathPhase;
    float scrollVelocity;

    // Colors (from wallpaper sampling)
    vector_float4 accentColor;
    vector_float4 dominantColors[5];
} LiquidGlassUniforms;

// MARK: - Bubble Data

typedef struct {
    vector_float2 center;
    vector_float2 size;
    float cornerRadius;
    float fusionWeight;
    float timestamp;
    bool isUser;
    bool isHovered;
    bool isPressed;
} BubbleData;

// MARK: - Buffer Indices

typedef enum {
    BufferIndexVertices = 0,
    BufferIndexUniforms = 1,
    BufferIndexBubbles = 2,
} BufferIndex;

// MARK: - Texture Indices

typedef enum {
    TextureIndexBackground = 0,
    TextureIndexNoise = 1,
} TextureIndex;

#endif /* ShaderTypes_h */
```

**Step 3: 验证文件创建**

Run: `cat platforms/macos/Aether/Sources/LiquidGlass/Metal/Shaders/ShaderTypes.h | head -20`
Expected: Shows header content

**Step 4: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/
git commit -m "feat(liquid-glass): add shader types header for Metal-Swift interop"
```

---

## Task 2: 创建 Metal Shader 文件

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/Metal/Shaders/LiquidGlassShaders.metal`

**Step 1: 创建 Metal Shader 文件**

```metal
//
//  LiquidGlassShaders.metal
//  Aleph
//
//  Metal shaders for Liquid Glass effect:
//  - Aurora background with flowing colors
//  - Metaball fusion for bubble merging
//  - Glass refraction and fresnel highlights
//

#include <metal_stdlib>
#include "ShaderTypes.h"

using namespace metal;

// MARK: - Constants

constant float PI = 3.14159265359;
constant float FUSION_THRESHOLD = 1.0;
constant float FUSION_SOFTNESS = 0.1;

// MARK: - Noise Functions

// Simplex noise permutation
constant int perm[512] = {
    151,160,137,91,90,15,131,13,201,95,96,53,194,233,7,225,140,36,103,30,69,142,
    8,99,37,240,21,10,23,190,6,148,247,120,234,75,0,26,197,62,94,252,219,203,117,
    35,11,32,57,177,33,88,237,149,56,87,174,20,125,136,171,168,68,175,74,165,71,
    134,139,48,27,166,77,146,158,231,83,111,229,122,60,211,133,230,220,105,92,41,
    55,46,245,40,244,102,143,54,65,25,63,161,1,216,80,73,209,76,132,187,208,89,
    18,169,200,196,135,130,116,188,159,86,164,100,109,198,173,186,3,64,52,217,226,
    250,124,123,5,202,38,147,118,126,255,82,85,212,207,206,59,227,47,16,58,17,182,
    189,28,42,223,183,170,213,119,248,152,2,44,154,163,70,221,153,101,155,167,43,
    172,9,129,22,39,253,19,98,108,110,79,113,224,232,178,185,112,104,218,246,97,
    228,251,34,242,193,238,210,144,12,191,179,162,241,81,51,145,235,249,14,239,
    107,49,192,214,31,181,199,106,157,184,84,204,176,115,121,50,45,127,4,150,254,
    138,236,205,93,222,114,67,29,24,72,243,141,128,195,78,66,215,61,156,180,
    151,160,137,91,90,15,131,13,201,95,96,53,194,233,7,225,140,36,103,30,69,142,
    8,99,37,240,21,10,23,190,6,148,247,120,234,75,0,26,197,62,94,252,219,203,117,
    35,11,32,57,177,33,88,237,149,56,87,174,20,125,136,171,168,68,175,74,165,71,
    134,139,48,27,166,77,146,158,231,83,111,229,122,60,211,133,230,220,105,92,41,
    55,46,245,40,244,102,143,54,65,25,63,161,1,216,80,73,209,76,132,187,208,89,
    18,169,200,196,135,130,116,188,159,86,164,100,109,198,173,186,3,64,52,217,226,
    250,124,123,5,202,38,147,118,126,255,82,85,212,207,206,59,227,47,16,58,17,182,
    189,28,42,223,183,170,213,119,248,152,2,44,154,163,70,221,153,101,155,167,43,
    172,9,129,22,39,253,19,98,108,110,79,113,224,232,178,185,112,104,218,246,97,
    228,251,34,242,193,238,210,144,12,191,179,162,241,81,51,145,235,249,14,239,
    107,49,192,214,31,181,199,106,157,184,84,204,176,115,121,50,45,127,4,150,254,
    138,236,205,93,222,114,67,29,24,72,243,141,128,195,78,66,215,61,156,180
};

float grad(int hash, float x, float y, float z) {
    int h = hash & 15;
    float u = h < 8 ? x : y;
    float v = h < 4 ? y : (h == 12 || h == 14 ? x : z);
    return ((h & 1) == 0 ? u : -u) + ((h & 2) == 0 ? v : -v);
}

float noise3D(float3 p) {
    int X = int(floor(p.x)) & 255;
    int Y = int(floor(p.y)) & 255;
    int Z = int(floor(p.z)) & 255;

    float x = p.x - floor(p.x);
    float y = p.y - floor(p.y);
    float z = p.z - floor(p.z);

    float u = x * x * x * (x * (x * 6 - 15) + 10);
    float v = y * y * y * (y * (y * 6 - 15) + 10);
    float w = z * z * z * (z * (z * 6 - 15) + 10);

    int A = perm[X] + Y;
    int AA = perm[A] + Z;
    int AB = perm[A + 1] + Z;
    int B = perm[X + 1] + Y;
    int BA = perm[B] + Z;
    int BB = perm[B + 1] + Z;

    return mix(
        mix(mix(grad(perm[AA], x, y, z), grad(perm[BA], x - 1, y, z), u),
            mix(grad(perm[AB], x, y - 1, z), grad(perm[BB], x - 1, y - 1, z), u), v),
        mix(mix(grad(perm[AA + 1], x, y, z - 1), grad(perm[BA + 1], x - 1, y, z - 1), u),
            mix(grad(perm[AB + 1], x, y - 1, z - 1), grad(perm[BB + 1], x - 1, y - 1, z - 1), u), v),
        w
    );
}

// Fractal Brownian Motion
float fbm(float3 p, int octaves) {
    float value = 0.0;
    float amplitude = 0.5;
    float frequency = 1.0;

    for (int i = 0; i < octaves; i++) {
        value += amplitude * noise3D(p * frequency);
        amplitude *= 0.5;
        frequency *= 2.0;
    }

    return value;
}

// MARK: - SDF Functions

// Rounded rectangle SDF
float roundedRectSDF(float2 p, float2 center, float2 size, float radius) {
    float2 d = abs(p - center) - size * 0.5 + radius;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
}

// Metaball potential field
float metaballPotential(float2 p, float2 center, float2 size, float radius, float weight) {
    float dist = roundedRectSDF(p, center, size, radius);
    // Smooth falloff using inverse square
    float potential = weight / (1.0 + dist * dist * 0.01);
    return potential;
}

// MARK: - Vertex Shader

struct VertexOut {
    float4 position [[position]];
    float2 texCoord;
};

vertex VertexOut liquidGlassVertex(
    uint vertexID [[vertex_id]],
    constant LiquidGlassVertex *vertices [[buffer(BufferIndexVertices)]]
) {
    VertexOut out;
    out.position = float4(vertices[vertexID].position, 0.0, 1.0);
    out.texCoord = vertices[vertexID].texCoord;
    return out;
}

// MARK: - Aurora Background Fragment Shader

fragment float4 auroraBackgroundFragment(
    VertexOut in [[stage_in]],
    constant LiquidGlassUniforms &uniforms [[buffer(BufferIndexUniforms)]]
) {
    float2 uv = in.texCoord;
    float time = uniforms.time;

    // Generate flowing noise field
    float3 noiseCoord = float3(uv * 2.0, time * 0.1);
    float noise1 = fbm(noiseCoord, 4) * 0.5 + 0.5;
    float noise2 = fbm(noiseCoord + float3(5.2, 1.3, 2.8), 4) * 0.5 + 0.5;
    float noise3 = fbm(noiseCoord * 0.5 + float3(time * 0.05, 0, 0), 3) * 0.5 + 0.5;

    // Mix dominant colors based on noise
    float4 color1 = uniforms.dominantColors[0];
    float4 color2 = uniforms.dominantColors[1];
    float4 color3 = uniforms.dominantColors[2];
    float4 accentColor = uniforms.accentColor;

    // Blend colors
    float4 baseColor = mix(color1, color2, noise1);
    baseColor = mix(baseColor, color3, noise2 * 0.5);
    baseColor = mix(baseColor, accentColor, noise3 * 0.4);

    // Breathing animation
    float breath = 1.0 + 0.15 * sin(uniforms.breathPhase);
    baseColor.rgb *= breath;

    // Edge fade for soft borders
    float edgeFade = smoothstep(0.0, 0.15, min(uv.x, min(uv.y, min(1.0 - uv.x, 1.0 - uv.y))));

    // Apply alpha with edge fade
    baseColor.a = 0.4 * edgeFade;

    return baseColor;
}

// MARK: - Metaball Fusion Fragment Shader

fragment float4 metaballFusionFragment(
    VertexOut in [[stage_in]],
    constant LiquidGlassUniforms &uniforms [[buffer(BufferIndexUniforms)]],
    constant BubbleData *bubbles [[buffer(BufferIndexBubbles)]],
    texture2d<float> backgroundTexture [[texture(TextureIndexBackground)]]
) {
    constexpr sampler textureSampler(mag_filter::linear, min_filter::linear);

    float2 uv = in.texCoord;
    float2 pixelPos = uv * uniforms.viewportSize;

    // Calculate total metaball potential field
    float totalPotential = 0.0;
    float4 bubbleColor = float4(0.0);
    float colorWeight = 0.0;

    for (int i = 0; i < uniforms.bubbleCount && i < 50; i++) {
        BubbleData bubble = bubbles[i];

        // Calculate potential from this bubble
        float potential = metaballPotential(
            pixelPos,
            bubble.center,
            bubble.size,
            bubble.cornerRadius,
            bubble.fusionWeight
        );

        totalPotential += potential;

        // Accumulate color contribution
        float4 bColor = bubble.isUser ? float4(0.3, 0.6, 1.0, 1.0) : float4(0.9, 0.9, 0.95, 1.0);
        bubbleColor += bColor * potential;
        colorWeight += potential;
    }

    // Normalize color
    if (colorWeight > 0.001) {
        bubbleColor /= colorWeight;
    }

    // Sample background
    float4 bgColor = backgroundTexture.sample(textureSampler, uv);

    // Determine if inside metaball region
    float edge = smoothstep(FUSION_THRESHOLD - FUSION_SOFTNESS, FUSION_THRESHOLD + FUSION_SOFTNESS, totalPotential);

    // Calculate surface normal for fresnel (approximate)
    float dx = 0.001;
    float potentialRight = 0.0;
    float potentialUp = 0.0;

    for (int i = 0; i < uniforms.bubbleCount && i < 50; i++) {
        BubbleData bubble = bubbles[i];
        potentialRight += metaballPotential(pixelPos + float2(dx, 0) * uniforms.viewportSize.x, bubble.center, bubble.size, bubble.cornerRadius, bubble.fusionWeight);
        potentialUp += metaballPotential(pixelPos + float2(0, dx) * uniforms.viewportSize.y, bubble.center, bubble.size, bubble.cornerRadius, bubble.fusionWeight);
    }

    float2 gradient = float2(potentialRight - totalPotential, potentialUp - totalPotential);
    float gradientLength = length(gradient);

    // Fresnel effect at edges
    float fresnel = 0.0;
    if (gradientLength > 0.0001) {
        fresnel = pow(1.0 - smoothstep(0.0, 0.3, edge), 2.0) * 0.6;
    }

    // Top highlight
    float topHighlight = smoothstep(0.3, 1.0, uv.y) * edge * 0.15;

    // Glass refraction (subtle UV distortion)
    float2 refractedUV = uv;
    if (gradientLength > 0.0001 && edge > 0.01) {
        float2 normal = normalize(gradient);
        refractedUV += normal * 0.02 * edge;
    }
    float4 refractedBg = backgroundTexture.sample(textureSampler, refractedUV);

    // Composite
    float4 glassColor = mix(refractedBg, bubbleColor, 0.1);
    glassColor.rgb += fresnel + topHighlight;
    glassColor.a = edge * 0.85;

    // Blend with background where no bubbles
    float4 result = mix(bgColor, glassColor, edge);

    return result;
}

// MARK: - Final Composite Fragment Shader

fragment float4 liquidGlassCompositeFragment(
    VertexOut in [[stage_in]],
    constant LiquidGlassUniforms &uniforms [[buffer(BufferIndexUniforms)]],
    texture2d<float> auroraTexture [[texture(0)]],
    texture2d<float> bubbleTexture [[texture(1)]]
) {
    constexpr sampler textureSampler(mag_filter::linear, min_filter::linear);

    float2 uv = in.texCoord;

    // Sample both layers
    float4 aurora = auroraTexture.sample(textureSampler, uv);
    float4 bubbles = bubbleTexture.sample(textureSampler, uv);

    // Composite: aurora background + bubble layer on top
    float4 result = aurora;
    result = mix(result, bubbles, bubbles.a);

    return result;
}
```

**Step 2: 验证 shader 文件**

Run: `wc -l platforms/macos/Aether/Sources/LiquidGlass/Metal/Shaders/LiquidGlassShaders.metal`
Expected: ~280 lines

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/Metal/Shaders/LiquidGlassShaders.metal
git commit -m "feat(liquid-glass): add Metal shaders for aurora, metaball fusion, and glass effects"
```

---

## Task 3: 创建 Metal Renderer

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/Metal/LiquidGlassRenderer.swift`

**Step 1: 创建 Renderer 文件**

```swift
//
//  LiquidGlassRenderer.swift
//  Aleph
//
//  Metal renderer for Liquid Glass effects.
//  Manages render pipeline, buffers, and frame updates.
//

import Foundation
import MetalKit
import simd

// MARK: - LiquidGlassRenderer

final class LiquidGlassRenderer: NSObject {

    // MARK: - Metal Objects

    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue

    // Render pipelines
    private var auroraPipeline: MTLRenderPipelineState?
    private var metaballPipeline: MTLRenderPipelineState?
    private var compositePipeline: MTLRenderPipelineState?

    // Buffers
    private var vertexBuffer: MTLBuffer?
    private var uniformBuffers: [MTLBuffer] = []
    private var bubbleBuffer: MTLBuffer?
    private var currentBufferIndex = 0
    private let maxBuffersInFlight = 3

    // Textures
    private var auroraTexture: MTLTexture?
    private var bubbleTexture: MTLTexture?

    // State
    private var viewportSize: SIMD2<Float> = .zero
    private var startTime: CFAbsoluteTime = CFAbsoluteTimeGetCurrent()

    // Data
    private var uniforms = LiquidGlassUniforms()
    private var bubbles: [BubbleData] = []

    // Semaphore for triple buffering
    private let semaphore = DispatchSemaphore(value: 3)

    // MARK: - Initialization

    init?(device: MTLDevice) {
        self.device = device

        guard let queue = device.makeCommandQueue() else {
            return nil
        }
        self.commandQueue = queue

        super.init()

        setupPipelines()
        setupBuffers()
        initializeUniforms()
    }

    // MARK: - Setup

    private func setupPipelines() {
        guard let library = device.makeDefaultLibrary() else {
            print("[LiquidGlassRenderer] Failed to create default library")
            return
        }

        let vertexFunction = library.makeFunction(name: "liquidGlassVertex")
        let auroraFragment = library.makeFunction(name: "auroraBackgroundFragment")
        let metaballFragment = library.makeFunction(name: "metaballFusionFragment")
        let compositeFragment = library.makeFunction(name: "liquidGlassCompositeFragment")

        // Common pipeline descriptor setup
        let pipelineDescriptor = MTLRenderPipelineDescriptor()
        pipelineDescriptor.vertexFunction = vertexFunction
        pipelineDescriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipelineDescriptor.colorAttachments[0].isBlendingEnabled = true
        pipelineDescriptor.colorAttachments[0].sourceRGBBlendFactor = .sourceAlpha
        pipelineDescriptor.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
        pipelineDescriptor.colorAttachments[0].sourceAlphaBlendFactor = .one
        pipelineDescriptor.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha

        do {
            // Aurora pipeline
            pipelineDescriptor.fragmentFunction = auroraFragment
            auroraPipeline = try device.makeRenderPipelineState(descriptor: pipelineDescriptor)

            // Metaball pipeline
            pipelineDescriptor.fragmentFunction = metaballFragment
            metaballPipeline = try device.makeRenderPipelineState(descriptor: pipelineDescriptor)

            // Composite pipeline
            pipelineDescriptor.fragmentFunction = compositeFragment
            compositePipeline = try device.makeRenderPipelineState(descriptor: pipelineDescriptor)
        } catch {
            print("[LiquidGlassRenderer] Pipeline creation failed: \(error)")
        }
    }

    private func setupBuffers() {
        // Full-screen quad vertices
        let vertices: [LiquidGlassVertex] = [
            LiquidGlassVertex(position: SIMD2<Float>(-1, -1), texCoord: SIMD2<Float>(0, 1)),
            LiquidGlassVertex(position: SIMD2<Float>(1, -1), texCoord: SIMD2<Float>(1, 1)),
            LiquidGlassVertex(position: SIMD2<Float>(-1, 1), texCoord: SIMD2<Float>(0, 0)),
            LiquidGlassVertex(position: SIMD2<Float>(1, 1), texCoord: SIMD2<Float>(1, 0)),
        ]

        vertexBuffer = device.makeBuffer(
            bytes: vertices,
            length: MemoryLayout<LiquidGlassVertex>.stride * vertices.count,
            options: .storageModeShared
        )

        // Triple buffering for uniforms
        for _ in 0..<maxBuffersInFlight {
            if let buffer = device.makeBuffer(
                length: MemoryLayout<LiquidGlassUniforms>.size,
                options: .storageModeShared
            ) {
                uniformBuffers.append(buffer)
            }
        }

        // Bubble buffer (max 50 bubbles)
        bubbleBuffer = device.makeBuffer(
            length: MemoryLayout<BubbleData>.stride * 50,
            options: .storageModeShared
        )
    }

    private func initializeUniforms() {
        // Default colors (will be updated by wallpaper sampler)
        uniforms.accentColor = SIMD4<Float>(0.0, 0.478, 1.0, 1.0) // System blue
        uniforms.dominantColors.0 = SIMD4<Float>(0.2, 0.4, 0.6, 1.0)
        uniforms.dominantColors.1 = SIMD4<Float>(0.4, 0.3, 0.5, 1.0)
        uniforms.dominantColors.2 = SIMD4<Float>(0.3, 0.5, 0.4, 1.0)
        uniforms.dominantColors.3 = SIMD4<Float>(0.5, 0.4, 0.6, 1.0)
        uniforms.dominantColors.4 = SIMD4<Float>(0.4, 0.5, 0.5, 1.0)
    }

    // MARK: - Texture Management

    private func ensureTextures(width: Int, height: Int) {
        guard width > 0 && height > 0 else { return }

        // Check if textures need recreation
        if let aurora = auroraTexture,
           aurora.width == width && aurora.height == height {
            return
        }

        let textureDescriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm,
            width: width,
            height: height,
            mipmapped: false
        )
        textureDescriptor.usage = [.renderTarget, .shaderRead]
        textureDescriptor.storageMode = .private

        auroraTexture = device.makeTexture(descriptor: textureDescriptor)
        bubbleTexture = device.makeTexture(descriptor: textureDescriptor)
    }

    // MARK: - Update Methods

    func updateViewportSize(_ size: CGSize) {
        viewportSize = SIMD2<Float>(Float(size.width), Float(size.height))
        uniforms.viewportSize = viewportSize
        ensureTextures(width: Int(size.width), height: Int(size.height))
    }

    func updateBubbles(_ newBubbles: [BubbleData]) {
        bubbles = newBubbles
        uniforms.bubbleCount = Int32(min(bubbles.count, 50))

        guard let buffer = bubbleBuffer, !bubbles.isEmpty else { return }

        let bufferPointer = buffer.contents().bindMemory(to: BubbleData.self, capacity: 50)
        for (index, bubble) in bubbles.prefix(50).enumerated() {
            bufferPointer[index] = bubble
        }
    }

    func updateColors(accent: SIMD4<Float>, dominant: [SIMD4<Float>]) {
        uniforms.accentColor = accent
        if dominant.count >= 1 { uniforms.dominantColors.0 = dominant[0] }
        if dominant.count >= 2 { uniforms.dominantColors.1 = dominant[1] }
        if dominant.count >= 3 { uniforms.dominantColors.2 = dominant[2] }
        if dominant.count >= 4 { uniforms.dominantColors.3 = dominant[3] }
        if dominant.count >= 5 { uniforms.dominantColors.4 = dominant[4] }
    }

    func updateInteraction(mousePosition: SIMD2<Float>, hoveredIndex: Int, inputFocused: Bool, scrollVelocity: Float) {
        uniforms.mousePosition = mousePosition
        uniforms.hoveredBubbleIndex = Int32(hoveredIndex)
        uniforms.inputFocused = inputFocused
        uniforms.scrollVelocity = scrollVelocity
    }

    func updateScrollOffset(_ offset: Float) {
        uniforms.scrollOffset = offset
    }
}

// MARK: - MTKViewDelegate

extension LiquidGlassRenderer: MTKViewDelegate {

    func mtkView(_ view: MTKView, drawableSizeDidChange size: CGSize) {
        updateViewportSize(size)
    }

    func draw(in view: MTKView) {
        // Wait for buffer availability
        _ = semaphore.wait(timeout: .distantFuture)

        // Update time-based uniforms
        let currentTime = CFAbsoluteTimeGetCurrent()
        uniforms.time = Float(currentTime - startTime)
        uniforms.breathPhase = Float(currentTime - startTime) * 0.5

        // Get current uniform buffer
        currentBufferIndex = (currentBufferIndex + 1) % maxBuffersInFlight
        let uniformBuffer = uniformBuffers[currentBufferIndex]

        // Copy uniforms to buffer
        memcpy(uniformBuffer.contents(), &uniforms, MemoryLayout<LiquidGlassUniforms>.size)

        guard let drawable = view.currentDrawable,
              let commandBuffer = commandQueue.makeCommandBuffer() else {
            semaphore.signal()
            return
        }

        commandBuffer.addCompletedHandler { [weak self] _ in
            self?.semaphore.signal()
        }

        // Render aurora to texture
        if let auroraTexture = auroraTexture,
           let pipeline = auroraPipeline {
            renderToTexture(
                commandBuffer: commandBuffer,
                texture: auroraTexture,
                pipeline: pipeline,
                uniformBuffer: uniformBuffer
            )
        }

        // Render metaballs to texture
        if let bubbleTexture = bubbleTexture,
           let auroraTexture = auroraTexture,
           let pipeline = metaballPipeline {
            renderMetaballs(
                commandBuffer: commandBuffer,
                outputTexture: bubbleTexture,
                backgroundTexture: auroraTexture,
                pipeline: pipeline,
                uniformBuffer: uniformBuffer
            )
        }

        // Final composite to screen
        if let renderPassDescriptor = view.currentRenderPassDescriptor,
           let pipeline = compositePipeline,
           let auroraTexture = auroraTexture,
           let bubbleTexture = bubbleTexture {

            renderPassDescriptor.colorAttachments[0].loadAction = .clear
            renderPassDescriptor.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)

            guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPassDescriptor) else {
                commandBuffer.commit()
                return
            }

            encoder.setRenderPipelineState(pipeline)
            encoder.setVertexBuffer(vertexBuffer, offset: 0, index: 0)
            encoder.setFragmentBuffer(uniformBuffer, offset: 0, index: 1)
            encoder.setFragmentTexture(auroraTexture, index: 0)
            encoder.setFragmentTexture(bubbleTexture, index: 1)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            encoder.endEncoding()
        }

        commandBuffer.present(drawable)
        commandBuffer.commit()
    }

    // MARK: - Render Helpers

    private func renderToTexture(
        commandBuffer: MTLCommandBuffer,
        texture: MTLTexture,
        pipeline: MTLRenderPipelineState,
        uniformBuffer: MTLBuffer
    ) {
        let renderPassDescriptor = MTLRenderPassDescriptor()
        renderPassDescriptor.colorAttachments[0].texture = texture
        renderPassDescriptor.colorAttachments[0].loadAction = .clear
        renderPassDescriptor.colorAttachments[0].storeAction = .store
        renderPassDescriptor.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)

        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPassDescriptor) else {
            return
        }

        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(vertexBuffer, offset: 0, index: 0)
        encoder.setFragmentBuffer(uniformBuffer, offset: 0, index: 1)
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        encoder.endEncoding()
    }

    private func renderMetaballs(
        commandBuffer: MTLCommandBuffer,
        outputTexture: MTLTexture,
        backgroundTexture: MTLTexture,
        pipeline: MTLRenderPipelineState,
        uniformBuffer: MTLBuffer
    ) {
        let renderPassDescriptor = MTLRenderPassDescriptor()
        renderPassDescriptor.colorAttachments[0].texture = outputTexture
        renderPassDescriptor.colorAttachments[0].loadAction = .clear
        renderPassDescriptor.colorAttachments[0].storeAction = .store
        renderPassDescriptor.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)

        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPassDescriptor) else {
            return
        }

        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(vertexBuffer, offset: 0, index: 0)
        encoder.setFragmentBuffer(uniformBuffer, offset: 0, index: 1)
        encoder.setFragmentBuffer(bubbleBuffer, offset: 0, index: 2)
        encoder.setFragmentTexture(backgroundTexture, index: 0)
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        encoder.endEncoding()
    }
}

// MARK: - Swift Types Bridging

struct LiquidGlassVertex {
    var position: SIMD2<Float>
    var texCoord: SIMD2<Float>
}

struct LiquidGlassUniforms {
    var time: Float = 0
    var scrollOffset: Float = 0
    var mousePosition: SIMD2<Float> = .zero
    var viewportSize: SIMD2<Float> = .zero
    var hoveredBubbleIndex: Int32 = -1
    var bubbleCount: Int32 = 0
    var inputFocused: Bool = false
    var breathPhase: Float = 0
    var scrollVelocity: Float = 0

    var accentColor: SIMD4<Float> = .zero
    var dominantColors: (SIMD4<Float>, SIMD4<Float>, SIMD4<Float>, SIMD4<Float>, SIMD4<Float>) = (.zero, .zero, .zero, .zero, .zero)
}

struct BubbleData {
    var center: SIMD2<Float> = .zero
    var size: SIMD2<Float> = .zero
    var cornerRadius: Float = 12
    var fusionWeight: Float = 1.0
    var timestamp: Float = 0
    var isUser: Bool = false
    var isHovered: Bool = false
    var isPressed: Bool = false
}
```

**Step 2: 验证文件**

Run: `wc -l platforms/macos/Aether/Sources/LiquidGlass/Metal/LiquidGlassRenderer.swift`
Expected: ~320 lines

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/Metal/LiquidGlassRenderer.swift
git commit -m "feat(liquid-glass): add Metal renderer with triple buffering and multi-pass rendering"
```

---

## Task 4: 创建 MTKView 封装

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/Metal/LiquidGlassMetalView.swift`

**Step 1: 创建 MTKView 封装**

```swift
//
//  LiquidGlassMetalView.swift
//  Aleph
//
//  SwiftUI wrapper for MTKView to render Liquid Glass effects.
//

import SwiftUI
import MetalKit

// MARK: - LiquidGlassMetalView

struct LiquidGlassMetalView: NSViewRepresentable {

    // Bubble data from SwiftUI
    @Binding var bubbles: [BubbleData]
    @Binding var scrollOffset: CGFloat
    @Binding var mousePosition: CGPoint
    @Binding var hoveredBubbleIndex: Int
    @Binding var inputFocused: Bool
    @Binding var scrollVelocity: CGFloat

    // Colors from wallpaper sampler
    @Binding var accentColor: SIMD4<Float>
    @Binding var dominantColors: [SIMD4<Float>]

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> MTKView {
        guard let device = MTLCreateSystemDefaultDevice() else {
            fatalError("Metal is not supported on this device")
        }

        let mtkView = MTKView(frame: .zero, device: device)
        mtkView.clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        mtkView.colorPixelFormat = .bgra8Unorm
        mtkView.layer?.isOpaque = false
        mtkView.preferredFramesPerSecond = 60
        mtkView.enableSetNeedsDisplay = false
        mtkView.isPaused = false

        // Create renderer
        if let renderer = LiquidGlassRenderer(device: device) {
            context.coordinator.renderer = renderer
            mtkView.delegate = renderer
        }

        return mtkView
    }

    func updateNSView(_ mtkView: MTKView, context: Context) {
        guard let renderer = context.coordinator.renderer else { return }

        // Update bubble data
        renderer.updateBubbles(bubbles)

        // Update scroll
        renderer.updateScrollOffset(Float(scrollOffset))

        // Update interaction state
        renderer.updateInteraction(
            mousePosition: SIMD2<Float>(Float(mousePosition.x), Float(mousePosition.y)),
            hoveredIndex: hoveredBubbleIndex,
            inputFocused: inputFocused,
            scrollVelocity: Float(scrollVelocity)
        )

        // Update colors
        renderer.updateColors(accent: accentColor, dominant: dominantColors)
    }

    class Coordinator {
        var renderer: LiquidGlassRenderer?
    }
}

// MARK: - Preview

#Preview("Liquid Glass Metal View") {
    LiquidGlassMetalView(
        bubbles: .constant([
            BubbleData(
                center: SIMD2<Float>(200, 150),
                size: SIMD2<Float>(300, 60),
                cornerRadius: 12,
                fusionWeight: 1.0,
                timestamp: 0,
                isUser: false,
                isHovered: false,
                isPressed: false
            ),
            BubbleData(
                center: SIMD2<Float>(200, 230),
                size: SIMD2<Float>(250, 50),
                cornerRadius: 12,
                fusionWeight: 1.0,
                timestamp: 1,
                isUser: true,
                isHovered: false,
                isPressed: false
            )
        ]),
        scrollOffset: .constant(0),
        mousePosition: .constant(.zero),
        hoveredBubbleIndex: .constant(-1),
        inputFocused: .constant(false),
        scrollVelocity: .constant(0),
        accentColor: .constant(SIMD4<Float>(0.0, 0.478, 1.0, 1.0)),
        dominantColors: .constant([
            SIMD4<Float>(0.4, 0.2, 0.6, 1.0),
            SIMD4<Float>(0.2, 0.5, 0.7, 1.0),
            SIMD4<Float>(0.6, 0.3, 0.5, 1.0),
            SIMD4<Float>(0.3, 0.6, 0.4, 1.0),
            SIMD4<Float>(0.5, 0.4, 0.6, 1.0)
        ])
    )
    .frame(width: 400, height: 300)
    .background(Color.black)
}
```

**Step 2: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/Metal/LiquidGlassMetalView.swift
git commit -m "feat(liquid-glass): add SwiftUI wrapper for MTKView"
```

---

## Task 5: 创建壁纸色采样器

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/ColorSampling/WallpaperColorSampler.swift`
- Create: `platforms/macos/Aether/Sources/LiquidGlass/ColorSampling/DominantColorExtractor.swift`

**Step 1: 创建 DominantColorExtractor**

```swift
//
//  DominantColorExtractor.swift
//  Aleph
//
//  K-means clustering for extracting dominant colors from images.
//

import AppKit
import Accelerate
import simd

// MARK: - DominantColorExtractor

struct DominantColorExtractor {

    /// Extract dominant colors using K-means clustering
    /// - Parameters:
    ///   - image: Source image
    ///   - count: Number of colors to extract (default 5)
    ///   - iterations: K-means iterations (default 10)
    /// - Returns: Array of dominant colors as SIMD4<Float> (RGBA)
    static func extract(from image: NSImage, count: Int = 5, iterations: Int = 10) -> [SIMD4<Float>] {
        guard let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            return defaultColors(count: count)
        }

        // Downsample to 32x32 for performance
        let sampleSize = 32
        guard let downsampled = downsample(cgImage, to: CGSize(width: sampleSize, height: sampleSize)) else {
            return defaultColors(count: count)
        }

        // Extract pixel colors
        let pixels = extractPixels(from: downsampled)
        guard !pixels.isEmpty else {
            return defaultColors(count: count)
        }

        // Run K-means
        let clusters = kmeans(pixels: pixels, k: count, iterations: iterations)

        // Sort by vibrancy (saturation * brightness)
        let sorted = clusters.sorted { vibrancy($0) > vibrancy($1) }

        return sorted
    }

    // MARK: - Private Helpers

    private static func downsample(_ image: CGImage, to size: CGSize) -> CGImage? {
        let width = Int(size.width)
        let height = Int(size.height)

        guard let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            return nil
        }

        context.interpolationQuality = .high
        context.draw(image, in: CGRect(origin: .zero, size: size))

        return context.makeImage()
    }

    private static func extractPixels(from image: CGImage) -> [SIMD4<Float>] {
        let width = image.width
        let height = image.height
        let bytesPerPixel = 4
        let bytesPerRow = width * bytesPerPixel

        var pixelData = [UInt8](repeating: 0, count: width * height * bytesPerPixel)

        guard let context = CGContext(
            data: &pixelData,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: bytesPerRow,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            return []
        }

        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))

        var pixels: [SIMD4<Float>] = []
        pixels.reserveCapacity(width * height)

        for y in 0..<height {
            for x in 0..<width {
                let offset = (y * width + x) * bytesPerPixel
                let r = Float(pixelData[offset]) / 255.0
                let g = Float(pixelData[offset + 1]) / 255.0
                let b = Float(pixelData[offset + 2]) / 255.0
                let a = Float(pixelData[offset + 3]) / 255.0

                // Skip nearly transparent pixels
                if a > 0.5 {
                    pixels.append(SIMD4<Float>(r, g, b, 1.0))
                }
            }
        }

        return pixels
    }

    private static func kmeans(pixels: [SIMD4<Float>], k: Int, iterations: Int) -> [SIMD4<Float>] {
        guard !pixels.isEmpty, k > 0 else { return [] }

        // Initialize centroids with evenly spaced pixels
        var centroids: [SIMD4<Float>] = []
        let step = max(1, pixels.count / k)
        for i in 0..<k {
            let index = min(i * step, pixels.count - 1)
            centroids.append(pixels[index])
        }

        // Iterate
        for _ in 0..<iterations {
            // Assign pixels to nearest centroid
            var clusters: [[SIMD4<Float>]] = Array(repeating: [], count: k)

            for pixel in pixels {
                var minDist = Float.infinity
                var minIndex = 0

                for (index, centroid) in centroids.enumerated() {
                    let diff = pixel - centroid
                    let dist = simd_dot(diff, diff)
                    if dist < minDist {
                        minDist = dist
                        minIndex = index
                    }
                }

                clusters[minIndex].append(pixel)
            }

            // Update centroids
            for i in 0..<k {
                if !clusters[i].isEmpty {
                    var sum = SIMD4<Float>.zero
                    for pixel in clusters[i] {
                        sum += pixel
                    }
                    centroids[i] = sum / Float(clusters[i].count)
                }
            }
        }

        return centroids
    }

    private static func vibrancy(_ color: SIMD4<Float>) -> Float {
        let r = color.x
        let g = color.y
        let b = color.z

        let maxC = max(r, max(g, b))
        let minC = min(r, min(g, b))

        let saturation = maxC > 0 ? (maxC - minC) / maxC : 0
        let brightness = maxC

        return saturation * brightness
    }

    private static func defaultColors(count: Int) -> [SIMD4<Float>] {
        // Default aurora-like colors
        let defaults: [SIMD4<Float>] = [
            SIMD4<Float>(0.3, 0.5, 0.7, 1.0),  // Blue
            SIMD4<Float>(0.5, 0.3, 0.6, 1.0),  // Purple
            SIMD4<Float>(0.4, 0.6, 0.5, 1.0),  // Teal
            SIMD4<Float>(0.6, 0.4, 0.5, 1.0),  // Pink
            SIMD4<Float>(0.5, 0.5, 0.6, 1.0),  // Lavender
        ]
        return Array(defaults.prefix(count))
    }
}
```

**Step 2: 创建 WallpaperColorSampler**

```swift
//
//  WallpaperColorSampler.swift
//  Aleph
//
//  Samples colors from the desktop wallpaper and system accent color.
//  Updates periodically and on system events.
//

import AppKit
import Combine
import simd

// MARK: - WallpaperColorSampler

@MainActor
final class WallpaperColorSampler: ObservableObject {

    // MARK: - Published Properties

    @Published private(set) var accentColor: SIMD4<Float> = SIMD4<Float>(0.0, 0.478, 1.0, 1.0)
    @Published private(set) var dominantColors: [SIMD4<Float>] = []

    // MARK: - Private Properties

    private var sampleTimer: Timer?
    private var lastSampleTime: Date = .distantPast
    private var windowObserver: NSObjectProtocol?
    private var wallpaperObserver: NSObjectProtocol?

    private let sampleInterval: TimeInterval = 5.0
    private let transitionDuration: TimeInterval = 0.8

    private var targetColors: [SIMD4<Float>] = []
    private var transitionProgress: Float = 1.0
    private var previousColors: [SIMD4<Float>] = []

    // MARK: - Initialization

    init() {
        setupDefaultColors()
        setupObservers()
        startPeriodicSampling()

        // Initial sample
        Task {
            await sample()
        }
    }

    deinit {
        sampleTimer?.invalidate()
        if let observer = windowObserver {
            NotificationCenter.default.removeObserver(observer)
        }
        if let observer = wallpaperObserver {
            DistributedNotificationCenter.default().removeObserver(observer)
        }
    }

    // MARK: - Setup

    private func setupDefaultColors() {
        dominantColors = [
            SIMD4<Float>(0.3, 0.5, 0.7, 1.0),
            SIMD4<Float>(0.5, 0.3, 0.6, 1.0),
            SIMD4<Float>(0.4, 0.6, 0.5, 1.0),
            SIMD4<Float>(0.6, 0.4, 0.5, 1.0),
            SIMD4<Float>(0.5, 0.5, 0.6, 1.0),
        ]
        targetColors = dominantColors
        previousColors = dominantColors
    }

    private func setupObservers() {
        // Window moved notification
        windowObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didMoveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.scheduleSample()
            }
        }

        // Wallpaper changed notification
        wallpaperObserver = DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.apple.desktop.background.changed"),
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.scheduleSample()
            }
        }
    }

    private func startPeriodicSampling() {
        sampleTimer = Timer.scheduledTimer(withTimeInterval: sampleInterval, repeats: true) { [weak self] _ in
            Task { @MainActor in
                await self?.sample()
            }
        }
    }

    // MARK: - Sampling

    private func scheduleSample() {
        // Debounce rapid events
        let now = Date()
        if now.timeIntervalSince(lastSampleTime) > 0.5 {
            Task {
                await sample()
            }
        }
    }

    func sample() async {
        lastSampleTime = Date()

        // Sample system accent color
        let accent = sampleAccentColor()

        // Sample wallpaper colors
        let wallpaperColors = await sampleWallpaperColors()

        // Mix accent with wallpaper colors
        var finalColors = wallpaperColors
        if !finalColors.isEmpty {
            // Blend accent color into first color slot (40% accent, 60% wallpaper)
            finalColors[0] = mix(accent, finalColors[0], t: 0.6)
        }

        // Start transition
        previousColors = dominantColors
        targetColors = finalColors
        transitionProgress = 0

        // Animate transition
        await animateTransition()

        accentColor = accent
    }

    private func sampleAccentColor() -> SIMD4<Float> {
        let nsColor = NSColor.controlAccentColor
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0

        if let rgbColor = nsColor.usingColorSpace(.deviceRGB) {
            rgbColor.getRed(&r, green: &g, blue: &b, alpha: &a)
        }

        return SIMD4<Float>(Float(r), Float(g), Float(b), Float(a))
    }

    private func sampleWallpaperColors() async -> [SIMD4<Float>] {
        // Capture screen behind window
        guard let screen = NSScreen.main else {
            return dominantColors
        }

        let screenRect = screen.frame

        // Create screenshot of desktop
        guard let screenshot = CGWindowListCreateImage(
            screenRect,
            .optionOnScreenBelowWindow,
            kCGNullWindowID,
            .bestResolution
        ) else {
            return dominantColors
        }

        let nsImage = NSImage(cgImage: screenshot, size: screenRect.size)

        // Extract dominant colors
        return DominantColorExtractor.extract(from: nsImage, count: 5)
    }

    private func animateTransition() async {
        let steps = 30
        let stepDuration = transitionDuration / Double(steps)

        for i in 1...steps {
            try? await Task.sleep(nanoseconds: UInt64(stepDuration * 1_000_000_000))

            transitionProgress = Float(i) / Float(steps)

            // Interpolate colors
            var interpolated: [SIMD4<Float>] = []
            for j in 0..<min(previousColors.count, targetColors.count) {
                let color = mix(previousColors[j], targetColors[j], t: transitionProgress)
                interpolated.append(color)
            }

            dominantColors = interpolated
        }
    }

    // MARK: - Helpers

    private func mix(_ a: SIMD4<Float>, _ b: SIMD4<Float>, t: Float) -> SIMD4<Float> {
        return a * (1 - t) + b * t
    }

    // MARK: - Manual Trigger

    func forceSample() {
        Task {
            await sample()
        }
    }
}
```

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/ColorSampling/
git commit -m "feat(liquid-glass): add wallpaper color sampling with K-means extraction"
```

---

## Task 6: 创建气泡融合计算器

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/Physics/BubbleFusionCalculator.swift`

**Step 1: 创建 BubbleFusionCalculator**

```swift
//
//  BubbleFusionCalculator.swift
//  Aleph
//
//  Calculates fusion weights for bubble merging based on distance, time, and interaction.
//

import Foundation
import simd

// MARK: - BubbleFusionCalculator

struct BubbleFusionCalculator {

    // MARK: - Configuration

    struct Config {
        /// Distance at which fusion begins
        var fusionStartDistance: Float = 60

        /// Distance at which fusion is complete
        var fusionCompleteDistance: Float = 8

        /// Time window for temporal fusion (seconds)
        var temporalFusionWindow: Float = 5.0

        /// Same-turn fusion bonus
        var sameTurnBonus: Float = 0.2

        /// Hover isolation factor (reduces fusion when hovering)
        var hoverIsolationFactor: Float = 0.5

        /// Scroll velocity impact on fusion threshold
        var scrollFusionMultiplier: Float = 0.5
    }

    static let defaultConfig = Config()

    // MARK: - Calculation

    /// Calculate fusion weights for all bubbles
    /// - Parameters:
    ///   - bubbles: Array of bubble data with positions and timestamps
    ///   - hoveredIndex: Index of currently hovered bubble (-1 if none)
    ///   - scrollVelocity: Current scroll velocity
    ///   - currentTime: Current timestamp
    ///   - config: Configuration parameters
    /// - Returns: Array of fusion weights (0 = isolated, 1 = fully fused)
    static func calculateFusionWeights(
        bubbles: [BubbleInfo],
        hoveredIndex: Int,
        scrollVelocity: Float,
        currentTime: Float,
        config: Config = defaultConfig
    ) -> [Float] {
        guard !bubbles.isEmpty else { return [] }

        var weights = [Float](repeating: 1.0, count: bubbles.count)

        // Adjust fusion threshold based on scroll velocity
        let velocityFactor = 1.0 + min(abs(scrollVelocity) / 500, 1.0) * config.scrollFusionMultiplier
        let adjustedStartDistance = config.fusionStartDistance * velocityFactor

        for i in 0..<bubbles.count {
            var fusionWeight: Float = 1.0

            // Check distance to adjacent bubbles
            if i > 0 {
                let distanceAbove = calculateDistance(bubbles[i], bubbles[i - 1])
                let distanceFusion = distanceFusionFactor(
                    distance: distanceAbove,
                    startDistance: adjustedStartDistance,
                    completeDistance: config.fusionCompleteDistance
                )

                // Time-based fusion
                let timeDelta = abs(bubbles[i].timestamp - bubbles[i - 1].timestamp)
                let timeFusion = timeFusionFactor(timeDelta: timeDelta, window: config.temporalFusionWindow)

                // Same turn bonus
                let sameRole = bubbles[i].isUser == bubbles[i - 1].isUser
                let turnBonus: Float = sameRole ? 0 : config.sameTurnBonus

                // Combine factors
                fusionWeight = min(fusionWeight, distanceFusion + timeFusion * 0.3 + turnBonus)
            }

            if i < bubbles.count - 1 {
                let distanceBelow = calculateDistance(bubbles[i], bubbles[i + 1])
                let distanceFusion = distanceFusionFactor(
                    distance: distanceBelow,
                    startDistance: adjustedStartDistance,
                    completeDistance: config.fusionCompleteDistance
                )

                fusionWeight = min(fusionWeight, distanceFusion)
            }

            // Hover isolation
            if i == hoveredIndex {
                fusionWeight *= config.hoverIsolationFactor
            }

            weights[i] = fusionWeight
        }

        return weights
    }

    // MARK: - Helper Functions

    private static func calculateDistance(_ a: BubbleInfo, _ b: BubbleInfo) -> Float {
        // Calculate vertical gap between bubbles (bottom of one to top of other)
        let aBottom = a.center.y - a.size.y / 2
        let bTop = b.center.y + b.size.y / 2
        let verticalGap = abs(aBottom - bTop)

        // Horizontal overlap consideration
        let horizontalOverlap = min(a.center.x + a.size.x / 2, b.center.x + b.size.x / 2) -
                               max(a.center.x - a.size.x / 2, b.center.x - b.size.x / 2)

        if horizontalOverlap > 0 {
            return verticalGap
        } else {
            // No horizontal overlap, use euclidean distance
            let dx = a.center.x - b.center.x
            let dy = a.center.y - b.center.y
            return sqrt(dx * dx + dy * dy)
        }
    }

    private static func distanceFusionFactor(distance: Float, startDistance: Float, completeDistance: Float) -> Float {
        if distance <= completeDistance {
            return 1.0
        } else if distance >= startDistance {
            return 0.0
        } else {
            // Smooth interpolation
            let t = (distance - completeDistance) / (startDistance - completeDistance)
            return 1.0 - smoothstep(t)
        }
    }

    private static func timeFusionFactor(timeDelta: Float, window: Float) -> Float {
        if timeDelta >= window {
            return 0.0
        }
        let t = timeDelta / window
        return 1.0 - smoothstep(t)
    }

    private static func smoothstep(_ t: Float) -> Float {
        let x = max(0, min(1, t))
        return x * x * (3 - 2 * x)
    }
}

// MARK: - BubbleInfo

struct BubbleInfo {
    var center: SIMD2<Float>
    var size: SIMD2<Float>
    var timestamp: Float
    var isUser: Bool
}
```

**Step 2: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/Physics/BubbleFusionCalculator.swift
git commit -m "feat(liquid-glass): add bubble fusion calculator with distance/time/interaction factors"
```

---

## Task 7: 创建配置文件

**Files:**
- Create: `platforms/macos/Aether/Sources/LiquidGlass/LiquidGlassConfiguration.swift`

**Step 1: 创建配置文件**

```swift
//
//  LiquidGlassConfiguration.swift
//  Aleph
//
//  Central configuration for Liquid Glass effects.
//

import Foundation
import simd

// MARK: - LiquidGlassConfiguration

struct LiquidGlassConfiguration {

    // MARK: - Animation

    struct Animation {
        /// Aurora flow speed (time scale)
        static let auroraFlowSpeed: Float = 0.1

        /// Aurora noise scale
        static let auroraNoiseScale: Float = 2.0

        /// FBM octaves for aurora
        static let auroraOctaves: Int = 4

        /// Breathing animation period (seconds)
        static let breathPeriod: Float = 4.0

        /// Breathing amplitude (brightness variation)
        static let breathAmplitude: Float = 0.15

        /// Edge glow variation
        static let edgeGlowVariation: Float = 0.10

        /// Hover rise height (visual pixels)
        static let hoverRiseHeight: Float = 4.0

        /// Hover shadow multiplier
        static let hoverShadowMultiplier: Float = 1.5

        /// Hover transition duration (seconds)
        static let hoverTransitionDuration: Double = 0.2

        /// Ripple expansion speed (pixels/second)
        static let rippleSpeed: Float = 200

        /// Ripple fade duration (seconds)
        static let rippleFadeDuration: Float = 0.5

        /// Input focus glow width (pixels)
        static let inputGlowWidth: Float = 3

        /// Input focus pulse period (seconds)
        static let inputPulsePeriod: Float = 2.0

        /// AI thinking rotation speed (radians/second)
        static let thinkingRotationSpeed: Float = 0.3

        /// AI thinking light band count
        static let thinkingLightBands: Int = 3

        /// AI thinking opacity
        static let thinkingOpacity: Float = 0.3
    }

    // MARK: - Fusion

    struct Fusion {
        /// Distance at which fusion begins (pixels)
        static let startDistance: Float = 60

        /// Distance at which fusion is complete (pixels)
        static let completeDistance: Float = 8

        /// Temporal fusion window (seconds)
        static let temporalWindow: Float = 5.0

        /// Same-turn fusion bonus
        static let sameTurnBonus: Float = 0.2

        /// Time factor decay rate
        static let timeDecayRate: Float = 0.0  // Placeholder for non-linear decay
    }

    // MARK: - Glass

    struct Glass {
        /// Overall glass transparency
        static let transparency: Float = 0.85

        /// Refraction strength
        static let refractionStrength: Float = 0.02

        /// Fresnel edge highlight intensity
        static let fresnelIntensity: Float = 0.6

        /// Fresnel power exponent
        static let fresnelPower: Float = 2.0

        /// Top highlight intensity
        static let topHighlightIntensity: Float = 0.15

        /// Inner depth tint (center darker)
        static let depthTintMin: Float = 0.95
        static let depthTintMax: Float = 1.0
    }

    // MARK: - Color

    struct Color {
        /// Accent color blend ratio (vs wallpaper)
        static let accentBlendRatio: Float = 0.4

        /// Wallpaper sample interval (seconds)
        static let sampleInterval: TimeInterval = 5.0

        /// Color transition duration (seconds)
        static let transitionDuration: TimeInterval = 0.8

        /// Low vibrancy threshold (inject accent if below)
        static let lowVibrancyThreshold: Float = 0.2

        /// Vibrancy boost amount
        static let vibrancyBoostAmount: Float = 0.2
    }

    // MARK: - Performance

    struct Performance {
        /// Maximum bubbles to render
        static let maxBubbles: Int = 50

        /// LOD reduction threshold (bubble count)
        static let lodThreshold: Int = 20

        /// Target frame rate
        static let targetFrameRate: Int = 60

        /// Triple buffer count
        static let bufferCount: Int = 3
    }

    // MARK: - Scroll Physics

    struct ScrollPhysics {
        /// Velocity threshold for fusion adjustment
        static let velocityThreshold: Float = 500

        /// Maximum fusion threshold multiplier
        static let maxFusionMultiplier: Float = 1.5

        /// Bubble spacing increase factor
        static let spacingIncreaseFactor: Float = 0.3
    }
}
```

**Step 2: Commit**

```bash
git add platforms/macos/Aether/Sources/LiquidGlass/LiquidGlassConfiguration.swift
git commit -m "feat(liquid-glass): add central configuration for all effect parameters"
```

---

## Task 8: 创建 BubbleGeometryReporter

**Files:**
- Create: `platforms/macos/Aether/Sources/MultiTurn/Views/BubbleGeometryReporter.swift`

**Step 1: 创建 BubbleGeometryReporter**

```swift
//
//  BubbleGeometryReporter.swift
//  Aleph
//
//  Reports bubble geometry from SwiftUI to Metal layer.
//  Uses preference keys to collect frame data.
//

import SwiftUI
import simd

// MARK: - BubbleGeometryPreferenceKey

struct BubbleGeometryPreferenceKey: PreferenceKey {
    static var defaultValue: [BubbleGeometry] = []

    static func reduce(value: inout [BubbleGeometry], nextValue: () -> [BubbleGeometry]) {
        value.append(contentsOf: nextValue())
    }
}

// MARK: - BubbleGeometry

struct BubbleGeometry: Equatable {
    let id: String
    let frame: CGRect
    let isUser: Bool
    let timestamp: TimeInterval
    let index: Int
}

// MARK: - BubbleGeometryReporter Modifier

struct BubbleGeometryReporter: ViewModifier {
    let id: String
    let isUser: Bool
    let timestamp: TimeInterval
    let index: Int
    let coordinateSpace: CoordinateSpace

    func body(content: Content) -> some View {
        content
            .background(
                GeometryReader { geometry in
                    Color.clear
                        .preference(
                            key: BubbleGeometryPreferenceKey.self,
                            value: [
                                BubbleGeometry(
                                    id: id,
                                    frame: geometry.frame(in: coordinateSpace),
                                    isUser: isUser,
                                    timestamp: timestamp,
                                    index: index
                                )
                            ]
                        )
                }
            )
    }
}

// MARK: - View Extension

extension View {
    func reportBubbleGeometry(
        id: String,
        isUser: Bool,
        timestamp: TimeInterval,
        index: Int,
        coordinateSpace: CoordinateSpace = .named("liquidGlass")
    ) -> some View {
        modifier(BubbleGeometryReporter(
            id: id,
            isUser: isUser,
            timestamp: timestamp,
            index: index,
            coordinateSpace: coordinateSpace
        ))
    }
}

// MARK: - Geometry to BubbleData Converter

extension BubbleGeometry {
    func toBubbleData(in viewportSize: CGSize, startTime: TimeInterval) -> BubbleData {
        // Convert SwiftUI coordinates to Metal coordinates
        // SwiftUI: origin at top-left, Y increases downward
        // Metal texture: origin at top-left, same convention

        let center = SIMD2<Float>(
            Float(frame.midX),
            Float(frame.midY)
        )

        let size = SIMD2<Float>(
            Float(frame.width),
            Float(frame.height)
        )

        return BubbleData(
            center: center,
            size: size,
            cornerRadius: 12,  // Default corner radius
            fusionWeight: 1.0,  // Will be calculated by BubbleFusionCalculator
            timestamp: Float(timestamp - startTime),
            isUser: isUser,
            isHovered: false,
            isPressed: false
        )
    }

    func toBubbleInfo(startTime: TimeInterval) -> BubbleInfo {
        return BubbleInfo(
            center: SIMD2<Float>(Float(frame.midX), Float(frame.midY)),
            size: SIMD2<Float>(Float(frame.width), Float(frame.height)),
            timestamp: Float(timestamp - startTime),
            isUser: isUser
        )
    }
}

// MARK: - BubbleDataCollector

@MainActor
class BubbleDataCollector: ObservableObject {
    @Published var bubbles: [BubbleData] = []
    @Published var hoveredIndex: Int = -1

    private var geometries: [BubbleGeometry] = []
    private let startTime: TimeInterval = Date().timeIntervalSince1970

    func updateGeometries(_ newGeometries: [BubbleGeometry], viewportSize: CGSize) {
        geometries = newGeometries.sorted { $0.index < $1.index }
        recalculateBubbles(viewportSize: viewportSize)
    }

    func setHoveredBubble(id: String?) {
        if let id = id {
            hoveredIndex = geometries.firstIndex { $0.id == id } ?? -1
        } else {
            hoveredIndex = -1
        }
    }

    private func recalculateBubbles(viewportSize: CGSize, scrollVelocity: Float = 0) {
        // Convert geometries to BubbleInfo for fusion calculation
        let bubbleInfos = geometries.map { $0.toBubbleInfo(startTime: startTime) }

        // Calculate fusion weights
        let weights = BubbleFusionCalculator.calculateFusionWeights(
            bubbles: bubbleInfos,
            hoveredIndex: hoveredIndex,
            scrollVelocity: scrollVelocity,
            currentTime: Float(Date().timeIntervalSince1970 - startTime)
        )

        // Convert to BubbleData with weights
        var newBubbles: [BubbleData] = []
        for (index, geometry) in geometries.enumerated() {
            var bubbleData = geometry.toBubbleData(in: viewportSize, startTime: startTime)
            if index < weights.count {
                bubbleData.fusionWeight = weights[index]
            }
            bubbleData.isHovered = index == hoveredIndex
            newBubbles.append(bubbleData)
        }

        bubbles = newBubbles
    }
}
```

**Step 2: Commit**

```bash
git add platforms/macos/Aether/Sources/MultiTurn/Views/BubbleGeometryReporter.swift
git commit -m "feat(liquid-glass): add geometry reporter for SwiftUI-Metal bridge"
```

---

## Task 9: 集成 Metal 层到 UnifiedConversationView

**Files:**
- Modify: `platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationView.swift`

**Step 1: 读取当前文件（已完成）**

**Step 2: 修改 UnifiedConversationView 添加 Metal 背景层**

在 `UnifiedConversationView.swift` 中：

1. 添加 import 和状态变量
2. 替换背景为 LiquidGlassMetalView
3. 添加 coordinate space 和 geometry 收集
4. 移除旧的 VisualEffectBackground

具体修改：

```swift
// 在文件顶部添加:
import simd

// 在 struct UnifiedConversationView 中添加:
@StateObject private var colorSampler = WallpaperColorSampler()
@StateObject private var bubbleCollector = BubbleDataCollector()
@State private var scrollOffset: CGFloat = 0
@State private var scrollVelocity: CGFloat = 0
@State private var mousePosition: CGPoint = .zero
@State private var inputFocused: Bool = false

// 修改 body:
var body: some View {
    ZStack {
        // Metal background layer
        LiquidGlassMetalView(
            bubbles: $bubbleCollector.bubbles,
            scrollOffset: $scrollOffset,
            mousePosition: $mousePosition,
            hoveredBubbleIndex: $bubbleCollector.hoveredIndex,
            inputFocused: $inputFocused,
            scrollVelocity: $scrollVelocity,
            accentColor: .constant(colorSampler.accentColor),
            dominantColors: .constant(colorSampler.dominantColors)
        )

        // SwiftUI content overlay
        VStack(spacing: 0) {
            Spacer(minLength: 0)
            contentWithBackground
        }
    }
    .coordinateSpace(name: "liquidGlass")
    .onPreferenceChange(BubbleGeometryPreferenceKey.self) { geometries in
        bubbleCollector.updateGeometries(geometries, viewportSize: CGSize(width: 800, height: 600))
    }
    .onDrop(of: [.fileURL], isTargeted: nil) { providers in
        handleDrop(providers: providers)
    }
}

// 修改 contentWithBackground，移除旧背景:
private var contentWithBackground: some View {
    VStack(spacing: 0) {
        contentArea

        if viewModel.shouldShowConversation {
            Divider()
                .opacity(0.3)
                .padding(.horizontal, 12)

            infoStreamStatusBar

            Divider()
                .opacity(0.3)
                .padding(.horizontal, 12)
        }

        InputAreaView(viewModel: viewModel)
    }
    .frame(width: 800)
    // 移除旧的 .background(ZStack { ... })
    // 保留 clipShape 和 overlay
    .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
    .overlay(
        RoundedRectangle(cornerRadius: 20, style: .continuous)
            .stroke(
                LinearGradient(
                    colors: [
                        .white.opacity(0.35),
                        .white.opacity(0.1),
                        .white.opacity(0.02)
                    ],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                ),
                lineWidth: 1
            )
    )
    .shadow(color: .black.opacity(0.2), radius: 15, x: 0, y: 8)
    .animation(.smooth(duration: 0.25), value: viewModel.displayState)
}
```

**Step 3: 验证修改**

Run: `cd platforms/macos && xcodebuild -scheme Aleph -configuration Debug build 2>&1 | head -50`
Expected: Build starts (may have warnings initially)

**Step 4: Commit**

```bash
git add platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationView.swift
git commit -m "feat(liquid-glass): integrate Metal layer into UnifiedConversationView"
```

---

## Task 10: 修改 MessageBubbleView 添加几何上报

**Files:**
- Modify: `platforms/macos/Aether/Sources/MultiTurn/Views/MessageBubbleView.swift`

**Step 1: 修改 MessageBubbleView**

在 `RichMessageContentView` 的 body 中添加 geometry reporter：

```swift
// 在 RichMessageContentView 中:
var body: some View {
    if !textOnlyContent.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        Text(textOnlyContent)
            .font(.system(size: 13))
            .liquidGlassText()
            .textSelection(.enabled)
            .padding(12)
            // 移除 .glassBubble(isUser: isUser) - Metal 层会渲染玻璃效果
            .background(Color.clear) // 透明背景，让 Metal 层显示
    }
}
```

在 `MessageBubbleView` 中添加 geometry 上报：

```swift
// 在 MessageBubbleView body 的外层 HStack 添加:
.reportBubbleGeometry(
    id: message.id,
    isUser: isUser,
    timestamp: message.timestamp?.timeIntervalSince1970 ?? Date().timeIntervalSince1970,
    index: 0  // 将从外部传入
)
```

**Step 2: Commit**

```bash
git add platforms/macos/Aether/Sources/MultiTurn/Views/MessageBubbleView.swift
git commit -m "feat(liquid-glass): add geometry reporting to MessageBubbleView"
```

---

## Task 11: 修改 InputAreaView 透明背景

**Files:**
- Modify: `platforms/macos/Aether/Sources/MultiTurn/Views/InputAreaView.swift`

**Step 1: 修改 InputAreaView**

将输入框背景改为透明，让 Metal 层渲染玻璃效果：

```swift
// 在 InputAreaView 的 input container 背景中:
// 移除 VisualEffectBackground，改为透明
.background {
    ZStack {
        // 透明背景，让 Metal 层显示
        Color.clear

        // 保留边框效果
        RoundedRectangle(cornerRadius: 12)
            .stroke(
                isTargeted ? AnyShapeStyle(Color.cyan.gradient) :
                    (isFocused ? AnyShapeStyle(.primary.opacity(0.2)) : AnyShapeStyle(LinearGradient(
                        colors: [.white.opacity(0.35), .clear, .white.opacity(0.1)],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    ))),
                lineWidth: isTargeted ? 2 : 1
            )
    }
    .clipShape(RoundedRectangle(cornerRadius: 12))
}
```

**Step 2: 添加输入框几何上报**

在 InputAreaView 的 HStack 外层添加：

```swift
.reportBubbleGeometry(
    id: "input-area",
    isUser: true,
    timestamp: Date().timeIntervalSince1970,
    index: -1  // 特殊索引表示输入框
)
```

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/MultiTurn/Views/InputAreaView.swift
git commit -m "feat(liquid-glass): make InputAreaView transparent for Metal rendering"
```

---

## Task 12: 添加 Metal 文件到 Xcode 项目

**Files:**
- Modify: `platforms/macos/Aether/project.yml` (如果使用 XcodeGen)
- 或手动添加到 Xcode 项目

**Step 1: 检查项目配置方式**

Run: `ls platforms/macos/Aether/*.xcodeproj 2>/dev/null || ls platforms/macos/*.xcodeproj 2>/dev/null || echo "No xcodeproj found"`

**Step 2: 如果使用 XcodeGen，更新 project.yml**

添加 Metal 和 Shader 文件到 sources。

**Step 3: 创建 bridging header 确保 ShaderTypes.h 可用**

如果需要，创建 bridging header 并添加:
```c
#import "ShaderTypes.h"
```

**Step 4: Commit**

```bash
git add -A
git commit -m "chore: configure Xcode project for Metal shaders"
```

---

## Task 13: 构建和测试

**Step 1: 构建项目**

Run: `cd platforms/macos && xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -20`
Expected: BUILD SUCCEEDED

**Step 2: 修复编译错误（如果有）**

根据错误信息修复问题。常见问题：
- ShaderTypes.h 路径问题
- Metal shader 编译错误
- Swift/Metal 类型不匹配

**Step 3: 运行测试**

Run: `cd platforms/macos && xcodebuild -scheme Aleph -configuration Debug test 2>&1 | tail -30`

**Step 4: 最终 Commit**

```bash
git add -A
git commit -m "feat(liquid-glass): complete Phase 1 - Metal infrastructure"
```

---

## 后续任务 (Phase 2-6)

以上完成了 Phase 1（Metal 基础设施）。后续阶段将在此基础上继续：

- **Phase 2**: 极光背景动效调优
- **Phase 3**: 气泡融合效果细化
- **Phase 4**: 玻璃光学增强
- **Phase 5**: 交互响应（悬停、点击、滚动）
- **Phase 6**: 性能优化和降级处理

每个阶段将按照相同的 TDD 模式进行：写测试 → 运行失败 → 实现 → 运行通过 → 提交。
