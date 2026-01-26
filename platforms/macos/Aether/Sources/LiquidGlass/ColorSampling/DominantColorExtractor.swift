//
//  DominantColorExtractor.swift
//  Aether
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
