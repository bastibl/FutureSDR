import init, { futuresdr_wasm_scheduler_worker_entry } from "./wasm-threaded.js";

self.onmessage = async (event) => {
  const data = event.data;
  if (!data || data.type !== "futuresdr-wasm-scheduler-init") {
    return;
  }

  await init(data.module, data.memory);
  futuresdr_wasm_scheduler_worker_entry(data.executor_id, data.worker_index);
};
