// Same-origin trusted/verified modules only; this ABI is not a sandbox.
const failure = (code, cause) => Object.assign(new Error(code, { cause }), { code });
const requireSync = (value) => {
    if (!value || typeof value.then !== 'function') return;
    // This ABI is synchronous after initialization. Observe rejected promises
    // even when rejecting the result so malformed modules cannot leak rejections.
    Promise.resolve(value).catch(() => {});
    throw Error('async ABI result');
};

const parseIntegrity = (integrity) => {
    const match = /^sha256-([A-Za-z0-9+/]{43}=)$/.exec(integrity);
    if (!match) throw failure('InvalidAssetIntegrity');
    try {
        const bytes = Uint8Array.from(atob(match[1]), (character) => character.charCodeAt(0));
        if (bytes.length !== 32) throw Error('invalid SHA-256 length');
        return bytes;
    } catch (cause) {
        throw failure('InvalidAssetIntegrity', cause);
    }
};

const sameBytes = (left, right) =>
    left.length === right.length && left.every((value, index) => value === right[index]);

const importVerifiedModule = async (entry, integrity) => {
    const expected = parseIntegrity(integrity);
    const url = new URL(entry, import.meta.url).href;
    let response;
    try {
        response = await fetch(url, { credentials: 'same-origin', redirect: 'error' });
    } catch (cause) {
        throw failure('AssetFetchFailed', cause);
    }
    if (!response.ok) throw failure('AssetFetchFailed', Error(`HTTP ${response.status}`));

    let bytes;
    try {
        bytes = new Uint8Array(await response.arrayBuffer());
    } catch (cause) {
        throw failure('AssetFetchFailed', cause);
    }
    const actual = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
    if (!sameBytes(actual, expected)) throw failure('IntegrityMismatch');

    const blobUrl = URL.createObjectURL(new Blob([bytes], { type: 'text/javascript' }));
    try {
        const module = await import(blobUrl);
        if (typeof module.default === 'function') await module.default();
        return module;
    } finally {
        URL.revokeObjectURL(blobUrl);
    }
};

export async function loadPanelModule(entry, integrity = null) {
    let module;
    try {
        module = integrity === null ? await import(new URL(entry, import.meta.url).href) : await importVerifiedModule(entry, integrity);
        if (integrity === null && typeof module.default === 'function') await module.default();
    } catch (cause) {
        if (cause?.code === 'AssetFetchFailed' || cause?.code === 'InvalidAssetIntegrity' || cause?.code === 'IntegrityMismatch') throw cause;
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
