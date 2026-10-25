// Same-origin trusted/verified modules only; this ABI is not a sandbox.
const failure = (code, cause) => Object.assign(new Error(code, { cause }), { code });
const requireSync = (value) => {
    if (!value || typeof value.then !== 'function') return;
    // This ABI is synchronous after initialization. Observe rejected promises
    // even when rejecting the result so malformed modules cannot leak rejections.
    Promise.resolve(value).catch(() => {});
    throw Error('async ABI result');
};

export async function loadPanelModule(entry) {
    let module;
    try {
        // Resolve relative entries against the host page, not a wasm-bindgen snippet.
        module = await import(new URL(entry, globalThis.document?.baseURI ?? import.meta.url).href);
        if (typeof module.default === 'function') await module.default();
    } catch (cause) {
        throw failure('ImportFailed', cause);
    }

    let descriptorJson;
    try {
        if (typeof module.describe !== 'function' || typeof module.mount !== 'function') throw Error('missing ABI');
        const descriptor = module.describe();
        requireSync(descriptor);
        if (!descriptor || typeof descriptor !== 'object' || Array.isArray(descriptor)) throw Error('invalid descriptor');
        descriptorJson = JSON.stringify(descriptor);
        if (typeof descriptorJson !== 'string') throw Error('invalid descriptor JSON');
    } catch (cause) {
        throw failure('InvalidDescriptor', cause);
    }

    return {
        descriptorJson,
        mount(sidebarRoot, contentRoot, sdk) {
            let handle;
            try {
                handle = module.mount(sidebarRoot, contentRoot, sdk);
            } catch (cause) {
                throw failure('MountFailed', cause);
            }
            let unmount;
            try {
                requireSync(handle);
                unmount = handle?.unmount;
                if (typeof unmount !== 'function') throw Error('invalid mounted handle');
            } catch (cause) {
                throw failure('InvalidDescriptor', cause);
            }
            let disposed = false;
            return {
                unmount() {
                    if (disposed) return;
                    disposed = true;
                    try {
                        requireSync(unmount.call(handle));
                    } catch (cause) {
                        throw failure('UnmountFailed', cause);
                    }
                },
            };
        },
    };
}
