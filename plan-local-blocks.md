# Local Blocks and Local Domains

## Current Model

FutureSDR now treats `Kernel` as the block-author implementation trait.
`SendKernel` is a send-capable marker implemented automatically for kernels
whose concrete value and returned kernel futures are `Send`.

This uses nightly return type notation:

```rust
pub trait SendKernel: Kernel + Send
where
    Self: Kernel<work(..): Send, init(..): Send, deinit(..): Send>,
{
}
```

On native targets, the normal scheduler still requires normal `Block::run()`
futures to be `Send`, so normal flowgraph blocks must satisfy
`SendKernel + SendKernelInterface`. Local domains require only
`Kernel + KernelInterface` and are run on a local executor through
`Runtime::run`.

`#[derive(Block)]` emits only `KernelInterface`. `SendKernelInterface` is a
blanket marker implemented automatically when RTN proves the generated
`KernelInterface` notification and message-handler futures are `Send` and the
block value itself is `Send`.

On wasm, the browser scheduler is a local executor. Normal flowgraph blocks are
therefore stored as `LocalWrappedKernel` and only require
`Kernel + KernelInterface`; the send-only wrapper/interface path is not compiled
for wasm.

There is still no `MaybeSend` abstraction. Sendability is expressed directly
through normal Rust bounds and RTN at the runtime boundaries. The root crate is
nightly-only while `return_type_notation` is unstable.

## Trait Layers

Send-capable traits:

- `SendKernel` as the RTN-backed marker over `Kernel`
- `SendBufferReader` / `SendBufferWriter`
- `SendCpuBufferReader` / `SendCpuBufferWriter`
- `SendInplaceReader` / `SendInplaceWriter`
- `SendCircuitWriter`
- `SendKernelInterface`

Local traits:

- `Kernel`
- `BufferReader` / `BufferWriter`
- `CpuBufferReader` / `CpuBufferWriter`
- `KernelInterface`

Send-capable buffer traits still blanket-implement their local counterparts. For
kernels, the direction is reversed: blocks implement `Kernel` and derive only
`KernelInterface`; the blanket `SendKernel` and `SendKernelInterface` impls
apply only when RTN proves the local futures are `Send`.

This means native normal blocks and normal buffers can be used in local domains.
Local-only blocks and buffers cannot be used in the native normal runtime unless
their concrete types also satisfy the normal send-capable bounds. On wasm, the
normal runtime is local and uses the local traits directly.

## SendKernel Futures

Blocks generic over local buffer traits can use ordinary `async fn work`.
With local-only buffers, the generated future may be non-`Send`; that is fine
for `Kernel` and simply prevents the blanket `SendKernel` impl from applying.

Native flowgraph entry points still reject those local-only instantiations
because `Flowgraph::add` and normal stream connections require `SendKernel`,
send-capable buffer traits, and `Send` where appropriate. Wasm flowgraph entry
points accept local instantiations.

No boxed futures are used for kernel methods.

`BufferReader` remains dyn-safe for stream connection/downcast plumbing, but
`notify_finished()` is a concrete-only RPITIT method. The runtime never calls it
through `dyn BufferReader`; the generated `KernelInterface` calls it on concrete
port fields.

## Local CPU Buffers

`LocalCpuReader<T>` and `LocalCpuWriter<T>` are concrete CPU buffer halves backed
by single-thread state (`Rc<RefCell<_>>`).

They implement local buffer traits directly:

- `BufferReader` / `BufferWriter`
- `CpuBufferReader` / `CpuBufferWriter`

They intentionally do not implement send-capable `SendBufferReader` /
`SendBufferWriter`, because their shared state is not `Send`.

## Local Domains and Connections

`Flowgraph::local_domain()` creates a local scheduling-domain builder:

```rust
let mut local = fg.local_domain();
let src = local.add(|| VectorSource::<u8, LocalCpuWriter<u8>>::new(vec![1, 2, 3]));
```

Local-domain blocks are stored as local block objects and executed with
`LocalWrappedKernel`. Normal flowgraph storage is isolated behind the
`flowgraph::normal_block` abstraction: native stores `WrappedKernel`/`BoxBlock`,
while wasm stores `LocalWrappedKernel`/`StoredLocalBlock`.

Implemented boundaries today:

- wasm normal flowgraphs run on the local path;
- local-to-local inside one local domain;
- local-to-normal through a normal/send buffer.

Still missing:

- normal-to-local;
- local-domain-to-local-domain;
- two-phase send-buffer connection API for domains that construct blocks on
  persistent domain threads.

## Verification Status

The current RTN model passes:

- `cargo +nightly check`
- `cargo +nightly check --lib --workspace --target wasm32-unknown-unknown --features=burn,audio,seify_dummy,wgpu`
- `cargo +nightly test --test local_buffer`
- `cargo +nightly test --test local_domain`
- `cargo +nightly run --example local-domain`
- `cargo +nightly test --workspace`
- `cargo +nightly clippy --all-targets --workspace --features=burn,vulkan,zeromq,audio,flow_scheduler,soapy,zynq,wgpu,seify_dummy -- -D warnings`
- `cargo +nightly clippy --lib --workspace --features=burn,audio,seify_dummy,wgpu --target wasm32-unknown-unknown -- -D warnings`
- `cargo +nightly fmt --all -- --check`

## Remaining Work

- Add normal-to-local and local-domain-to-local-domain boundary APIs.
- Add persistent domain-thread construction if local blocks must be created on
  their eventual execution thread.
- Incrementally migrate broader built-in blocks to local buffer trait bounds
  where that is useful for local-domain reuse.
