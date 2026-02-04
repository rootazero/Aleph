# Aether App Icon Integration

## Files Generated

1. **AetherLogo.svg** - Main logo with gradients (for marketing, web, etc.)
2. **AetherAppIcon.svg** - App icon with background (1024x1024 base)
3. **AetherMenuBar.svg** - Menu bar icon (template mode, monochrome)
4. **AetherSimple.svg** - Simplified version for small sizes

## Integration Steps

### 1. Generate .icns file (macOS App Icon)

```bash
cd Aether
./Scripts/generate_app_icon.sh
```

This will create `AppIcon.icns` from `AetherAppIcon.svg`.

### 2. Add to Xcode Project

**Option A: Use .icns directly**
1. In Xcode, select `Aether/Assets.xcassets`
2. Select `AppIcon` imageset
3. Drag `AppIcon.icns` into the appropriate slots
4. Or manually configure in `project.yml`:

```yaml
targets:
  Aether:
    settings:
      ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon
```

**Option B: Use individual PNG files**
1. Keep the `.iconset` folder
2. Drag individual PNG files to corresponding size slots in Assets.xcassets

### 3. Menu Bar Icon

For the menu bar icon, add `AetherMenuBar.svg` to Assets.xcassets:

1. Create new Image Set: `MenuBarIcon`
2. Set "Render As" to "Template Image"
3. Add `AetherMenuBar.svg` to "Universal" slot
4. Set "Preserve Vector Data" to true

Then use in code:
```swift
Image("MenuBarIcon")
    .renderingMode(.template)
```

## Design Notes

**Color Palette:**
- Main Star: Linear gradient #0A84FF → #5E5CE6 (Apple Blue)
- Satellite Star: Linear gradient #80E0FF → #0A84FF (Bright Cyan)
- Background: #1C1C1E → #0A0A0C (Dark gray)

**Sizes:**
- App Icon: 1024x1024 (required for App Store)
- Menu Bar: 16pt (32px @2x) - template mode
- Dock: Multiple sizes generated automatically

**Icon Philosophy:**
"Tighter Gravitational Pull" - The satellite star is deliberately small and close to the main star,
creating a sense of energy spark rather than two separate objects.
