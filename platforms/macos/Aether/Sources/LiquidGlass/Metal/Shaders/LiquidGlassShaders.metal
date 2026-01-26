//
//  LiquidGlassShaders.metal
//  Aether
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
