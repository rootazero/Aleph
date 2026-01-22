#!/usr/bin/env node
/**
 * Generate Tauri app icons from SVG source.
 *
 * This script converts the Aether SVG icon to all required formats for Tauri:
 * - 32x32.png
 * - 128x128.png
 * - 128x128@2x.png (256x256)
 * - icon.ico (Windows, multi-size)
 * - icon.png (128x128 for tray)
 *
 * Also copies provider SVG icons to Tauri frontend assets.
 *
 * Usage:
 *   cd platforms/tauri && node scripts/generate-icons.mjs
 */

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";
import sharp from "sharp";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const TAURI_ROOT = path.resolve(__dirname, "..");
const PROJECT_ROOT = path.resolve(TAURI_ROOT, "../..");

// Paths
const MACOS_RESOURCES = path.join(
  PROJECT_ROOT,
  "platforms/macos/Aether/Resources"
);
const TAURI_ICONS = path.join(TAURI_ROOT, "src-tauri/icons");
const TAURI_ASSETS = path.join(TAURI_ROOT, "src/assets");

// Source files
const SVG_SOURCE = path.join(MACOS_RESOURCES, "AppIcon/AetherAppIcon.svg");
const ICNS_SOURCE = path.join(MACOS_RESOURCES, "AppIcon/AppIcon.icns");
const PROVIDER_ICONS_DIR = path.join(MACOS_RESOURCES, "ProviderIcons");

async function svgToPng(svgPath, pngPath, size) {
  const svgBuffer = fs.readFileSync(svgPath);

  await sharp(svgBuffer, { density: 300 })
    .resize(size, size)
    .png()
    .toFile(pngPath);

  console.log(`  Generated: ${path.basename(pngPath)} (${size}x${size})`);
}

async function createIco(pngPaths, icoPath) {
  const icoSizes = [16, 32, 48, 64, 128, 256];
  const images = [];

  for (const size of icoSizes) {
    const pngPath = pngPaths[size];
    let buffer;

    if (pngPath && fs.existsSync(pngPath)) {
      buffer = await sharp(pngPath).png().toBuffer();
    } else {
      const sourcePath = pngPaths[256] || pngPaths[128] || pngPaths[32];
      buffer = await sharp(sourcePath).resize(size, size).png().toBuffer();
    }
    images.push({ size, buffer });
  }

  // Build ICO file
  const icoBuffer = buildIcoBuffer(images);
  fs.writeFileSync(icoPath, icoBuffer);

  console.log(
    `  Generated: ${path.basename(icoPath)} (sizes: ${icoSizes.join(", ")})`
  );
}

function buildIcoBuffer(images) {
  const numImages = images.length;
  const headerSize = 6;
  const dirEntrySize = 16;
  const dirSize = dirEntrySize * numImages;

  let dataOffset = headerSize + dirSize;
  const imageData = [];

  for (const { size, buffer } of images) {
    imageData.push({ size, buffer, offset: dataOffset });
    dataOffset += buffer.length;
  }

  const totalSize = dataOffset;
  const ico = Buffer.alloc(totalSize);

  // Write header
  ico.writeUInt16LE(0, 0); // Reserved
  ico.writeUInt16LE(1, 2); // Type (ICO)
  ico.writeUInt16LE(numImages, 4); // Count

  // Write directory entries
  let dirOffset = headerSize;
  for (const { size, buffer, offset } of imageData) {
    ico.writeUInt8(size === 256 ? 0 : size, dirOffset); // Width
    ico.writeUInt8(size === 256 ? 0 : size, dirOffset + 1); // Height
    ico.writeUInt8(0, dirOffset + 2); // Color palette
    ico.writeUInt8(0, dirOffset + 3); // Reserved
    ico.writeUInt16LE(1, dirOffset + 4); // Color planes
    ico.writeUInt16LE(32, dirOffset + 6); // Bits per pixel
    ico.writeUInt32LE(buffer.length, dirOffset + 8); // Size
    ico.writeUInt32LE(offset, dirOffset + 12); // Offset
    dirOffset += dirEntrySize;
  }

  // Write image data
  for (const { buffer, offset } of imageData) {
    buffer.copy(ico, offset);
  }

  return ico;
}

