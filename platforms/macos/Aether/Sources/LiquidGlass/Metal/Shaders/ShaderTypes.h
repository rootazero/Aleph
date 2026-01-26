//
//  ShaderTypes.h
//  Aether
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
