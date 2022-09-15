/* tslint:disable */
/* eslint-disable */
/**
* @returns {Float32Array}
*/
export function return_get_samples(): Float32Array;
/**
* @returns {Promise<void>}
*/
export function run_fg(): Promise<void>;
/**
* @param {Int8Array} s
* @returns {Promise<boolean>}
*/
export function push_samples(s: Int8Array): Promise<boolean>;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly return_get_samples: () => number;
  readonly run_fg: () => number;
  readonly push_samples: (a: number, b: number) => number;
  readonly __wbindgen_export_0: WebAssembly.Table;
  readonly _dyn_core__ops__function__FnMut__A____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__hb60990723a42bfe3: (a: number, b: number, c: number) => void;
  readonly __wbindgen_free: (a: number, b: number) => void;
  readonly __wbindgen_malloc: (a: number) => number;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly wasm_bindgen__convert__closures__invoke2_mut__h17f6ca5a16113e2b: (a: number, b: number, c: number, d: number) => void;
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
