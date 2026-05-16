# Custom Schedulers

Schedulers execute normal flowgraph block tasks and async tasks spawned through the runtime. Most applications should use `Runtime::new()` and the default `SmolScheduler`; write a scheduler only when you are experimenting with placement, latency, or executor integration.

The scheduler trait is:

```rust
pub trait Scheduler: Clone + Send + 'static {
    #[cfg(not(target_arch = "wasm32"))]
    fn run_domain(
        &self,
        blocks: Vec<Box<dyn Block>>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, Box<dyn Block>)>>;

    fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T>;
}
```

`run_domain()` receives the normal send-capable blocks in a flowgraph. It must spawn each block, call `block.run(main_channel).await`, and return task handles that yield `(BlockId, Box<dyn Block>)`. The runtime waits for those tasks and restores the finished block objects into the returned flowgraph.

`spawn()` runs general async tasks on the scheduler. `Runtime::spawn()`, `Runtime::spawn_background()`, and control-plane internals use this method.

## Normal vs Local Work

Schedulers handle only the normal scheduling domain. The runtime manages local domains separately for:

- blocks added through `Flowgraph::add_local()`,
- blocks marked with `#[blocking]`.

That separation lets scheduler implementations assume that `run_domain()` receives send-capable block tasks. Blocking or thread-affine work should be placed in a local domain instead of being hidden inside the normal scheduler.

## Starting Point

Use the existing schedulers as templates:

- `SmolScheduler` is a compact general-purpose scheduler backed by `async_executor`.
- `FlowScheduler` shows deterministic block placement onto worker-local queues.

A minimal native scheduler usually needs:

- a clonable handle to an executor,
- worker thread lifecycle management,
- an implementation of `run_domain()` that spawns every block and returns its task,
- an implementation of `spawn()` for unrelated async tasks.

## Selecting a Scheduler

Construct the runtime with your scheduler:

```rust
use futuresdr::prelude::*;

let scheduler = MyScheduler::new();
let rt = Runtime::with_scheduler(scheduler);

let fg = Flowgraph::new();
rt.run(fg)?;
```

Custom schedulers should preserve the runtime contract: every spawned block task must eventually return its block object, even if the block exits because the flowgraph was stopped. If a worker thread panics, treat it as a runtime failure rather than silently dropping block state.
