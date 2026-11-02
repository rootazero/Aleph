import { CLEANUP_STEPS } from "./native_project_protocol.mjs";

const DEVICE_LABELS = Object.freeze(["device-a", "device-b"]);

function requireResult(result, name) {
  if (result !== true) throw new Error(`cleanup_${name}`);
  return true;
}

export function createNativeCleanup({ shell, helper, devices, reaper, root, rootDir, nonce, gatewayUrl }) {
  if (!shell || typeof shell.terminate !== "function") throw new Error("cleanup_shell");
  if (!helper || typeof helper.stop !== "function") throw new Error("cleanup_helper");
  if (!Array.isArray(devices) || devices.length === 0 || devices.some((device) => !DEVICE_LABELS.includes(device))) {
    throw new Error("cleanup_devices");
  }
  if (new Set(devices).size !== devices.length) throw new Error("cleanup_devices");
  if (!reaper || typeof reaper.run !== "function") throw new Error("cleanup_reaper");
  if (!root || typeof root.remove !== "function") throw new Error("cleanup_root");
  if (typeof rootDir !== "string" || typeof nonce !== "string" || typeof gatewayUrl !== "string") throw new Error("cleanup_identity");

  return Object.freeze([
    async () => {
      const result = await shell.terminate();
      return result?.stopped === true && result?.ownedCmdlineProven === true;
    },
    async () => {
      const result = await helper.stop();
      return result?.stopped === true && result?.listenerClosed === true && result?.fixtureClosed === true;
    },
    async () => {
      for (const device of devices) requireResult(await helper.revokeExact(device), `revoke_${device}`);
      return true;
    },
    async () => {
      const result = reaper.run(rootDir, nonce, "delete-exact", gatewayUrl);
      return result?.ok === true && result?.absent === true;
    },
    async () => {
      const result = reaper.run(rootDir, nonce, "check-absent", gatewayUrl);
      return result?.ok === true && result?.absent === true;
    },
    async () => root.remove(rootDir) === true,
  ]);
}

export function cleanupStepNames() {
  return [...CLEANUP_STEPS];
}

export function validateCleanupResult(state) {
  return state.cleanup.length === CLEANUP_STEPS.length && state.cleanup.every((step, index) => step.name === CLEANUP_STEPS[index] && step.ok === true);
}
