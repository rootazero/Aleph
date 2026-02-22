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
    readonly __wbg_intounderlyingbytesource_free: (a: number, b: number) => void;
    readonly intounderlyingbytesource_autoAllocateChunkSize: (a: number) => number;
    readonly intounderlyingbytesource_cancel: (a: number) => void;
    readonly intounderlyingbytesource_pull: (a: number, b: any) => any;
    readonly intounderlyingbytesource_start: (a: number, b: any) => void;
    readonly intounderlyingbytesource_type: (a: number) => number;
    readonly __wbg_intounderlyingsink_free: (a: number, b: number) => void;
    readonly __wbg_intounderlyingsource_free: (a: number, b: number) => void;
    readonly intounderlyingsink_abort: (a: number, b: any) => any;
    readonly intounderlyingsink_close: (a: number) => any;
    readonly intounderlyingsink_write: (a: number, b: any) => any;
    readonly intounderlyingsource_cancel: (a: number) => void;
    readonly intounderlyingsource_pull: (a: number, b: any) => any;
    readonly wasm_bindgen__closure__destroy__hccdd22bf2e98c404: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h67664057e0b663d5: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h09d6d1636a24af53: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h5b821070a28f80b5: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h73880085a057feae: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h6e021d5227f87c04: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h7beb51e6bbb119f6: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h0824da44d9da4b52: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h5f109bfeb3502067: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h38dcb847c9018752: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hb4a63da9ffae8c94: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h1741a46830e7c6f0: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__he9eaff412718f49e: (a: number, b: number) => void;
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
