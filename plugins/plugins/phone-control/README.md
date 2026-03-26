# Phone Control Plugin

Android phone control via ADB for the Aleph AI assistant.

## Prerequisites

1. **Android SDK Platform-Tools** installed with `adb` on your PATH
   - Download: https://developer.android.com/tools/releases/platform-tools
   - Verify: `adb version`

2. **Android device** connected via USB (or wireless ADB) with **USB debugging** enabled
   - Settings > Developer Options > USB Debugging

3. **Device authorized** — accept the RSA key prompt on the device when first connecting

## Tools

| Tool | Description |
|------|-------------|
| `phone_devices` | List connected devices |
| `phone_screenshot` | Capture the device screen |
| `phone_tap` | Tap at screen coordinates |
| `phone_swipe` | Swipe gesture between two points |
| `phone_type` | Type text input |
| `phone_shell` | Run a shell command on the device |
| `phone_install` | Install an APK |
| `phone_push` | Push a file to the device |
| `phone_pull` | Pull a file from the device |

## Usage

The plugin is loaded by the Aleph host automatically. Tools become available as AI-callable functions once the plugin starts.

```bash
# Manual test
echo '{"jsonrpc":"2.0","id":1,"method":"plugin.call","params":{"handler":"phoneDevices","arguments":{}}}' | node src/index.js
```
