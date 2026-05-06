# Runtime Domains And Unified Graph Connections

## Summary

Refactor runtime execution around domains. Runtime<S, LS> owns one normal scheduler S and one or more local domains using scheduler type LS. A Flowgraph is created by
rt.flowgraph(), records block placement, and is run only by that runtime.

Native defaults to normal domain 0. Wasm uses Runtime<DummyScheduler, WasmLocalScheduler> and defaults to local domain 0.

## Runtime And Schedulers

- Runtime<S, LS> is generic over one normal scheduler type and one local scheduler type.
- All local domains use type LS, but each local domain owns its own LS instance.
- Scheduler has spawn and run_domain; no spawn_blocking.
- LocalScheduler runs local domains; native local schedulers construct local blocks on their own thread.
- Runtime owns a local scheduler factory to create fresh LS instances for explicit local domains and auto blocking domains.

## Flowgraph And Block Placement

- rt.flowgraph() replaces Flowgraph::new().
- Flowgraph is owned, not borrowing Runtime, but stamped with RuntimeId.
- BlockRef<K> is unified and stores hidden placement metadata.
- Native fg.add(block):
    - non-blocking block -> normal domain 0
    - #[blocking] block -> new flowgraph-scoped auto local domain
- Wasm fg.add(...) adds to local domain 0.
- fg.add_local(|| K::new()) adds to default local domain 0.
- fg.add_local_to(&domain, || K::new()) adds to an explicit local domain.
- Native local blocks are created from closures on the local domain thread, not moved there after construction.

## Connections

- There is one graph-level fg.stream(...) API for normal, local, blocking-local, and mixed-domain connections.
- connect!(fg, ...) always emits fg.stream(...), so ordinary graph construction stays simple.
- Native fg.stream(...) requires SendBufferWriter, because endpoints may run in different domains.
- Same-local-domain non-Send buffers use a local-domain setup API, e.g.:

local.stream(|| {
    connect!(a > b);
});

- That local setup runs on the local domain thread and only requires BufferWriter.

## Execution

- rt.run(fg) / rt.start_async(fg) validate RuntimeId.
- Runtime groups blocks by domain and starts each group on its scheduler.
- The graph coordinator manages init, termination, descriptions, message calls, and waits for all domain tasks.
- Normal and auto-blocking blocks can be returned to the finished flowgraph.
- Explicit native local blocks remain owned by their domain thread.
- Stopped-flowgraph access to explicit local blocks uses typed closures returning Send values.

## Buffer IDs

- Introduce globally unique BufferId.
- Stream edges should carry BufferId instead of relying only on flowgraph-local endpoint tuples.

## Tests

- Native fg.add() places non-blocking blocks in normal domain.
- Native fg.add() auto-places #[blocking] blocks in fresh local domains.
- Normal scheduler never receives blocking blocks.
- connect!(fg, ...) works across normal, blocking-local, and explicit local endpoints with Send buffers.
- Same-local-domain setup supports LocalCpuWriter / LocalCpuReader.
- Wasm fg.add() targets local domain 0 and normal-domain APIs are unavailable.
- Finished graphs can query explicit local blocks through with closures returning Send values.