async function generateAppIcons() {
  console.log("\n=== Generating Tauri App Icons ===\n");

  if (!fs.existsSync(SVG_SOURCE)) {
    throw new Error(`SVG source not found: ${SVG_SOURCE}`);
  }

  fs.mkdirSync(TAURI_ICONS, { recursive: true });

  const pngSizes = {
    32: path.join(TAURI_ICONS, "32x32.png"),
    128: path.join(TAURI_ICONS, "128x128.png"),
    256: path.join(TAURI_ICONS, "128x128@2x.png"),
  };

  const tempPngs = {};

  for (const [size, outputPath] of Object.entries(pngSizes)) {
    await svgToPng(SVG_SOURCE, outputPath, parseInt(size));
    tempPngs[parseInt(size)] = outputPath;
  }

  // Generate additional sizes for ICO
  for (const size of [16, 48, 64]) {
    const tempPath = path.join(TAURI_ICONS, `temp_${size}.png`);
    await svgToPng(SVG_SOURCE, tempPath, size);
    tempPngs[size] = tempPath;
  }

  // Generate icon.ico
  await createIco(tempPngs, path.join(TAURI_ICONS, "icon.ico"));

  // Clean up temp files
  for (const size of [16, 48, 64]) {
    const tempPath = path.join(TAURI_ICONS, `temp_${size}.png`);
    if (fs.existsSync(tempPath)) {
      fs.unlinkSync(tempPath);
    }
  }

  // Generate tray icon
  await svgToPng(SVG_SOURCE, path.join(TAURI_ICONS, "icon.png"), 128);
}

function copyIcns() {
  console.log("\n=== Copying macOS .icns ===\n");

  if (!fs.existsSync(ICNS_SOURCE)) {
    console.log(`  Warning: .icns source not found: ${ICNS_SOURCE}`);
    console.log("  Skipping .icns copy (only needed for macOS builds)");
    return;
  }

  const dest = path.join(TAURI_ICONS, "icon.icns");
  fs.copyFileSync(ICNS_SOURCE, dest);
  console.log(`  Copied: icon.icns`);
}

function copyProviderIcons() {
  console.log("\n=== Copying Provider Icons ===\n");

  if (!fs.existsSync(PROVIDER_ICONS_DIR)) {
    console.log(
      `  Warning: Provider icons directory not found: ${PROVIDER_ICONS_DIR}`
    );
    return;
  }

  const destDir = path.join(TAURI_ASSETS, "providers");
  fs.mkdirSync(destDir, { recursive: true });

  const svgFiles = fs
    .readdirSync(PROVIDER_ICONS_DIR)
    .filter((f) => f.endsWith(".svg"))
    .sort();

  for (const svgFile of svgFiles) {
    const src = path.join(PROVIDER_ICONS_DIR, svgFile);
    const dest = path.join(destDir, svgFile);
    fs.copyFileSync(src, dest);
    console.log(`  Copied: ${svgFile}`);
  }

  console.log(`\n  Total: ${svgFiles.length} provider icons`);
}

async function verifyIcons() {
  console.log("\n=== Verifying Icons ===\n");

  const requiredFiles = [
    ["32x32.png", 32],
    ["128x128.png", 128],
    ["128x128@2x.png", 256],
    ["icon.ico", null],
    ["icon.png", 128],
  ];

  let allOk = true;

  for (const [filename, expectedSize] of requiredFiles) {
    const filepath = path.join(TAURI_ICONS, filename);

    if (!fs.existsSync(filepath)) {
      console.log(`  MISSING: ${filename}`);
      allOk = false;
      continue;
    }

    if (expectedSize && filename.endsWith(".png")) {
      const metadata = await sharp(filepath).metadata();
      if (
        metadata.width !== expectedSize ||
        metadata.height !== expectedSize
      ) {
        console.log(
          `  SIZE ERROR: ${filename} is ${metadata.width}x${metadata.height}, expected ${expectedSize}x${expectedSize}`
        );
        allOk = false;
      } else {
        console.log(`  OK: ${filename} (${expectedSize}x${expectedSize})`);
      }
    } else {
      const stats = fs.statSync(filepath);
      const sizeKb = (stats.size / 1024).toFixed(1);
      console.log(`  OK: ${filename} (${sizeKb} KB)`);
    }
  }

  // Check icns
  const icnsPath = path.join(TAURI_ICONS, "icon.icns");
  if (fs.existsSync(icnsPath)) {
    const stats = fs.statSync(icnsPath);
    const sizeKb = (stats.size / 1024).toFixed(1);
    console.log(`  OK: icon.icns (${sizeKb} KB)`);
  } else {
    console.log(`  SKIPPED: icon.icns (not required on Windows)`);
  }

  // Check provider icons
  const providerDir = path.join(TAURI_ASSETS, "providers");
  if (fs.existsSync(providerDir)) {
    const svgCount = fs
      .readdirSync(providerDir)
      .filter((f) => f.endsWith(".svg")).length;
    console.log(`  OK: ${svgCount} provider icons in src/assets/providers`);
  }

  if (allOk) {
    console.log("\n  All required icons verified!");
  } else {
    console.log("\n  Some icons are missing or incorrect!");
    process.exit(1);
  }
}

async function main() {
  console.log(`Project root: ${PROJECT_ROOT}`);
  console.log(`SVG source: ${SVG_SOURCE}`);
  console.log(`Output dir: ${TAURI_ICONS}`);

  await generateAppIcons();
  copyIcns();
  copyProviderIcons();
  await verifyIcons();

  console.log("\n=== Done! ===\n");
}

main().catch((err) => {
  console.error("Error:", err);
  process.exit(1);
});
