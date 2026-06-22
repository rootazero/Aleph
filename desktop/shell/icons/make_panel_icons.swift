// make_panel_icons.swift
//
// Generate the "Aleph Panel" (lite shell) icon variant: the full-app artwork
// with a small cyan badge bearing a dark "P" in the bottom-right corner.
//
// Pipeline mirrors icons/README.md:
//   source.png (1024 full-bleed master)
//     -> composite badge  -> panel/panel-source.png (1024 full-bleed master)
//     -> downscale         -> panel/32x32.png / 128x128.png / 128x128@2x.png  (full-bleed)
//     -> pad 824/1024      -> .iconset -> iconutil -> panel/icon.icns         (macOS safe-area)
//     -> multi-size ICO    -> panel/icon.ico                                  (Windows)
//
// Run (macOS, no external deps):
//   cd desktop/shell/icons && swift make_panel_icons.swift
//
// NOTE: only the lite build consumes these (bundle.icon override in
// tauri.lite.conf.json). The full-app icons/* are never touched.

import Foundation
import CoreGraphics
import CoreText
import ImageIO
import UniformTypeIdentifiers

// ---- paths ----
let here = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let sourceURL = here.appendingPathComponent("source.png")
let outDir = here.appendingPathComponent("panel")
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

// ---- helpers ----
func loadImage(_ url: URL) -> CGImage {
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
        fatalError("cannot load \(url.path)")
    }
    return img
}

func makeContext(_ size: Int) -> CGContext {
    let cs = CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data: nil, width: size, height: size,
                        bitsPerComponent: 8, bytesPerRow: 0, space: cs,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    return ctx
}

func savePNG(_ image: CGImage, _ url: URL) {
    let dst = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dst, image, nil)
    CGImageDestinationFinalize(dst)
}

func rgb(_ r: Double, _ g: Double, _ b: Double, _ a: Double = 1) -> CGColor {
    CGColor(srgbRed: r/255, green: g/255, blue: b/255, alpha: a)
}

// Draw the floating "P" badge in the bottom-right of the artwork: cyan fill +
// white keyline + soft cyan glow. No disc — it floats via the keyline + glow,
// echoing the glowing Aleph.
func drawBadge(_ ctx: CGContext, _ W: CGFloat) {
    let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

    // Geometry: cap-height ~25% of canvas, anchored bottom-right (CG origin bottom-left).
    let cap = W * 0.250
    let cx = W * 0.768
    let cy = W * 0.250

    // Build the positioned "P" glyph path.
    let font = CTFontCreateWithName("HelveticaNeue-Bold" as CFString, cap / 0.717, nil)
    var uni: [UniChar] = Array("P".utf16)
    var glyphs = [CGGlyph](repeating: 0, count: uni.count)
    CTFontGetGlyphsForCharacters(font, &uni, &glyphs, uni.count)
    var ident = CGAffineTransform.identity
    let raw = CTFontCreatePathForGlyph(font, glyphs[0], &ident)!
    let bb = raw.boundingBoxOfPath
    var move = CGAffineTransform(translationX: cx - bb.midX, y: cy - bb.midY)
    let path = raw.copy(using: &move)!
    let keyline = cap * 0.16

    // 1) Cyan fill + soft cyan glow (luminous float).
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: cap * 0.34, color: rgb(0x4F, 0xE6, 0xF7, 0.95))
    ctx.addPath(path); ctx.setFillColor(rgb(0x3A, 0xCF, 0xE6)); ctx.fillPath()
    ctx.restoreGState()

    // 2) White keyline for a crisp floating edge (high contrast on the dark body).
    ctx.saveGState()
    ctx.addPath(path); ctx.setLineWidth(keyline * 0.8); ctx.setLineJoin(.round)
    ctx.setStrokeColor(rgb(0xFF, 0xFF, 0xFF)); ctx.strokePath()
    ctx.restoreGState()

    // 3) Cyan vertical gradient gloss (covers the inner half of the keyline).
    ctx.saveGState()
    ctx.addPath(path); ctx.clip()
    let grad = CGGradient(colorsSpace: sRGB,
        colors: [rgb(0x9A, 0xF2, 0xFC), rgb(0x27, 0xC0, 0xDB)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: CGPoint(x: cx, y: cy + cap*0.6),
        end: CGPoint(x: cx, y: cy - cap*0.6),
        options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()
}

// ---- build full-bleed badged master (1024) ----
let source = loadImage(sourceURL)
let MASTER = 1024
let masterCtx = makeContext(MASTER)
masterCtx.draw(source, in: CGRect(x: 0, y: 0, width: MASTER, height: MASTER))
drawBadge(masterCtx, CGFloat(MASTER))
let badgedMaster = masterCtx.makeImage()!
savePNG(badgedMaster, outDir.appendingPathComponent("panel-source.png"))

// downscale helper from the badged master
func scaled(_ size: Int) -> CGImage {
    let ctx = makeContext(size)
    ctx.draw(badgedMaster, in: CGRect(x: 0, y: 0, width: size, height: size))
    return ctx.makeImage()!
}

// ---- full-bleed PNGs (Linux tray / Tauri bundle) ----
savePNG(scaled(32),  outDir.appendingPathComponent("32x32.png"))
savePNG(scaled(128), outDir.appendingPathComponent("128x128.png"))
savePNG(scaled(256), outDir.appendingPathComponent("128x128@2x.png"))

// ---- padded master (824 art on 1024 transparent) for macOS .icns ----
let paddedCtx = makeContext(MASTER)
let inner = CGFloat(824)
let off = (CGFloat(MASTER) - inner) / 2     // 100px margins
paddedCtx.draw(badgedMaster, in: CGRect(x: off, y: off, width: inner, height: inner))
let paddedMaster = paddedCtx.makeImage()!

func paddedScaled(_ size: Int) -> CGImage {
    let ctx = makeContext(size)
    ctx.draw(paddedMaster, in: CGRect(x: 0, y: 0, width: size, height: size))
    return ctx.makeImage()!
}

// ---- build .iconset and run iconutil ----
let iconset = outDir.appendingPathComponent("Aleph-Panel.iconset")
try? FileManager.default.removeItem(at: iconset)
try? FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
let icnsSpec: [(String, Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
]
for (name, size) in icnsSpec {
    savePNG(paddedScaled(size), iconset.appendingPathComponent(name))
}
let proc = Process()
proc.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
proc.arguments = ["-c", "icns", iconset.path,
                  "-o", outDir.appendingPathComponent("icon.icns").path]
try! proc.run(); proc.waitUntilExit()
try? FileManager.default.removeItem(at: iconset)

// ---- multi-size .ico (full-bleed) for Windows ----
let icoURL = outDir.appendingPathComponent("icon.ico")
let icoSizes = [16, 32, 48, 64, 128, 256]
let icoDst = CGImageDestinationCreateWithURL(
    icoURL as CFURL, UTType.ico.identifier as CFString, icoSizes.count, nil)!
for size in icoSizes { CGImageDestinationAddImage(icoDst, scaled(size), nil) }
CGImageDestinationFinalize(icoDst)

print("panel icons written to \(outDir.path)")
