#[cfg(not(target_arch = "wasm32"))]
use futures::channel::oneshot;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::runtime::BlockId;
use crate::runtime::BlockPortCtx;
use crate::runtime::Error;
use crate::runtime::FlowgraphId;
use crate::runtime::PortId;
use crate::runtime::Result;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::block::Block;
use crate::runtime::block::BlockObject;
#[cfg(target_arch = "wasm32")]
use crate::runtime::block::LocalBlock;
use crate::runtime::buffer::BufferReader;
use crate::runtime::buffer::BufferWriter;
use crate::runtime::buffer::CircuitWriter;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::buffer::SendBufferWriter;
use crate::runtime::dev::BlockInbox;
use crate::runtime::dev::BlockMeta;
use crate::runtime::dev::Kernel;
use crate::runtime::dev::LocalKernel;
use crate::runtime::dev::SendKernel;
use crate::runtime::kernel_interface::KernelInterface;
use crate::runtime::kernel_interface::LocalKernelInterface;
use crate::runtime::kernel_interface::SendKernelInterface;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::local_domain::LocalBlockBuilder;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::local_domain::LocalDomainRuntime;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::local_domain::LocalDomainState;
use crate::runtime::wrapped_kernel::WrappedKernel;
use crate::runtime::wrapped_kernel::WrappedLocalKernel;

static NEXT_FLOWGRAPH_ID: AtomicUsize = AtomicUsize::new(0);

/// Shared typed access to a block stored inside a [`Flowgraph`].
///
/// The guard dereferences to the block's kernel type and also exposes runtime
/// metadata such as the block id and instance name. It is only available before
/// the flowgraph is moved into a running [`Runtime`](crate::runtime::Runtime).
pub struct TypedBlockGuard<'a, K> {
    id: BlockId,
    meta: &'a BlockMeta,
    kernel: &'a K,
}

/// Mutable typed access to a block stored inside a [`Flowgraph`].
///
/// The guard dereferences to the block's kernel type and can be used to update
/// block state or metadata before the flowgraph is started.
pub struct TypedBlockGuardMut<'a, K> {
    id: BlockId,
    meta: &'a mut BlockMeta,
    kernel: &'a mut K,
}

impl<K> TypedBlockGuard<'_, K> {
    /// Get the block id.
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// Get block metadata.
    pub fn meta(&self) -> &BlockMeta {
        self.meta
    }

    /// Get the block instance name.
    pub fn instance_name(&self) -> Option<&str> {
        self.meta.instance_name()
    }
}

impl<K> Deref for TypedBlockGuard<'_, K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        self.kernel
    }
}

impl<K> TypedBlockGuardMut<'_, K> {
    /// Get the block id.
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// Get block metadata.
    pub fn meta(&self) -> &BlockMeta {
        self.meta
    }

    /// Mutably access block metadata.
    pub fn meta_mut(&mut self) -> &mut BlockMeta {
        self.meta
    }

    /// Get the block instance name.
    pub fn instance_name(&self) -> Option<&str> {
        self.meta.instance_name()
    }

    /// Set the block instance name.
    pub fn set_instance_name(&mut self, name: &str) {
        self.meta.set_instance_name(name);
    }
}

impl<K> Deref for TypedBlockGuardMut<'_, K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        self.kernel
    }
}

impl<K> DerefMut for TypedBlockGuardMut<'_, K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.kernel
    }
}

/// Typed reference to a block that was added to a [`Flowgraph`].
///
/// `BlockRef` is a lightweight identifier that preserves the Rust kernel type.
/// The block itself remains owned by the [`Flowgraph`] and can only be accessed
/// together with that flowgraph before execution starts.
///
/// ```
/// use futuresdr::blocks::NullSink;
/// use futuresdr::prelude::*;
///
/// let mut fg = Flowgraph::new();
/// let snk = fg.add(NullSink::<u8>::new());
///
/// assert_eq!(snk.id(), snk.get(&fg)?.id());
/// # Ok::<(), futuresdr::runtime::Error>(())
/// ```
pub struct BlockRef<K> {
    id: BlockId,
    flowgraph_id: FlowgraphId,
    placement: BlockPlacement,
    _marker: PhantomData<fn() -> K>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum BlockPlacement {
    Normal {
        domain_id: usize,
    },
    #[cfg(not(target_arch = "wasm32"))]
    Local {
        domain_id: usize,
        local_id: usize,
        kind: LocalBlockKind,
    },
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum LocalBlockKind {
    Kernel,
    LocalKernel,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct LocalEndpoint {
    block_id: BlockId,
    domain_id: usize,
    local_id: usize,
    kind: LocalBlockKind,
}

#[cfg(not(target_arch = "wasm32"))]
impl LocalEndpoint {
    fn new(block_id: BlockId, domain_id: usize, local_id: usize, kind: LocalBlockKind) -> Self {
        Self {
            block_id,
            domain_id,
            local_id,
            kind,
        }
    }
}

/// Type-erased stream edge between two stream ports.
#[derive(Debug, Clone)]
pub(crate) struct StreamEdge {
    pub(crate) src_block: BlockId,
    pub(crate) src_port: PortId,
    pub(crate) dst_block: BlockId,
    pub(crate) dst_port: PortId,
}

impl StreamEdge {
    pub(crate) fn endpoints(&self) -> (BlockId, PortId, BlockId, PortId) {
        (
            self.src_block,
            self.src_port.clone(),
            self.dst_block,
            self.dst_port.clone(),
        )
    }
}

/// Handle for a local scheduling domain inside a [`Flowgraph`].
///
/// Local domains run their blocks on a dedicated single-thread executor. They
/// are used for blocks or buffers that are not `Send`, and for blocks marked
/// as blocking. Stream connections with local-only buffers can only connect
/// blocks inside the same local domain.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LocalDomain {
    flowgraph_id: FlowgraphId,
    domain_id: usize,
}

#[cfg(target_arch = "wasm32")]
type WasmBlockTask = crate::runtime::scheduler::Task<(BlockId, Box<dyn LocalBlock>)>;

#[cfg(not(target_arch = "wasm32"))]
type StoredBlock = dyn Block;
#[cfg(target_arch = "wasm32")]
type StoredBlock = dyn LocalBlock;

pub(crate) struct BlockEntry {
    block: Option<Box<StoredBlock>>,
    placement: BlockPlacement,
    inbox: Option<BlockInbox>,
    message_inputs: Option<&'static [&'static str]>,
}

impl BlockEntry {
    fn empty(placement: BlockPlacement) -> Self {
        Self {
            block: None,
            placement,
            inbox: None,
            message_inputs: None,
        }
    }

    fn with_block(
        block: Box<StoredBlock>,
        placement: BlockPlacement,
        inbox: BlockInbox,
        message_inputs: &'static [&'static str],
    ) -> Self {
        Self {
            block: Some(block),
            placement,
            inbox: Some(inbox),
            message_inputs: Some(message_inputs),
        }
    }
}

impl<K> BlockRef<K> {
    /// Get the block id.
    pub fn id(&self) -> BlockId {
        self.id
    }
}

