// adb.js — ADB wrapper for phone-control plugin

const { execSync } = require("child_process");
const path = require("path");

const EXEC_OPTIONS = { encoding: "utf-8", timeout: 30000 };

/**
 * Verify that adb is available on the system PATH.
 * Throws a descriptive error if not found.
 */
function checkAdb() {
  try {
    execSync("adb version", { ...EXEC_OPTIONS, stdio: "pipe" });
  } catch {
    throw new Error(
      "adb not found. Install Android SDK Platform-Tools and ensure 'adb' is on your PATH. " +
        "See https://developer.android.com/tools/releases/platform-tools",
    );
  }
}

/**
 * Execute an adb command and return stdout.
 *
 * @param {string[]} args - Arguments to pass to adb
 * @param {string} [device] - Device serial to target (-s flag)
 * @returns {string} stdout output
 */
function adbExec(args, device) {
  const parts = ["adb"];
  if (device) {
    parts.push("-s", device);
  }
  parts.push(...args);

  const cmd = parts
    .map((a) => {
      // Quote arguments that contain spaces or shell-special characters
      if (/["\s$`\\!#&|;()<>]/.test(a)) {
        return `"${a.replace(/"/g, '\\"')}"`;
      }
      return a;
    })
    .join(" ");

  return execSync(cmd, { ...EXEC_OPTIONS, stdio: "pipe" }).trim();
}

/**
 * List connected devices.
 * @returns {{ serial: string, status: string }[]}
 */
function listDevices() {
  checkAdb();
  const output = adbExec(["devices"]);
  const lines = output.split("\n").slice(1); // skip header
  const devices = [];
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const [serial, status] = trimmed.split(/\s+/);
    if (serial) {
      devices.push({ serial, status: status || "unknown" });
    }
  }
  return devices;
}

/**
 * Take a screenshot and save it locally.
 *
 * @param {string} [device] - Device serial
 * @param {string} [outputPath] - Local path to save the screenshot
 * @returns {{ path: string }} Result with the saved file path
 */
function screenshot(device, outputPath) {
  checkAdb();
  const remotePath = "/sdcard/aleph_screenshot.png";
  const localPath = outputPath || "/tmp/phone_screenshot.png";

  // Capture screenshot on device
  adbExec(["shell", "screencap", "-p", remotePath], device);
  // Pull to local
  adbExec(["pull", remotePath, localPath], device);
  // Clean up remote file
  adbExec(["shell", "rm", "-f", remotePath], device);

  return { path: localPath };
}

/**
 * Tap a point on the screen.
 */
function tap(x, y, device) {
  checkAdb();
  return adbExec(["shell", "input", "tap", String(x), String(y)], device);
}

/**
 * Perform a swipe gesture.
 */
function swipe(x1, y1, x2, y2, duration, device) {
  checkAdb();
  return adbExec(
    [
      "shell",
      "input",
      "swipe",
      String(x1),
      String(y1),
      String(x2),
      String(y2),
      String(duration),
    ],
    device,
  );
}

/**
 * Type text on the device.
 * Special characters are escaped for adb shell input text.
 */
function typeText(text, device) {
  checkAdb();
  // adb shell input text requires spaces as %s and special chars escaped
  const escaped = text
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/'/g, "\\'")
    .replace(/ /g, "%s")
    .replace(/&/g, "\\&")
    .replace(/</g, "\\<")
    .replace(/>/g, "\\>")
    .replace(/\(/g, "\\(")
    .replace(/\)/g, "\\)")
    .replace(/;/g, "\\;")
    .replace(/\|/g, "\\|");

  return adbExec(["shell", "input", "text", `"${escaped}"`], device);
}

/**
 * Execute a shell command on the device.
 */
function shellCmd(cmd, device) {
  checkAdb();
  return adbExec(["shell", cmd], device);
}

/**
 * Install an APK.
 */
function install(apkPath, device) {
  checkAdb();
  return adbExec(["install", apkPath], device);
}

/**
 * Push a file to the device.
 */
function push(localPath, remotePath, device) {
  checkAdb();
  return adbExec(["push", localPath, remotePath], device);
}

/**
 * Pull a file from the device.
 */
function pull(remotePath, localPath, device) {
  checkAdb();
  const dest = localPath || path.basename(remotePath);
  return adbExec(["pull", remotePath, dest], device);
}

module.exports = {
  checkAdb,
  adbExec,
  listDevices,
  screenshot,
  tap,
  swipe,
  typeText,
  shellCmd,
  install,
  push,
  pull,
};
