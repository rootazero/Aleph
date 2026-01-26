//
//  LiquidGlassRenderer.swift
//  Aether
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

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {
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
