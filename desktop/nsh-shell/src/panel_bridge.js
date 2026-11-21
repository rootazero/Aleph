function installAlephShellBridge(origin, prefix) {
  if (window !== window.top || location.origin !== origin) return;

  const documentId = Array.from(crypto.getRandomValues(new Uint8Array(16)),
    byte => byte.toString(16).padStart(2, "0")).join("");
  Object.defineProperty(window, "__alephShellBridge", {
    value: Object.freeze({
      document: documentId,
      reply(payload) {
        if (typeof payload === "string" && payload.length <= 16384) {
          window.nsh.postMessage(prefix + payload);
        }
      },
    }),
  });
}