impl<K: 'static> BlockRef<K> {
    /// Get typed shared access to the block stored in the given [`Flowgraph`].
    ///
    /// This is a convenience wrapper around [`Flowgraph::block`]. It can only
    /// access a block while the flowgraph owns its block instances, i.e. before
    /// startup or after a running flowgraph has returned the finished graph.
    pub fn get<'a>(&self, fg: &'a Flowgraph) -> Result<TypedBlockGuard<'a, K>, Error> {
        fg.block(self)
    }

    /// Access the typed block through the given [`Flowgraph`].
    ///
    /// Native local-domain blocks are accessed by running the closure on the
    /// local-domain thread. This keeps non-`Send` block state confined to its
    /// owning domain.
    pub fn with<R>(
        &self,
        fg: &Flowgraph,
        f: impl FnOnce(&K) -> R + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        fg.validate_block_ref(self)?;
        match self.placement {
            BlockPlacement::Normal { .. } => {
                let block = fg.block(self)?;
                Ok(f(&block))
            }
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local {
                domain_id,
                local_id,
                kind,
                ..
            } => {
                let domain = fg
                    .local_domains
                    .get(domain_id)
                    .ok_or(Error::InvalidBlock(self.id))?;
                if domain.is_running() {
                    return Err(Error::LockError);
                }
                let block_id = self.id;
                domain.exec(move |state| {
                    Ok(f(Flowgraph::local_state_kernel_ref(
                        state, local_id, block_id, kind,
                    )?))
                })
            }
        }
    }

    /// Mutably access the typed block through the given [`Flowgraph`].
    ///
    /// Native local-domain blocks are accessed by running the closure on the
    /// local-domain thread. This requires the flowgraph to be stopped; running
    /// local-domain blocks cannot be borrowed mutably through the construction
    /// API.
    pub fn with_mut<R>(
        &self,
        fg: &mut Flowgraph,
        f: impl FnOnce(&mut K) -> R + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        fg.validate_block_ref(self)?;
        match self.placement {
            BlockPlacement::Normal { .. } => {
                let mut block = fg.block_mut(self)?;
                Ok(f(&mut block))
            }
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local {
                domain_id,
                local_id,
                kind,
                ..
            } => {
                let domain = fg
                    .local_domains
                    .get(domain_id)
                    .ok_or(Error::InvalidBlock(self.id))?;
                if domain.is_running() {
                    return Err(Error::LockError);
                }
                let block_id = self.id;
                domain.exec(move |state| {
                    Ok(f(Flowgraph::local_state_kernel_mut(
                        state, local_id, block_id, kind,
                    )?))
                })
            }
        }
    }
}

impl<K> Copy for BlockRef<K> {}
impl<K> Clone for BlockRef<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Debug for BlockRef<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockRef")
            .field("id", &self.id)
            .field("flowgraph_id", &self.flowgraph_id)
            .field("placement", &self.placement)
            .field("type_name", &std::any::type_name::<K>())
            .finish()
    }
}

impl<K> From<BlockRef<K>> for BlockId {
    fn from(value: BlockRef<K>) -> Self {
        value.id
    }
}

impl<K> From<&BlockRef<K>> for BlockId {
    fn from(value: &BlockRef<K>) -> Self {
        value.id
    }
}

/// A directed graph of blocks and their stream/message connections.
///
/// A [`Flowgraph`] owns the blocks until it is passed to a
/// [`Runtime`](crate::runtime::Runtime). It is typically built with the
/// [`connect`](crate::runtime::macros::connect) macro, which adds blocks and
/// wires their default or named ports in one step.
///
/// ```
/// use anyhow::Result;
/// use futuresdr::blocks::Head;
/// use futuresdr::blocks::NullSink;
/// use futuresdr::blocks::NullSource;
/// use futuresdr::prelude::*;
///
/// fn main() -> Result<()> {
///     let mut fg = Flowgraph::new();
///
///     let src = NullSource::<u8>::new();
///     let head = Head::<u8>::new(1234);
///     let snk = NullSink::<u8>::new();
///
///     connect!(fg, src > head > snk);
///     Runtime::new().run(fg)?;
///
///     Ok(())
/// }
/// ```
pub struct Flowgraph {
    pub(crate) id: FlowgraphId,
    pub(crate) blocks: Vec<BlockEntry>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) local_domains: Vec<LocalDomainRuntime>,
    pub(crate) stream_edges: Vec<StreamEdge>,
    pub(crate) message_edges: Vec<(BlockId, PortId, BlockId, PortId)>,
}

impl Flowgraph {
    /// Create an empty [`Flowgraph`].
    pub fn new() -> Flowgraph {
        Flowgraph {
            id: FlowgraphId(NEXT_FLOWGRAPH_ID.fetch_add(1, Ordering::Relaxed)),
            blocks: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            local_domains: Vec::new(),
            stream_edges: vec![],
            message_edges: vec![],
        }
    }

