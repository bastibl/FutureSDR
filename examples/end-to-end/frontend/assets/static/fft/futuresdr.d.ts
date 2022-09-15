/* tslint:disable */
/* eslint-disable */
/**
* @param {string} id
* @param {string} url
* @param {number} block
* @param {number} callback
* @param {number} min
* @param {number} max
* @param {number} step
* @param {number} value
*/
export function add_slider_u32(id: string, url: string, block: number, callback: number, min: number, max: number, step: number, value: number): void;
/**
* @param {string} id
* @param {string} url
* @param {number} min
* @param {number} max
*/
export function add_freq(id: string, url: string, min: number, max: number): void;
/**
* @param {string} id
* @param {string} url
* @param {number} min
* @param {number} max
*/
export function add_time(id: string, url: string, min: number, max: number): void;
/**
* @param {string} id
*/
export function kitchen_sink(id: string): void;
/**
*/
export function futuresdr_init(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly add_slider_u32: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => void;
  readonly add_freq: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly add_time: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly kitchen_sink: (a: number, b: number) => void;
  readonly futuresdr_init: () => void;
  readonly __wbindgen_malloc: (a: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number) => number;
  readonly __wbindgen_export_2: WebAssembly.Table;
  readonly _dyn_core__ops__function__Fn__A____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__h74d87bf120bd4638: (a: number, b: number, c: number) => void;
  readonly _dyn_core__ops__function__Fn__A____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__h421c1c7997fe7fdd: (a: number, b: number, c: number) => void;
  readonly _dyn_core__ops__function__FnMut_____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__h7a3ac4c8fb000205: (a: number, b: number) => void;
  readonly _dyn_core__ops__function__FnMut_____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__h6cd69a69a6ac8403: (a: number, b: number) => void;
  readonly _dyn_core__ops__function__FnMut__A____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__h5f43faab0b1e8c82: (a: number, b: number, c: number) => void;
  readonly _dyn_core__ops__function__FnMut__A____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__hf3da9a06832ad6a5: (a: number, b: number, c: number) => void;
  readonly __wbindgen_free: (a: number, b: number) => void;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __wbindgen_start: () => void;
}

/**
* Synchronously compiles the given `bytes` and instantiates the WebAssembly module.
*
* @param {BufferSource} bytes
*
* @returns {InitOutput}
*/
export function initSync(bytes: BufferSource): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {InitInput | Promise<InitInput>} module_or_path
*
* @returns {Promise<InitOutput>}
*/
export default function init (module_or_path?: InitInput | Promise<InitInput>): Promise<InitOutput>;
