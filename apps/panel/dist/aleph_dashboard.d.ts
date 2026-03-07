/* tslint:disable */
/* eslint-disable */
/**
 * The `ReadableStreamType` enum.
 *
 * *This API requires the following crate features to be activated: `ReadableStreamType`*
 */

type ReadableStreamType = "bytes";

export class IntoUnderlyingByteSource {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    cancel(): void;
    pull(controller: ReadableByteStreamController): Promise<any>;
    start(controller: ReadableByteStreamController): void;
    readonly autoAllocateChunkSize: number;
    readonly type: ReadableStreamType;
}

export class IntoUnderlyingSink {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    abort(reason: any): Promise<any>;
    close(): Promise<any>;
    write(chunk: any): Promise<any>;
}

export class IntoUnderlyingSource {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    cancel(): void;
    pull(controller: ReadableStreamDefaultController): Promise<any>;
}

/**
 * Initialize the Leptos application
 * This function is automatically called when the WASM module is loaded
 */
export function main(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly main: () => void;
    readonly __wbg_intounderlyingsink_free: (a: number, b: number) => void;
    readonly intounderlyingsink_abort: (a: number, b: any) => any;
    readonly intounderlyingsink_close: (a: number) => any;
    readonly intounderlyingsink_write: (a: number, b: any) => any;
    readonly __wbg_intounderlyingsource_free: (a: number, b: number) => void;
    readonly intounderlyingsource_cancel: (a: number) => void;
    readonly intounderlyingsource_pull: (a: number, b: any) => any;
    readonly __wbg_intounderlyingbytesource_free: (a: number, b: number) => void;
    readonly intounderlyingbytesource_autoAllocateChunkSize: (a: number) => number;
    readonly intounderlyingbytesource_cancel: (a: number) => void;
    readonly intounderlyingbytesource_pull: (a: number, b: any) => any;
    readonly intounderlyingbytesource_start: (a: number, b: any) => void;
    readonly intounderlyingbytesource_type: (a: number) => number;
    readonly wasm_bindgen__closure__destroy__hc6f4255a928ba91c: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h4d0b95e8cae015a5: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__hd425541e1c2d86a8: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__hbdfedb02060ee672: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h59df8b2169c4bc56: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__hfe045a872e70a2b1: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__hbe71710fe9fac899: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__ha5521178945b489c: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hd9589248c808767b: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h1ba8f7cc6d6f896c: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hc7838fcdca55a153: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h0e2b4f7fbe6a4c3d: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__he475b92ddd8d0c00: (a: number, b: number) => number;
    readonly wasm_bindgen__convert__closures_____invoke__h92fdcd19b7c3d3bf: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hd971fc654ddf32f5: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h3161cec36d5274bf: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
