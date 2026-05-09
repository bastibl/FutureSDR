import init, { initSync, futuresdr_wasm_scheduler_worker_entry } from "./wasm-threaded.js";

self.onerror = (event) => {
  console.error("FutureSDR scheduler worker error", event.message, event.error);
};

self.onunhandledrejection = (event) => {
  console.error("FutureSDR scheduler worker unhandled rejection", event.reason);
};

self.onmessage = async (event) => {
  const data = event.data;
  if (!data || data.type !== "futuresdr-wasm-scheduler-init") {
    return;
  }

  try {
    console.info("FutureSDR scheduler worker init", data.worker_index);
    if (data.worker_index > 0) {
      await new Promise((resolve) => setTimeout(resolve, data.worker_index * 250));
    }
    const thread_stack_size = 1024 * 1024;
    console.info(
      "FutureSDR scheduler worker module",
      data.worker_index,
      data.module instanceof WebAssembly.Module,
      data.memory?.buffer instanceof SharedArrayBuffer,
    );
    const i32 = new Int32Array(data.memory.buffer);
    const base = data.thread_metadata_base;
    console.info(
      "FutureSDR scheduler worker guards before init",
      data.worker_index,
      base,
      i32[(base - 4) >> 2],
      i32[base >> 2],
      i32[(base + 4) >> 2],
    );
    console.info("FutureSDR scheduler worker before wasm-bindgen init", data.worker_index);
    if (data.module instanceof WebAssembly.Module) {
      initSync({ module: data.module, memory: data.memory, thread_stack_size });
    } else {
      await init({ module_or_path: data.module, memory: data.memory, thread_stack_size });
    }
    console.info("FutureSDR scheduler worker after wasm-bindgen init", data.worker_index);
    console.info("FutureSDR scheduler worker entering", data.worker_index);
    futuresdr_wasm_scheduler_worker_entry(data.executor_id, data.worker_index);
  } catch (e) {
    console.error("FutureSDR scheduler worker init failed", data.worker_index, e);
    throw e;
  }
};
