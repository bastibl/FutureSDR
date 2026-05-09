import { initSync, futuresdr_wasm_scheduler_worker_entry } from "./wasm-threaded.js";

const THREAD_STACK_SIZE = 1024 * 1024;

self.onerror = (event) => {
  console.error("FutureSDR scheduler worker error", event.message, event.error);
};

self.onunhandledrejection = (event) => {
  console.error("FutureSDR scheduler worker unhandled rejection", event.reason);
};

self.onmessage = (event) => {
  const data = event.data;
  if (!data || data.type !== "futuresdr-wasm-scheduler-init") {
    return;
  }

  try {
    initSync({
      module: data.module,
      memory: data.memory,
      thread_stack_size: THREAD_STACK_SIZE,
    });
    futuresdr_wasm_scheduler_worker_entry(data.executor_id, data.worker_index);
  } catch (e) {
    console.error("FutureSDR scheduler worker init failed", data.worker_index, e);
    throw e;
  }
};
