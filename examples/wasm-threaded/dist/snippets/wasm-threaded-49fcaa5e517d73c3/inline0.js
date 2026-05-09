
export function futuresdr_is_scheduler_worker() {
  const isWorker = globalThis.__futuresdr_scheduler_worker === true;
  console.info("FutureSDR Rust main", isWorker ? "scheduler-worker" : "browser-main");
  return isWorker;
}