    /// Create a local scheduling domain.
    ///
    /// Add non-`Send` or explicitly local blocks to this domain with
    /// [`Flowgraph::add_local`]. Blocks in one local domain can use local-only
    /// stream buffers with [`Flowgraph::stream_local`]. Normal send-capable
    /// stream buffers may connect local-domain blocks to normal blocks.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn local_domain(&mut self) -> LocalDomain {
        let domain_id = self.local_domains.len();
        self.local_domains.push(LocalDomainRuntime::new());
        LocalDomain {
            flowgraph_id: self.id,
            domain_id,
        }
    }

    /// Add a block and return a typed reference to it.
    ///
    /// The returned [`BlockRef`] can be used for explicit typed connections or
    /// for inspecting/mutating the block before the flowgraph is started. Blocks
    /// marked as blocking are placed in an internal local domain so their async
    /// API may perform blocking work without occupying a normal scheduler worker.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add<K>(&mut self, block: K) -> BlockRef<K>
    where
        K: SendKernel + SendKernelInterface + 'static,
    {
        let block_id = BlockId(self.blocks.len());
        let mut b = WrappedKernel::new(block, block_id);
        let block_name = <K as KernelInterface>::type_name();
        b.meta
            .set_instance_name(format!("{}-{}", block_name, block_id.0));
        if <K as KernelInterface>::is_blocking() {
            let domain_id = self.local_domains.len();
            self.local_domains.push(LocalDomainRuntime::new());
            self.add_local_block_builder(
                domain_id,
                LocalBlockKind::Kernel,
                <K as KernelInterface>::message_inputs(),
                move |_| Box::new(move || Box::new(b)),
                "failed to insert blocking block into local domain",
            )
        } else {
            let inbox = b.inbox();
            self.add_normal_block(Box::new(b), inbox, <K as KernelInterface>::message_inputs())
        }
    }

    /// Add a block and return a typed reference to it.
    ///
    /// On WASM, the browser executor is single-threaded, so ordinary flowgraph
    /// blocks are stored in the local block representation.
    #[cfg(target_arch = "wasm32")]
    pub fn add<K>(&mut self, block: K) -> BlockRef<K>
    where
        K: SendKernel + SendKernelInterface + 'static,
    {
        let block_id = BlockId(self.blocks.len());
        let mut b = WrappedKernel::new(block, block_id);
        let block_name = <K as KernelInterface>::type_name();
        b.meta
            .set_instance_name(format!("{}-{}", block_name, block_id.0));
        let inbox = b.inbox();
        self.add_normal_block(Box::new(b), inbox, <K as KernelInterface>::message_inputs())
    }

    /// Add a block to the local runtime path and return a typed reference to it.
    #[cfg(target_arch = "wasm32")]
    pub fn add_local<K>(&mut self, block: K) -> BlockRef<K>
    where
        K: crate::runtime::__private::AddLocal + 'static,
    {
        crate::runtime::__private::AddLocal::add_local(block, self)
    }

    #[cfg(target_arch = "wasm32")]
    #[doc(hidden)]
    pub fn __add_local_from_kernel<K>(&mut self, block: K) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        let block_id = BlockId(self.blocks.len());
        let mut b = WrappedKernel::new(block, block_id);
        let block_name = <K as KernelInterface>::type_name();
        b.meta
            .set_instance_name(format!("{}-{}", block_name, block_id.0));
        let inbox = b.inbox();
        self.add_normal_block(Box::new(b), inbox, <K as KernelInterface>::message_inputs())
    }

    #[cfg(target_arch = "wasm32")]
    #[doc(hidden)]
    pub fn __add_local_from_local_kernel<K>(&mut self, block: K) -> BlockRef<K>
    where
        K: LocalKernel + LocalKernelInterface + 'static,
    {
        let block_id = BlockId(self.blocks.len());
        let mut b = WrappedLocalKernel::new(block, block_id);
        let block_name = <K as LocalKernelInterface>::type_name();
        b.meta
            .set_instance_name(format!("{}-{}", block_name, block_id.0));
        let inbox = b.inbox();
        self.add_normal_block(
            Box::new(b),
            inbox,
            <K as LocalKernelInterface>::message_inputs(),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn reserve_block_id(&mut self, placement: BlockPlacement) -> BlockId {
        let block_id = BlockId(self.blocks.len());
        self.blocks.push(BlockEntry::empty(placement));
        block_id
    }

    fn block_ref<K>(&self, block_id: BlockId, placement: BlockPlacement) -> BlockRef<K> {
        BlockRef {
            id: block_id,
            flowgraph_id: self.id,
            placement,
            _marker: PhantomData,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_normal_block<K>(
        &mut self,
        block: Box<dyn Block>,
        inbox: BlockInbox,
        message_inputs: &'static [&'static str],
    ) -> BlockRef<K> {
        let block_id = BlockId(self.blocks.len());
        let placement = BlockPlacement::Normal { domain_id: 0 };
        self.blocks
            .push(BlockEntry::with_block(block, placement, inbox, message_inputs));
        self.block_ref(block_id, placement)
    }

    #[cfg(target_arch = "wasm32")]
    fn add_normal_block<K>(
        &mut self,
        block: Box<dyn LocalBlock>,
        inbox: BlockInbox,
        message_inputs: &'static [&'static str],
    ) -> BlockRef<K> {
        let block_id = BlockId(self.blocks.len());
        let placement = BlockPlacement::Normal { domain_id: 0 };
        self.blocks
            .push(BlockEntry::with_block(block, placement, inbox, message_inputs));
        self.block_ref(block_id, placement)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_local_block_builder<K>(
        &mut self,
        domain_id: usize,
        kind: LocalBlockKind,
        message_inputs: &'static [&'static str],
        builder: impl FnOnce(BlockId) -> LocalBlockBuilder,
        expect_msg: &str,
    ) -> BlockRef<K> {
        let local_id = self.local_domains[domain_id].reserve_block();
        let placement = BlockPlacement::Local {
            domain_id,
            local_id,
            kind,
        };
        let block_id = self.reserve_block_id(placement);
        let inbox = self.local_domains[domain_id]
            .build(local_id, builder(block_id))
            .expect(expect_msg);
        let entry = &mut self.blocks[block_id.0];
        entry.inbox = Some(inbox);
        entry.message_inputs = Some(message_inputs);
        self.block_ref(block_id, placement)
    }

    /// Add a block to a local domain.
    ///
    /// The closure is executed on the local-domain thread, so it may construct
    /// non-`Send` state that never leaves that thread. Use this for blocks
    /// derived with `LocalBlock`, for non-`Send` buffers, or for integrations
    /// that must remain thread-affine.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_local<K>(
        &mut self,
        domain: LocalDomain,
        block: impl FnOnce() -> K + Send + 'static,
    ) -> BlockRef<K>
    where
        K: crate::runtime::__private::AddLocal + 'static,
    {
        crate::runtime::__private::AddLocal::add_local(block, self, domain)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn __add_local_from_kernel<K>(
        &mut self,
        domain: LocalDomain,
        block: impl FnOnce() -> K + Send + 'static,
    ) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        let domain_id = self
            .validate_local_domain(domain)
            .expect("local domain belongs to another flowgraph");
        self.add_kernel_to_domain(domain_id, block)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn __add_local_from_local_kernel<K>(
        &mut self,
        domain: LocalDomain,
        block: impl FnOnce() -> K + Send + 'static,
    ) -> BlockRef<K>
    where
        K: LocalKernel + LocalKernelInterface + 'static,
    {
        let domain_id = self
            .validate_local_domain(domain)
            .expect("local domain belongs to another flowgraph");
        self.add_local_to_domain(domain_id, block)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_kernel_to_domain<K>(
        &mut self,
        domain_id: usize,
        block: impl FnOnce() -> K + Send + 'static,
    ) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        self.add_local_block_builder(
            domain_id,
            LocalBlockKind::Kernel,
            <K as KernelInterface>::message_inputs(),
            move |block_id| {
                Box::new(move || {
                    let mut b = WrappedKernel::new(block(), block_id);
                    let block_name = <K as KernelInterface>::type_name();
                    b.meta
                        .set_instance_name(format!("{}-{}", block_name, block_id.0));
                    Box::new(b)
                })
            },
            "failed to build block in local domain",
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_local_to_domain<K>(
        &mut self,
        domain_id: usize,
        block: impl FnOnce() -> K + Send + 'static,
    ) -> BlockRef<K>
    where
        K: LocalKernel + LocalKernelInterface + 'static,
    {
        self.add_local_block_builder(
            domain_id,
            LocalBlockKind::LocalKernel,
            <K as LocalKernelInterface>::message_inputs(),
            move |block_id| {
                Box::new(move || {
                    let mut b = WrappedLocalKernel::new(block(), block_id);
                    let block_name = <K as LocalKernelInterface>::type_name();
                    b.meta
                        .set_instance_name(format!("{}-{}", block_name, block_id.0));
                    Box::new(b)
                })
            },
            "failed to build local block in local domain",
        )
    }

    pub(crate) fn validate_block_ref<K>(&self, block: &BlockRef<K>) -> Result<(), Error> {
        if block.flowgraph_id != self.id {
            return Err(Error::ValidationError(format!(
                "block {:?} belongs to flowgraph {}, not {}",
                block.id, block.flowgraph_id, self.id
            )));
        }
        if self.blocks.get(block.id.0).map(|entry| entry.placement) != Some(block.placement) {
            return Err(Error::InvalidBlock(block.id));
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn validate_local_domain(&self, domain: LocalDomain) -> Result<usize, Error> {
        if domain.flowgraph_id != self.id {
            return Err(Error::ValidationError(format!(
                "local domain belongs to flowgraph {}, not {}",
                domain.flowgraph_id, self.id
            )));
        }
        if domain.domain_id >= self.local_domains.len() {
            return Err(Error::ValidationError("invalid local domain".to_string()));
        }
        Ok(domain.domain_id)
    }

    fn placement(&self, block_id: BlockId) -> Result<BlockPlacement, Error> {
        self.blocks
            .get(block_id.0)
            .map(|entry| entry.placement)
            .ok_or(Error::InvalidBlock(block_id))
    }

    fn raw_block(&self, block_id: BlockId) -> Result<&dyn BlockObject, Error> {
        match self.placement(block_id)? {
            BlockPlacement::Normal { .. } => self
                .blocks
                .get(block_id.0)
                .ok_or(Error::InvalidBlock(block_id))?
                .block
                .as_ref()
                .map(|block| block.as_ref() as &dyn BlockObject)
                .ok_or(Error::LockError),
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local { .. } => Err(Error::LockError),
        }
    }

    fn raw_block_mut(&mut self, block_id: BlockId) -> Result<&mut dyn BlockObject, Error> {
        match self.placement(block_id)? {
            BlockPlacement::Normal { .. } => self
                .blocks
                .get_mut(block_id.0)
                .ok_or(Error::InvalidBlock(block_id))?
                .block
                .as_mut()
                .map(|block| block.as_mut() as &mut dyn BlockObject)
                .ok_or(Error::LockError),
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local { .. } => Err(Error::LockError),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn get_typed_wrapped_block_by_id<K: 'static>(
        &self,
        block_id: BlockId,
    ) -> Result<&WrappedKernel<K>, Error> {
        let block = self.raw_block(block_id)?;
        block
            .as_any()
            .downcast_ref::<WrappedKernel<K>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    block_id,
                    std::any::type_name::<K>()
                ))
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn get_typed_wrapped_block_mut_by_id<K: 'static>(
        &mut self,
        block_id: BlockId,
    ) -> Result<&mut WrappedKernel<K>, Error> {
        let block = self.raw_block_mut(block_id)?;
        block
            .as_any_mut()
            .downcast_mut::<WrappedKernel<K>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    block_id,
                    std::any::type_name::<K>()
                ))
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn get_two_typed_wrapped_blocks_mut<KS, KD>(
        &mut self,
        src_id: BlockId,
        dst_id: BlockId,
    ) -> Result<(&mut WrappedKernel<KS>, &mut WrappedKernel<KD>), Error>
    where
        KS: 'static,
        KD: 'static,
    {
        if src_id == dst_id {
            return Err(Error::LockError);
        }

        let len = self.blocks.len();
        let invalid_block = if src_id.0 >= len { src_id } else { dst_id };
        let [src_slot, dst_slot] =
            self.blocks
                .get_disjoint_mut([src_id.0, dst_id.0])
                .map_err(|err| match err {
                    std::slice::GetDisjointMutError::IndexOutOfBounds => {
                        Error::InvalidBlock(invalid_block)
                    }
                    std::slice::GetDisjointMutError::OverlappingIndices => Error::LockError,
                })?;

        let src = src_slot
            .block
            .as_mut()
            .ok_or(Error::LockError)?
            .as_mut()
            .as_any_mut()
            .downcast_mut::<WrappedKernel<KS>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    src_id,
                    std::any::type_name::<KS>()
                ))
            })?;
        let dst = dst_slot
            .block
            .as_mut()
            .ok_or(Error::LockError)?
            .as_mut()
            .as_any_mut()
            .downcast_mut::<WrappedKernel<KD>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    dst_id,
                    std::any::type_name::<KD>()
                ))
            })?;

        Ok((src, dst))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn local_kernel_ref<K: 'static>(
        block: &dyn BlockObject,
        block_id: BlockId,
        kind: LocalBlockKind,
    ) -> Result<&K, Error> {
        match kind {
            LocalBlockKind::Kernel => block
                .as_any()
                .downcast_ref::<WrappedKernel<K>>()
                .map(|block| &block.kernel),
            LocalBlockKind::LocalKernel => block
                .as_any()
                .downcast_ref::<WrappedLocalKernel<K>>()
                .map(|block| &block.kernel),
        }
        .ok_or_else(|| {
            Error::ValidationError(format!(
                "local block {:?} has unexpected type for {}",
                block_id,
                std::any::type_name::<K>()
            ))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn local_kernel_mut<K: 'static>(
        block: &mut dyn BlockObject,
        block_id: BlockId,
        kind: LocalBlockKind,
    ) -> Result<&mut K, Error> {
        match kind {
            LocalBlockKind::Kernel => block
                .as_any_mut()
                .downcast_mut::<WrappedKernel<K>>()
                .map(|block| &mut block.kernel),
            LocalBlockKind::LocalKernel => block
                .as_any_mut()
                .downcast_mut::<WrappedLocalKernel<K>>()
                .map(|block| &mut block.kernel),
        }
        .ok_or_else(|| {
            Error::ValidationError(format!(
                "local block {:?} has unexpected type for {}",
                block_id,
                std::any::type_name::<K>()
            ))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn local_state_kernel_ref<K: 'static>(
        state: &LocalDomainState,
        local_id: usize,
        block_id: BlockId,
        kind: LocalBlockKind,
    ) -> Result<&K, Error> {
        let block = state.block(local_id, block_id)?;
        Self::local_kernel_ref(block, block_id, kind)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn local_state_kernel_mut<K: 'static>(
        state: &mut LocalDomainState,
        local_id: usize,
        block_id: BlockId,
        kind: LocalBlockKind,
    ) -> Result<&mut K, Error> {
        let block = state.block_mut(local_id, block_id)?;
        Self::local_kernel_mut(block, block_id, kind)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn two_local_state_kernels_mut<KS: 'static, KD: 'static>(
        state: &mut LocalDomainState,
        src: (usize, BlockId, LocalBlockKind),
        dst: (usize, BlockId, LocalBlockKind),
    ) -> Result<(&mut KS, &mut KD), Error> {
        let (src_local, src_id, src_kind) = src;
        let (dst_local, dst_id, dst_kind) = dst;
        let (src_block, dst_block) =
            state.two_blocks_mut((src_local, src_id), (dst_local, dst_id))?;
        let src = Self::local_kernel_mut(src_block, src_id, src_kind)?;
        let dst = Self::local_kernel_mut(dst_block, dst_id, dst_kind)?;
        Ok((src, dst))
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_typed_block<K: 'static>(
        block: &dyn BlockObject,
        block_id: BlockId,
    ) -> Result<(BlockId, &BlockMeta, &K), Error> {
        if let Some(block) = block.as_any().downcast_ref::<WrappedKernel<K>>() {
            return Ok((block.id, &block.meta, &block.kernel));
        }
        if let Some(block) = block.as_any().downcast_ref::<WrappedLocalKernel<K>>() {
            return Ok((block.id, &block.meta, &block.kernel));
        }
        Err(Error::ValidationError(format!(
            "block {:?} has unexpected type for {}",
            block_id,
            std::any::type_name::<K>()
        )))
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_typed_block_mut<K: 'static>(
        block: &mut dyn BlockObject,
        block_id: BlockId,
    ) -> Result<(BlockId, &mut BlockMeta, &mut K), Error> {
        if block.as_any().is::<WrappedKernel<K>>() {
            let block = block
                .as_any_mut()
                .downcast_mut::<WrappedKernel<K>>()
                .expect("checked block type");
            return Ok((block.id, &mut block.meta, &mut block.kernel));
        }
        if block.as_any().is::<WrappedLocalKernel<K>>() {
            let block = block
                .as_any_mut()
                .downcast_mut::<WrappedLocalKernel<K>>()
                .expect("checked block type");
            return Ok((block.id, &mut block.meta, &mut block.kernel));
        }
        Err(Error::ValidationError(format!(
            "block {:?} has unexpected type for {}",
            block_id,
            std::any::type_name::<K>()
        )))
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_kernel_mut<K: 'static>(
        block: &mut dyn BlockObject,
        block_id: BlockId,
    ) -> Result<&mut K, Error> {
        if block.as_any().is::<WrappedKernel<K>>() {
            let block = block
                .as_any_mut()
                .downcast_mut::<WrappedKernel<K>>()
                .expect("checked block type");
            return Ok(&mut block.kernel);
        }
        if block.as_any().is::<WrappedLocalKernel<K>>() {
            let block = block
                .as_any_mut()
                .downcast_mut::<WrappedLocalKernel<K>>()
                .expect("checked block type");
            return Ok(&mut block.kernel);
        }
        Err(Error::ValidationError(format!(
            "block {:?} has unexpected type for {}",
            block_id,
            std::any::type_name::<K>()
        )))
    }

    #[cfg(target_arch = "wasm32")]
    fn get_two_typed_kernels_mut<KS, KD>(
        &mut self,
        src_id: BlockId,
        dst_id: BlockId,
    ) -> Result<(&mut KS, &mut KD), Error>
    where
        KS: 'static,
        KD: 'static,
    {
        if src_id == dst_id {
            return Err(Error::LockError);
        }

        let len = self.blocks.len();
        let invalid_block = if src_id.0 >= len { src_id } else { dst_id };
        let [src_slot, dst_slot] =
            self.blocks
                .get_disjoint_mut([src_id.0, dst_id.0])
                .map_err(|err| match err {
                    std::slice::GetDisjointMutError::IndexOutOfBounds => {
                        Error::InvalidBlock(invalid_block)
                    }
                    std::slice::GetDisjointMutError::OverlappingIndices => Error::LockError,
                })?;

        let src_block = src_slot
            .block
            .as_mut()
            .ok_or(Error::LockError)?
            .as_mut();
        let dst_block = dst_slot
            .block
            .as_mut()
            .ok_or(Error::LockError)?
            .as_mut();
        let src = Self::wasm_kernel_mut(src_block, src_id)?;
        let dst = Self::wasm_kernel_mut(dst_block, dst_id)?;

        Ok((src, dst))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn get_typed_block_by_id<K: 'static>(
        &self,
        block_id: BlockId,
    ) -> Result<TypedBlockGuard<'_, K>, Error> {
        let wrapped = self.get_typed_wrapped_block_by_id(block_id)?;
        Ok(TypedBlockGuard {
            id: wrapped.id,
            meta: &wrapped.meta,
            kernel: &wrapped.kernel,
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn get_typed_block_by_id<K: 'static>(
        &self,
        block_id: BlockId,
    ) -> Result<TypedBlockGuard<'_, K>, Error> {
        let block = self.raw_block(block_id)?;
        let (id, meta, kernel) = Self::wasm_typed_block(block, block_id)?;
        Ok(TypedBlockGuard { id, meta, kernel })
    }

    fn get_typed_block<K: 'static>(
        &self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuard<'_, K>, Error> {
        self.validate_block_ref(block)?;
        self.get_typed_block_by_id(block.id)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn get_typed_block_mut<K: 'static>(
        &mut self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuardMut<'_, K>, Error> {
        self.validate_block_ref(block)?;
        let wrapped = self.get_typed_wrapped_block_mut_by_id::<K>(block.id)?;
        Ok(TypedBlockGuardMut {
            id: wrapped.id,
            meta: &mut wrapped.meta,
            kernel: &mut wrapped.kernel,
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn get_typed_block_mut<K: 'static>(
        &mut self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuardMut<'_, K>, Error> {
        self.validate_block_ref(block)?;
        let raw = self.raw_block_mut(block.id)?;
        let (id, meta, kernel) = Self::wasm_typed_block_mut(raw, block.id)?;
        Ok(TypedBlockGuardMut { id, meta, kernel })
    }

    /// Get typed shared access to a block in this flowgraph.
    ///
    /// The reference must have been returned by this flowgraph. Access fails
    /// while a native local-domain block is running because its state lives on
    /// the local-domain thread.
    pub fn block<K: 'static>(&self, block: &BlockRef<K>) -> Result<TypedBlockGuard<'_, K>, Error> {
        self.get_typed_block(block)
    }

    /// Get typed mutable access to a block in this flowgraph.
    ///
    /// Use this before startup to configure block state or metadata, or after
    /// [`Runtime::run`](crate::runtime::Runtime::run) returns the finished
    /// flowgraph. It cannot borrow a block while the runtime has taken
    /// ownership of the block tasks.
    pub fn block_mut<K: 'static>(
        &mut self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuardMut<'_, K>, Error> {
        self.get_typed_block_mut(block)
    }

    fn connect_stream_ports<B: BufferWriter>(
        src_port: &mut B,
        dst_port: &mut B::Reader,
    ) -> StreamEdge {
        let edge = StreamEdge {
            src_block: src_port.block_id(),
            src_port: src_port.port_id(),
            dst_block: dst_port.block_id(),
            dst_port: dst_port.port_id(),
        };
        src_port.connect(dst_port);
        edge
    }

    fn connect_stream_ports_dyn(
        src_block_id: BlockId,
        src_port_id: &PortId,
        src_block: &mut dyn BlockObject,
        dst_block_id: BlockId,
        dst_port_id: &PortId,
        dst_block: &mut dyn BlockObject,
    ) -> Result<StreamEdge, Error> {
        let reader = dst_block.stream_input(dst_port_id).map_err(|e| match e {
            Error::InvalidStreamPort(_, port) => {
                Error::InvalidStreamPort(crate::runtime::BlockPortCtx::Id(dst_block_id), port)
            }
            o => o,
        })?;

        src_block
            .connect_stream_output(src_port_id, reader)
            .map_err(|e| match e {
                Error::InvalidStreamPort(_, port) => {
                    Error::InvalidStreamPort(crate::runtime::BlockPortCtx::Id(src_block_id), port)
                }
                o => o,
            })?;

        Ok(StreamEdge {
            src_block: src_block_id,
            src_port: src_port_id.clone(),
            dst_block: dst_block_id,
            dst_port: dst_port_id.clone(),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_local_local_stream<KS, KD, B, FS, FD>(
        &self,
        src: LocalEndpoint,
        src_port: FS,
        dst: LocalEndpoint,
        dst_port: FD,
    ) -> Result<StreamEdge, Error>
    where
        KS: 'static,
        KD: 'static,
        B: BufferWriter,
        FS: FnOnce(&mut KS) -> &mut B + Send + 'static,
        FD: FnOnce(&mut KD) -> &mut B::Reader + Send + 'static,
    {
        if src.domain_id != dst.domain_id {
            return Err(Error::ValidationError(
                "stream connections between different local domains are not supported".to_string(),
            ));
        }
        let domain = self
            .local_domains
            .get(src.domain_id)
            .ok_or(Error::InvalidBlock(src.block_id))?;
        domain.exec(move |state| {
            let (src, dst) = Self::two_local_state_kernels_mut::<KS, KD>(
                state,
                (src.local_id, src.block_id, src.kind),
                (dst.local_id, dst.block_id, dst.kind),
            )?;
            Ok(Self::connect_stream_ports(src_port(src), dst_port(dst)))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_cross_local_stream<KS, KD, B, FS, FD>(
        &self,
        src: LocalEndpoint,
        src_port: FS,
        dst: LocalEndpoint,
        dst_port: FD,
    ) -> Result<StreamEdge, Error>
    where
        KS: 'static,
        KD: 'static,
        B: SendBufferWriter + Default + 'static,
        FS: FnOnce(&mut KS) -> &mut B + Send + 'static,
        FD: FnOnce(&mut KD) -> &mut B::Reader + Send + 'static,
    {
        let src_handle = self
            .local_domains
            .get(src.domain_id)
            .ok_or(Error::InvalidBlock(src.block_id))?
            .handle();
        let dst_handle = self
            .local_domains
            .get(dst.domain_id)
            .ok_or(Error::InvalidBlock(dst.block_id))?
            .handle();

        src_handle.exec(move |state| {
            let src = Self::local_state_kernel_mut::<KS>(
                state,
                src.local_id,
                src.block_id,
                src.kind,
            )?;
            let src_port = src_port(src);
            let writer = Arc::new(Mutex::new(Some(std::mem::take(src_port))));
            let dst_writer = Arc::clone(&writer);

            let edge_result = dst_handle.exec(move |state| {
                let mut writer_guard = dst_writer.lock().map_err(|_| Error::LockError)?;
                let writer = writer_guard.as_mut().ok_or(Error::LockError)?;
                let dst = Self::local_state_kernel_mut::<KD>(
                    state,
                    dst.local_id,
                    dst.block_id,
                    dst.kind,
                )?;
                Ok(Self::connect_stream_ports(writer, dst_port(dst)))
            });

            let mut writer_guard = writer.lock().map_err(|_| Error::LockError)?;
            *src_port = writer_guard.take().ok_or(Error::LockError)?;
            edge_result
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wrapped_kernel_mut<K: 'static>(
        block: &mut dyn BlockObject,
        block_id: BlockId,
    ) -> Result<&mut WrappedKernel<K>, Error> {
        block
            .as_any_mut()
            .downcast_mut::<WrappedKernel<K>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    block_id,
                    std::any::type_name::<K>()
                ))
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn with_normal_local_blocks_mut<R, F>(
        &mut self,
        normal_id: BlockId,
        local: LocalEndpoint,
        f: F,
    ) -> Result<R, Error>
    where
        F: FnOnce(&mut dyn BlockObject, &mut dyn BlockObject) -> Result<R, Error> + Send + 'static,
        R: Send + 'static,
    {
        let normal = self.blocks[normal_id.0].block.take().ok_or(Error::LockError)?;
        let normal = Arc::new(Mutex::new(Some(normal)));
        let domain_normal = Arc::clone(&normal);

        let result = self.local_domains[local.domain_id].exec(move |state| {
            let mut normal_guard = domain_normal.lock().map_err(|_| Error::LockError)?;
            let normal = normal_guard.as_mut().ok_or(Error::LockError)?;
            let local = state.block_mut(local.local_id, local.block_id)?;
            f(normal.as_mut(), local)
        });

        self.blocks[normal_id.0].block = normal.lock().map_err(|_| Error::LockError)?.take();
        result
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_local_normal_stream<KS, KD, B, FS, FD>(
        &mut self,
        src: LocalEndpoint,
        src_port: FS,
        dst_id: BlockId,
        dst_port: FD,
    ) -> Result<StreamEdge, Error>
    where
        KS: 'static,
        KD: 'static,
        B: SendBufferWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut B + Send + 'static,
        FD: FnOnce(&mut KD) -> &mut B::Reader + Send + 'static,
    {
        self.with_normal_local_blocks_mut(dst_id, src, move |dst, src_block| {
            let src = Self::local_kernel_mut::<KS>(src_block, src.block_id, src.kind)?;
            let dst = Self::wrapped_kernel_mut::<KD>(dst, dst_id)?;
            Ok(Self::connect_stream_ports(
                src_port(src),
                dst_port(&mut dst.kernel),
            ))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_normal_local_stream<KS, KD, B, FS, FD>(
        &mut self,
        src_id: BlockId,
        src_port: FS,
        dst: LocalEndpoint,
        dst_port: FD,
    ) -> Result<StreamEdge, Error>
    where
        KS: 'static,
        KD: 'static,
        B: SendBufferWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut B + Send + 'static,
        FD: FnOnce(&mut KD) -> &mut B::Reader + Send + 'static,
    {
        self.with_normal_local_blocks_mut(src_id, dst, move |src, dst_block| {
            let src = Self::wrapped_kernel_mut::<KS>(src, src_id)?;
            let dst = Self::local_kernel_mut::<KD>(dst_block, dst.block_id, dst.kind)?;
            Ok(Self::connect_stream_ports(
                src_port(&mut src.kernel),
                dst_port(dst),
            ))
        })
    }

    fn connect_normal_normal_stream_dyn(
        &mut self,
        src_block_id: BlockId,
        src_port_id: &PortId,
        dst_block_id: BlockId,
        dst_port_id: &PortId,
    ) -> Result<StreamEdge, Error> {
        if src_block_id == dst_block_id {
            return Err(Error::LockError);
        }
        let len = self.blocks.len();
        let invalid_block = if src_block_id.0 >= len {
            src_block_id
        } else {
            dst_block_id
        };
        let [src_slot, dst_slot] = self
            .blocks
            .get_disjoint_mut([src_block_id.0, dst_block_id.0])
            .map_err(|err| match err {
                std::slice::GetDisjointMutError::IndexOutOfBounds => {
                    Error::InvalidBlock(invalid_block)
                }
                std::slice::GetDisjointMutError::OverlappingIndices => Error::LockError,
            })?;
        let src_block = src_slot
            .block
            .as_mut()
            .map(Box::as_mut)
            .ok_or(Error::LockError)?;
        let dst_block = dst_slot
            .block
            .as_mut()
            .map(Box::as_mut)
            .ok_or(Error::LockError)?;
        Self::connect_stream_ports_dyn(
            src_block_id,
            src_port_id,
            src_block,
            dst_block_id,
            dst_port_id,
            dst_block,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_local_local_stream_dyn(
        &self,
        src: LocalEndpoint,
        src_port_id: PortId,
        dst: LocalEndpoint,
        dst_port_id: PortId,
    ) -> Result<StreamEdge, Error> {
        if src.domain_id != dst.domain_id {
            return Err(Error::ValidationError(
                "stream connections between different local domains are not supported".to_string(),
            ));
        }
        let domain = self
            .local_domains
            .get(src.domain_id)
            .ok_or(Error::InvalidBlock(src.block_id))?;
        domain.exec(move |state| {
            let (src_block, dst_block) = state
                .two_blocks_mut((src.local_id, src.block_id), (dst.local_id, dst.block_id))?;
            Self::connect_stream_ports_dyn(
                src.block_id,
                &src_port_id,
                src_block,
                dst.block_id,
                &dst_port_id,
                dst_block,
            )
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_local_normal_stream_dyn(
        &mut self,
        src: LocalEndpoint,
        src_port_id: PortId,
        dst_id: BlockId,
        dst_port_id: PortId,
    ) -> Result<StreamEdge, Error> {
        self.with_normal_local_blocks_mut(
            dst_id,
            src,
            move |dst_block, src_block| {
                Self::connect_stream_ports_dyn(
                    src.block_id,
                    &src_port_id,
                    src_block,
                    dst_id,
                    &dst_port_id,
                    dst_block,
                )
            },
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect_normal_local_stream_dyn(
        &mut self,
        src_id: BlockId,
        src_port_id: PortId,
        dst: LocalEndpoint,
        dst_port_id: PortId,
    ) -> Result<StreamEdge, Error> {
        self.with_normal_local_blocks_mut(
            src_id,
            dst,
            move |src_block, dst_block| {
                Self::connect_stream_ports_dyn(
                    src_id,
                    &src_port_id,
                    src_block,
                    dst.block_id,
                    &dst_port_id,
                    dst_block,
                )
            },
        )
    }

    /// Connect stream ports through typed block handles owned by this flowgraph.
    ///
    /// This is the typed block-level stream API used by the
    /// [`connect`](crate::runtime::macros::connect) macro.
    ///
    /// On native targets the selected writer must be send-capable and
    /// default-constructible. Use
    /// [`Flowgraph::stream_local`] for local-only buffers in a local domain.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stream<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: 'static,
        KD: 'static,
        B: SendBufferWriter + Default + 'static,
        FS: FnOnce(&mut KS) -> &mut B + Send + 'static,
        FD: FnOnce(&mut KD) -> &mut B::Reader + Send + 'static,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        let src_id = src_block.id;
        let dst_id = dst_block.id;
        let edge = match (src_block.placement, dst_block.placement) {
            (BlockPlacement::Normal { .. }, BlockPlacement::Normal { .. }) => {
                let (src, dst) = self.get_two_typed_wrapped_blocks_mut(src_id, dst_id)?;
                Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel))
            }
            (
                BlockPlacement::Local {
                    domain_id: src_domain,
                    local_id: src_local,
                    kind: src_kind,
                    ..
                },
                BlockPlacement::Local {
                    domain_id: dst_domain,
                    local_id: dst_local,
                    kind: dst_kind,
                    ..
                },
            ) => {
                if src_domain == dst_domain {
                    self.connect_local_local_stream::<KS, KD, B, FS, FD>(
                        LocalEndpoint::new(src_id, src_domain, src_local, src_kind),
                        src_port,
                        LocalEndpoint::new(dst_id, dst_domain, dst_local, dst_kind),
                        dst_port,
                    )?
                } else {
                    self.connect_cross_local_stream::<KS, KD, B, FS, FD>(
                        LocalEndpoint::new(src_id, src_domain, src_local, src_kind),
                        src_port,
                        LocalEndpoint::new(dst_id, dst_domain, dst_local, dst_kind),
                        dst_port,
                    )?
                }
            }
            (
                BlockPlacement::Local {
                    domain_id,
                    local_id,
                    kind,
                    ..
                },
                BlockPlacement::Normal { .. },
            ) => self.connect_local_normal_stream::<KS, KD, B, FS, FD>(
                LocalEndpoint::new(src_id, domain_id, local_id, kind),
                src_port,
                dst_id,
                dst_port,
            )?,
            (
                BlockPlacement::Normal { .. },
                BlockPlacement::Local {
                    domain_id,
                    local_id,
                    kind,
                    ..
                },
            ) => self.connect_normal_local_stream::<KS, KD, B, FS, FD>(
                src_id,
                src_port,
                LocalEndpoint::new(dst_id, domain_id, local_id, kind),
                dst_port,
            )?,
        };
        self.stream_edges.push(edge);
        Ok(())
    }

    /// Connect local-only stream ports through typed block handles owned by this flowgraph.
    ///
    /// On native targets this only accepts two local-domain blocks in the same
    /// [`LocalDomain`]. Use this for non-`Send` stream buffers such as
    /// [`LocalCpuWriter`](crate::runtime::buffer::LocalCpuWriter).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stream_local<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: 'static,
        KD: 'static,
        B: BufferWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut B + Send + 'static,
        FD: FnOnce(&mut KD) -> &mut B::Reader + Send + 'static,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        let src_id = src_block.id;
        let dst_id = dst_block.id;
        let edge = match (src_block.placement, dst_block.placement) {
            (
                BlockPlacement::Local {
                    domain_id: src_domain,
                    local_id: src_local,
                    kind: src_kind,
                    ..
                },
                BlockPlacement::Local {
                    domain_id: dst_domain,
                    local_id: dst_local,
                    kind: dst_kind,
                    ..
                },
            ) => self.connect_local_local_stream::<KS, KD, B, FS, FD>(
                LocalEndpoint::new(src_id, src_domain, src_local, src_kind),
                src_port,
                LocalEndpoint::new(dst_id, dst_domain, dst_local, dst_kind),
                dst_port,
            )?,
            _ => {
                return Err(Error::ValidationError(
                    "local stream connections require source and destination blocks in the same local domain"
                        .to_string(),
                ));
            }
        };
        self.stream_edges.push(edge);
        Ok(())
    }

    /// Connect stream ports through typed block handles owned by this flowgraph.
    ///
    /// On WASM all blocks run on the browser executor, so this accepts the
    /// primary local buffer traits directly.
    #[cfg(target_arch = "wasm32")]
    pub fn stream<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: 'static,
        KD: 'static,
        B: BufferWriter,
        FS: FnOnce(&mut KS) -> &mut B,
        FD: FnOnce(&mut KD) -> &mut B::Reader,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        let (src, dst) = self.get_two_typed_kernels_mut(src_block.id, dst_block.id)?;
        let edge = Self::connect_stream_ports(src_port(src), dst_port(dst));
        self.stream_edges.push(edge);
        Ok(())
    }

    /// Connect local-only stream ports through typed block handles owned by this flowgraph.
    ///
    /// On WASM this is an alias for [`Flowgraph::stream`], since the runtime is
    /// single-threaded and has no native local domains.
    #[cfg(target_arch = "wasm32")]
    pub fn stream_local<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: 'static,
        KD: 'static,
        B: BufferWriter,
        FS: FnOnce(&mut KS) -> &mut B,
        FD: FnOnce(&mut KD) -> &mut B::Reader,
    {
        self.stream(src_block, src_port, dst_block, dst_port)
    }

    /// Close a circuit between already connected circuit-capable buffers.
    ///
    /// Circuit-capable buffers are still connected like normal stream buffers with
    /// [`Flowgraph::stream`]. Closing the circuit is the additional step that
    /// makes the downstream end return buffers to the upstream start.
    ///
    /// This is the typed block-level circuit-closing API used by the
    /// [`connect`](crate::runtime::macros::connect) macro's `<` operator.
    pub fn close_circuit<KS, KD, CW, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: 'static,
        KD: 'static,
        CW: CircuitWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut CW,
        FD: FnOnce(&mut KD) -> &mut CW::CircuitEnd,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (src, dst) = self.get_two_typed_wrapped_blocks_mut(src_block.id, dst_block.id)?;
            src_port(&mut src.kernel).close_circuit(dst_port(&mut dst.kernel));
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (src, dst) = self.get_two_typed_kernels_mut(src_block.id, dst_block.id)?;
            src_port(src).close_circuit(dst_port(dst));
        }
        Ok(())
    }

    /// Connect stream ports without static port type checks.
    ///
    /// This function only does runtime checks. If the stream ports exist and have compatible
    /// types and sample types, that will only be checked during runtime.
    ///
    /// If possible, it is, therefore, recommended to use the typed API
    /// ([Flowgraph::stream]).
    ///
    /// This function can be helpful when using types is not practical. For example, when a runtime
    /// option switches between different block types, which is often used to switch between
    /// reading samples from hardware or a file.
    ///
    /// ```
    /// use anyhow::Result;
    /// use futuresdr::blocks::Head;
    /// use futuresdr::blocks::NullSink;
    /// use futuresdr::blocks::NullSource;
    /// use futuresdr::prelude::*;
    ///
    /// fn main() -> Result<()> {
    ///     let mut fg = Flowgraph::new();
    ///
    ///     let src = NullSource::<u8>::new();
    ///     let head = Head::<u8>::new(1234);
    ///     let snk = NullSink::<u8>::new();
    ///
    ///     let src = fg.add(src);
    ///     let head = fg.add(head);
    ///
    ///     // untyped stream connect
    ///     fg.stream_dyn(src, "output", head, "input")?;
    ///     // typed connect
    ///     connect!(fg, head > snk);
    ///
    ///     Runtime::new().run(fg)?;
    ///     Ok(())
    /// }
    /// ```
    pub fn stream_dyn(
        &mut self,
        src_block_id: impl Into<BlockId>,
        src_port_id: impl Into<PortId>,
        dst_block_id: impl Into<BlockId>,
        dst_port_id: impl Into<PortId>,
    ) -> Result<(), Error> {
        let src_block_id = src_block_id.into();
        let src_port_id = src_port_id.into();
        let dst_block_id = dst_block_id.into();
        let dst_port_id = dst_port_id.into();

        let edge = match (self.placement(src_block_id)?, self.placement(dst_block_id)?) {
            (BlockPlacement::Normal { .. }, BlockPlacement::Normal { .. }) => self
                .connect_normal_normal_stream_dyn(
                    src_block_id,
                    &src_port_id,
                    dst_block_id,
                    &dst_port_id,
                )?,
            #[cfg(not(target_arch = "wasm32"))]
            (BlockPlacement::Local { .. }, BlockPlacement::Local { .. }) => {
                return Err(Error::ValidationError(
                    "stream_dyn does not connect local-local streams; use stream_local_dyn for same-domain local stream buffers"
                        .to_string(),
                ));
            }
            #[cfg(not(target_arch = "wasm32"))]
            (
                BlockPlacement::Local {
                    domain_id,
                    local_id,
                    kind,
                },
                BlockPlacement::Normal { .. },
            ) => self.connect_local_normal_stream_dyn(
                LocalEndpoint::new(src_block_id, domain_id, local_id, kind),
                src_port_id,
                dst_block_id,
                dst_port_id,
            )?,
            #[cfg(not(target_arch = "wasm32"))]
            (
                BlockPlacement::Normal { .. },
                BlockPlacement::Local {
                    domain_id,
                    local_id,
                    kind,
                },
            ) => self.connect_normal_local_stream_dyn(
                src_block_id,
                src_port_id,
                LocalEndpoint::new(dst_block_id, domain_id, local_id, kind),
                dst_port_id,
            )?,
        };

        self.stream_edges.push(edge);
        Ok(())
    }

    /// Connect local-only stream ports without static port type checks.
    ///
    /// On native targets this only accepts two local-domain blocks in the same
    /// [`LocalDomain`]. Use [`Flowgraph::stream_dyn`] for send-capable/default
    /// dynamic stream connections that involve normal runtime blocks.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stream_local_dyn(
        &mut self,
        src_block_id: impl Into<BlockId>,
        src_port_id: impl Into<PortId>,
        dst_block_id: impl Into<BlockId>,
        dst_port_id: impl Into<PortId>,
    ) -> Result<(), Error> {
        let src_block_id = src_block_id.into();
        let src_port_id = src_port_id.into();
        let dst_block_id = dst_block_id.into();
        let dst_port_id = dst_port_id.into();

        let edge = match (self.placement(src_block_id)?, self.placement(dst_block_id)?) {
            (
                BlockPlacement::Local {
                    domain_id: src_domain,
                    local_id: src_local,
                    kind: src_kind,
                },
                BlockPlacement::Local {
                    domain_id: dst_domain,
                    local_id: dst_local,
                    kind: dst_kind,
                },
            ) => self.connect_local_local_stream_dyn(
                LocalEndpoint::new(src_block_id, src_domain, src_local, src_kind),
                src_port_id,
                LocalEndpoint::new(dst_block_id, dst_domain, dst_local, dst_kind),
                dst_port_id,
            )?,
            _ => {
                return Err(Error::ValidationError(
                    "local dynamic stream connections require source and destination blocks in the same local domain"
                        .to_string(),
                ));
            }
        };

        self.stream_edges.push(edge);
        Ok(())
    }

    /// Connect local-only stream ports without static port type checks.
    ///
    /// On WASM this is an alias for [`Flowgraph::stream_dyn`], since the
    /// runtime is single-threaded and has no native local domains.
    #[cfg(target_arch = "wasm32")]
    pub fn stream_local_dyn(
        &mut self,
        src_block_id: impl Into<BlockId>,
        src_port_id: impl Into<PortId>,
        dst_block_id: impl Into<BlockId>,
        dst_port_id: impl Into<PortId>,
    ) -> Result<(), Error> {
        self.stream_dyn(src_block_id, src_port_id, dst_block_id, dst_port_id)
    }

    /// Connect a message output port to a message input port.
    ///
    /// Message connections are type-erased and may form arbitrary topologies,
    /// including cycles and self-connections. The destination message input is
    /// validated immediately; the source output is validated when the connection
    /// is registered with the source block.
    pub fn message(
        &mut self,
        src_block_id: impl Into<BlockId>,
        src_port_id: impl Into<PortId>,
        dst_block_id: impl Into<BlockId>,
        dst_port_id: impl Into<PortId>,
    ) -> Result<(), Error> {
        let src_block_id = src_block_id.into();
        let src_port_id = src_port_id.into();
        let dst_block_id = dst_block_id.into();
        let dst_port_id = dst_port_id.into();

        let dst_inputs = self
            .blocks
            .get(dst_block_id.0)
            .and_then(|entry| entry.message_inputs)
            .ok_or(Error::InvalidBlock(dst_block_id))?;
        if !dst_inputs.contains(&dst_port_id.name()) {
            return Err(Error::InvalidMessagePort(
                BlockPortCtx::Id(dst_block_id),
                dst_port_id.clone(),
            ));
        }
        let dst_box = self
            .blocks
            .get(dst_block_id.0)
            .and_then(|entry| entry.inbox.as_ref())
            .cloned()
            .ok_or(Error::InvalidBlock(dst_block_id))?;
        match self.placement(src_block_id)? {
            BlockPlacement::Normal { .. } => {
                let src_block = self.raw_block_mut(src_block_id)?;
                src_block.connect(&src_port_id, dst_box, &dst_port_id)?;
            }
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local {
                domain_id,
                local_id,
                ..
            } => {
                let src_port = src_port_id.clone();
                let dst_port = dst_port_id.clone();
                self.local_domains[domain_id].exec(move |state| {
                    let src_block = state.block_mut(local_id, src_block_id)?;
                    src_block.connect(&src_port, dst_box, &dst_port)
                })?;
            }
        }
        self.message_edges
            .push((src_block_id, src_port_id, dst_block_id, dst_port_id));
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_blocks(&mut self) -> Result<Vec<Box<dyn Block>>, Error> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for entry in self.blocks.iter_mut() {
            if let Some(block) = entry.block.take() {
                blocks.push(block);
            }
        }
        Ok(blocks)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn take_blocks(&mut self) -> Result<Vec<Box<dyn LocalBlock>>, Error> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for entry in self.blocks.iter_mut() {
            if let Some(block) = entry.block.take() {
                blocks.push(block);
            }
        }
        Ok(blocks)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn spawn_wasm_blocks(
        &mut self,
        main_channel: crate::runtime::channel::mpsc::Sender<crate::runtime::FlowgraphMessage>,
    ) -> Result<Vec<WasmBlockTask>, Error> {
        let blocks = self.take_blocks()?;
        let mut tasks = Vec::with_capacity(blocks.len());
        for block in blocks {
            let main_channel = main_channel.clone();
            let task = crate::runtime::scheduler::Task::spawn_local(async move {
                let mut block = block;
                let id = block.as_ref().id();
                block.as_mut().run(main_channel).await;
                (id, block)
            });
            tasks.push(task);
        }
        Ok(tasks)
    }

    pub(crate) fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub(crate) fn inboxes(
        &self,
    ) -> Result<(Vec<Option<crate::runtime::dev::BlockInbox>>, Vec<BlockId>), Error> {
        let mut inboxes = vec![None; self.block_count()];
        let mut ids = Vec::new();
        for (id, inbox) in inboxes.iter_mut().enumerate().take(self.block_count()) {
            let block_id = BlockId(id);
            *inbox = Some(
                self.blocks
                    .get(id)
                    .and_then(|entry| entry.inbox.as_ref())
                    .cloned()
                    .ok_or(Error::InvalidBlock(block_id))?,
            );
            ids.push(block_id);
        }
        Ok((inboxes, ids))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn run_local_domains(
        &mut self,
        main_channel: crate::runtime::channel::mpsc::Sender<crate::runtime::FlowgraphMessage>,
    ) -> Result<Vec<oneshot::Receiver<Result<(), Error>>>, Error> {
        let mut tasks = Vec::new();
        for domain in self.local_domains.iter_mut() {
            if let Some(task) = domain.run_if_needed(main_channel.clone())? {
                tasks.push(task);
            }
        }
        Ok(tasks)
    }

    pub(crate) fn stream_edge_endpoints(&self) -> Vec<(BlockId, PortId, BlockId, PortId)> {
        self.stream_edges
            .iter()
            .map(StreamEdge::endpoints)
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn restore_blocks(
        &mut self,
        blocks: Vec<(BlockId, Box<dyn Block>)>,
    ) -> Result<(), Error> {
        for (id, block) in blocks {
            let entry = self.blocks.get_mut(id.0).ok_or(Error::InvalidBlock(id))?;
            if entry.block.is_some() {
                return Err(Error::RuntimeError(format!(
                    "block slot {:?} was restored more than once",
                    id
                )));
            }
            entry.block = Some(block);
        }

        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn restore_blocks(
        &mut self,
        blocks: Vec<(BlockId, Box<dyn LocalBlock>)>,
    ) -> Result<(), Error> {
        for (id, block) in blocks {
            let entry = self.blocks.get_mut(id.0).ok_or(Error::InvalidBlock(id))?;
            if entry.block.is_some() {
                return Err(Error::RuntimeError(format!(
                    "block slot {:?} was restored more than once",
                    id
                )));
            }
            entry.block = Some(block);
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn join_local_domains(
        &mut self,
        tasks: Vec<oneshot::Receiver<Result<(), Error>>>,
    ) -> Result<(), Error> {
        let mut result = Ok(());
        for task in tasks {
            let task_result = task
                .await
                .map_err(|_| Error::RuntimeError("local domain task canceled".to_string()))
                .and_then(|result| result);
            if result.is_ok() {
                result = task_result;
            }
        }
        for domain in self.local_domains.iter_mut() {
            domain.mark_stopped();
        }
        result
    }
}

impl Default for Flowgraph {
    fn default() -> Self {
        Self::new()
    }
}
