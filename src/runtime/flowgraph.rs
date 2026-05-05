use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ops::DerefMut;
#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::runtime::BlockId;
use crate::runtime::BlockPortCtx;
use crate::runtime::BufferId;
use crate::runtime::Error;
use crate::runtime::FlowgraphId;
use crate::runtime::PortId;
use crate::runtime::Result;
use crate::runtime::RuntimeId;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::block::BoxBlock;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::block::DynBlock;
use crate::runtime::buffer::BufferReader;
use crate::runtime::buffer::BufferWriter;
use crate::runtime::buffer::CircuitWriter;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::buffer::SendBufferWriter;
use crate::runtime::dev::BlockMeta;
use crate::runtime::dev::Kernel;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::dev::SendKernel;
use crate::runtime::kernel_interface::KernelInterface;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::kernel_interface::SendKernelInterface;
use crate::runtime::local_block::BlockObject;
#[cfg(target_arch = "wasm32")]
use crate::runtime::local_block::LocalBlock;
use crate::runtime::local_block::StoredLocalBlock;
use crate::runtime::wrapped_kernel::WrappedKernel;

static NEXT_FLOWGRAPH_ID: AtomicUsize = AtomicUsize::new(0);
static NEXT_BUFFER_ID: AtomicUsize = AtomicUsize::new(0);

mod normal_block {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) type Stored = super::BoxBlock;
    #[cfg(target_arch = "wasm32")]
    pub(crate) type Stored = super::StoredLocalBlock;

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) type Dyn = super::DynBlock;
    #[cfg(target_arch = "wasm32")]
    pub(crate) type Dyn = dyn super::LocalBlock;

    pub(crate) fn as_ref(block: &Stored) -> &Dyn {
        block.as_ref()
    }

    pub(crate) fn as_mut(block: &mut Stored) -> &mut Dyn {
        block.as_mut()
    }
}

pub(crate) use normal_block::Stored as NormalStoredBlock;

type NormalWrappedKernel<K> = WrappedKernel<K>;

/// Shared typed access to a block stored inside a [`Flowgraph`].
///
/// The guard dereferences to the block's kernel type and also exposes runtime
/// metadata such as the block id and instance name. It is only available before
/// the flowgraph is moved into a running [`Runtime`](crate::runtime::Runtime).
pub struct TypedBlockGuard<'a, K: Kernel> {
    wrapped: &'a NormalWrappedKernel<K>,
}

/// Mutable typed access to a block stored inside a [`Flowgraph`].
///
/// The guard dereferences to the block's kernel type and can be used to update
/// block state or metadata before the flowgraph is started.
pub struct TypedBlockGuardMut<'a, K: Kernel> {
    wrapped: &'a mut NormalWrappedKernel<K>,
}

impl<K: Kernel> TypedBlockGuard<'_, K> {
    /// Get the block id.
    pub fn id(&self) -> BlockId {
        self.wrapped.id
    }

    /// Get block metadata.
    pub fn meta(&self) -> &BlockMeta {
        &self.wrapped.meta
    }

    /// Get the block instance name.
    pub fn instance_name(&self) -> Option<&str> {
        self.wrapped.meta.instance_name()
    }
}

impl<K: Kernel> Deref for TypedBlockGuard<'_, K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.wrapped.kernel
    }
}

impl<K: Kernel> TypedBlockGuardMut<'_, K> {
    /// Get the block id.
    pub fn id(&self) -> BlockId {
        self.wrapped.id
    }

    /// Get block metadata.
    pub fn meta(&self) -> &BlockMeta {
        &self.wrapped.meta
    }

    /// Mutably access block metadata.
    pub fn meta_mut(&mut self) -> &mut BlockMeta {
        &mut self.wrapped.meta
    }

    /// Get the block instance name.
    pub fn instance_name(&self) -> Option<&str> {
        self.wrapped.meta.instance_name()
    }

    /// Set the block instance name.
    pub fn set_instance_name(&mut self, name: &str) {
        self.wrapped.meta.set_instance_name(name);
    }
}

impl<K: Kernel> Deref for TypedBlockGuardMut<'_, K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.wrapped.kernel
    }
}

impl<K: Kernel> DerefMut for TypedBlockGuardMut<'_, K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.wrapped.kernel
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

/// Typed reference to a block that was added to a local domain.
#[cfg(not(target_arch = "wasm32"))]
pub type LocalBlockRef<K> = BlockRef<K>;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum BlockPlacement {
    Normal {
        domain_id: usize,
    },
    #[cfg(not(target_arch = "wasm32"))]
    Local {
        domain_id: usize,
        local_id: usize,
        auto: bool,
    },
}

/// Type-erased stream edge with a globally unique buffer id.
#[derive(Debug, Clone)]
pub(crate) struct StreamEdge {
    pub(crate) buffer_id: BufferId,
    pub(crate) src_block: BlockId,
    pub(crate) src_port: PortId,
    pub(crate) dst_block: BlockId,
    pub(crate) dst_port: PortId,
}

impl StreamEdge {
    pub(crate) fn endpoints(&self) -> (BlockId, PortId, BlockId, PortId) {
        let _ = self.buffer_id;
        (
            self.src_block,
            self.src_port.clone(),
            self.dst_block,
            self.dst_port.clone(),
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct LocalDomainBlocks {
    pub(crate) blocks: Vec<Option<StoredLocalBlock>>,
}

/// Builder for a local scheduling domain inside a [`Flowgraph`].
#[cfg(not(target_arch = "wasm32"))]
pub struct LocalDomain {
    fg: *mut Flowgraph,
    flowgraph_id: FlowgraphId,
    domain_id: usize,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl<K> BlockRef<K> {
    /// Get the block id.
    pub fn id(&self) -> BlockId {
        self.id
    }
}

impl<K: Kernel + 'static> BlockRef<K> {
    /// Get a typed handle to the block stored in the given [`Flowgraph`].
    pub fn get<'a>(&self, fg: &'a Flowgraph) -> Result<TypedBlockGuard<'a, K>, Error> {
        fg.block(self)
    }

    /// Access the typed block through the given [`Flowgraph`].
    pub fn with<R>(&self, fg: &Flowgraph, f: impl FnOnce(&K) -> R) -> Result<R, Error> {
        let block = fg.block(self)?;
        Ok(f(&block))
    }

    /// Mutably access the typed block through the given [`Flowgraph`].
    pub fn with_mut<R>(&self, fg: &mut Flowgraph, f: impl FnOnce(&mut K) -> R) -> Result<R, Error> {
        let mut block = fg.block_mut(self)?;
        Ok(f(&mut block))
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
    pub(crate) runtime_id: Option<RuntimeId>,
    pub(crate) blocks: Vec<Option<NormalStoredBlock>>,
    pub(crate) block_placements: Vec<BlockPlacement>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) local_domains: Vec<LocalDomainBlocks>,
    pub(crate) stream_edges: Vec<StreamEdge>,
    pub(crate) message_edges: Vec<(BlockId, PortId, BlockId, PortId)>,
}

impl Flowgraph {
    /// Create an empty [`Flowgraph`].
    pub fn new() -> Flowgraph {
        Self::new_with_runtime(None)
    }

    pub(crate) fn new_with_runtime(runtime_id: Option<RuntimeId>) -> Flowgraph {
        Flowgraph {
            id: FlowgraphId(NEXT_FLOWGRAPH_ID.fetch_add(1, Ordering::Relaxed)),
            runtime_id,
            blocks: Vec::new(),
            block_placements: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            local_domains: vec![LocalDomainBlocks { blocks: Vec::new() }],
            stream_edges: vec![],
            message_edges: vec![],
        }
    }

    /// Create a local scheduling domain.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn local_domain(&mut self) -> LocalDomain {
        let domain_id = self.local_domains.len();
        self.local_domains
            .push(LocalDomainBlocks { blocks: Vec::new() });
        LocalDomain {
            fg: self as *mut Flowgraph,
            flowgraph_id: self.id,
            domain_id,
            _not_send_or_sync: PhantomData,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn auto_local_domain(&mut self) -> usize {
        let domain_id = self.local_domains.len();
        self.local_domains
            .push(LocalDomainBlocks { blocks: Vec::new() });
        domain_id
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn ensure_default_local_domain(&mut self) {
        if self.local_domains.is_empty() {
            self.local_domains
                .push(LocalDomainBlocks { blocks: Vec::new() });
        }
    }

    /// Add a block and return a typed reference to it.
    ///
    /// The returned [`BlockRef`] can be used for explicit typed connections or
    /// for inspecting/mutating the block before the flowgraph is started.
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
        let placement = if <K as KernelInterface>::is_blocking() {
            let domain_id = self.auto_local_domain();
            let local_id = self.local_domains[domain_id].blocks.len();
            self.blocks.push(None);
            self.local_domains[domain_id]
                .blocks
                .push(Some(StoredLocalBlock::new(Box::new(b))));
            BlockPlacement::Local {
                domain_id,
                local_id,
                auto: true,
            }
        } else {
            self.blocks.push(Some(Box::new(b)));
            BlockPlacement::Normal { domain_id: 0 }
        };
        self.block_placements.push(placement);
        BlockRef {
            id: block_id,
            flowgraph_id: self.id,
            placement,
            _marker: PhantomData,
        }
    }

    /// Add a block and return a typed reference to it.
    ///
    /// On WASM, the browser executor is single-threaded, so ordinary flowgraph
    /// blocks are stored in the local block representation.
    #[cfg(target_arch = "wasm32")]
    pub fn add<K>(&mut self, block: K) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        let block_id = BlockId(self.blocks.len());
        let mut b = WrappedKernel::new(block, block_id);
        let block_name = <K as KernelInterface>::type_name().to_string();
        b.meta
            .set_instance_name(format!("{}-{}", block_name, block_id.0));
        self.blocks.push(Some(StoredLocalBlock::new(Box::new(b))));
        let placement = BlockPlacement::Normal { domain_id: 0 };
        self.block_placements.push(placement);
        BlockRef {
            id: block_id,
            flowgraph_id: self.id,
            placement,
            _marker: PhantomData,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn reserve_block_id(&mut self, placement: BlockPlacement) -> BlockId {
        let block_id = BlockId(self.blocks.len());
        self.blocks.push(None);
        self.block_placements.push(placement);
        block_id
    }

    /// Add a block to the default local domain.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_local<K>(&mut self, block: impl FnOnce() -> K) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        self.ensure_default_local_domain();
        self.add_local_to_domain(0, false, block)
    }

    /// Add a block to an explicit local domain.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_local_to<K>(
        &mut self,
        domain: &LocalDomain,
        block: impl FnOnce() -> K,
    ) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        self.validate_local_domain(domain)
            .expect("local domain belongs to another flowgraph");
        self.add_local_to_domain(domain.domain_id, false, block)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_local_to_domain<K>(
        &mut self,
        domain_id: usize,
        auto: bool,
        block: impl FnOnce() -> K,
    ) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        let local_id = self.local_domains[domain_id].blocks.len();
        let placement = BlockPlacement::Local {
            domain_id,
            local_id,
            auto,
        };
        let block_id = self.reserve_block_id(placement);
        let mut b = WrappedKernel::new(block(), block_id);
        let block_name = <K as KernelInterface>::type_name().to_string();
        b.meta
            .set_instance_name(format!("{}-{}", block_name, block_id.0));
        self.local_domains[domain_id]
            .blocks
            .push(Some(StoredLocalBlock::new(Box::new(b))));
        BlockRef {
            id: block_id,
            flowgraph_id: self.id,
            placement,
            _marker: PhantomData,
        }
    }

    fn validate_block_ref<K>(&self, block: &BlockRef<K>) -> Result<(), Error> {
        if block.flowgraph_id != self.id {
            return Err(Error::ValidationError(format!(
                "block {:?} belongs to flowgraph {}, not {}",
                block.id, block.flowgraph_id, self.id
            )));
        }
        if self.block_placements.get(block.id.0).copied() != Some(block.placement) {
            return Err(Error::InvalidBlock(block.id));
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn validate_local_domain(&self, domain: &LocalDomain) -> Result<(), Error> {
        if domain.flowgraph_id != self.id {
            return Err(Error::ValidationError(format!(
                "local domain belongs to flowgraph {}, not {}",
                domain.flowgraph_id, self.id
            )));
        }
        if domain.domain_id >= self.local_domains.len() {
            return Err(Error::ValidationError("invalid local domain".to_string()));
        }
        Ok(())
    }

    fn placement(&self, block_id: BlockId) -> Result<BlockPlacement, Error> {
        self.block_placements
            .get(block_id.0)
            .copied()
            .ok_or(Error::InvalidBlock(block_id))
    }

    fn raw_block(&self, block_id: BlockId) -> Result<&dyn BlockObject, Error> {
        match self.placement(block_id)? {
            BlockPlacement::Normal { .. } => self
                .blocks
                .get(block_id.0)
                .ok_or(Error::InvalidBlock(block_id))?
                .as_ref()
                .map(|block| normal_block::as_ref(block) as &dyn BlockObject)
                .ok_or(Error::LockError),
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local {
                domain_id,
                local_id,
                ..
            } => self
                .local_domains
                .get(domain_id)
                .and_then(|domain| domain.blocks.get(local_id))
                .ok_or(Error::InvalidBlock(block_id))?
                .as_ref()
                .map(|block| block.as_ref() as &dyn BlockObject)
                .ok_or(Error::LockError),
        }
    }

    fn raw_block_mut(&mut self, block_id: BlockId) -> Result<&mut dyn BlockObject, Error> {
        match self.placement(block_id)? {
            BlockPlacement::Normal { .. } => self
                .blocks
                .get_mut(block_id.0)
                .ok_or(Error::InvalidBlock(block_id))?
                .as_mut()
                .map(|block| normal_block::as_mut(block) as &mut dyn BlockObject)
                .ok_or(Error::LockError),
            #[cfg(not(target_arch = "wasm32"))]
            BlockPlacement::Local {
                domain_id,
                local_id,
                ..
            } => self
                .local_domains
                .get_mut(domain_id)
                .and_then(|domain| domain.blocks.get_mut(local_id))
                .ok_or(Error::InvalidBlock(block_id))?
                .as_mut()
                .map(|block| block.as_mut() as &mut dyn BlockObject)
                .ok_or(Error::LockError),
        }
    }

    fn get_typed_wrapped_block_by_id<K: Kernel + 'static>(
        &self,
        block_id: BlockId,
    ) -> Result<&NormalWrappedKernel<K>, Error> {
        let block = self.raw_block(block_id)?;
        block
            .as_any()
            .downcast_ref::<NormalWrappedKernel<K>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    block_id,
                    std::any::type_name::<K>()
                ))
            })
    }

    fn get_typed_wrapped_block_mut_by_id<K: Kernel + 'static>(
        &mut self,
        block_id: BlockId,
    ) -> Result<&mut NormalWrappedKernel<K>, Error> {
        let block = self.raw_block_mut(block_id)?;
        block
            .as_any_mut()
            .downcast_mut::<NormalWrappedKernel<K>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    block_id,
                    std::any::type_name::<K>()
                ))
            })
    }

    fn get_two_typed_wrapped_blocks_mut<KS, KD>(
        &mut self,
        src_id: BlockId,
        dst_id: BlockId,
    ) -> Result<(&mut NormalWrappedKernel<KS>, &mut NormalWrappedKernel<KD>), Error>
    where
        KS: Kernel + 'static,
        KD: Kernel + 'static,
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
            .as_mut()
            .map(normal_block::as_mut)
            .ok_or(Error::LockError)?
            .as_any_mut()
            .downcast_mut::<NormalWrappedKernel<KS>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    src_id,
                    std::any::type_name::<KS>()
                ))
            })?;
        let dst = dst_slot
            .as_mut()
            .map(normal_block::as_mut)
            .ok_or(Error::LockError)?
            .as_any_mut()
            .downcast_mut::<NormalWrappedKernel<KD>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    dst_id,
                    std::any::type_name::<KD>()
                ))
            })?;

        Ok((src, dst))
    }

    fn get_typed_block_by_id<K: Kernel + 'static>(
        &self,
        block_id: BlockId,
    ) -> Result<TypedBlockGuard<'_, K>, Error> {
        Ok(TypedBlockGuard {
            wrapped: self.get_typed_wrapped_block_by_id(block_id)?,
        })
    }

    fn get_typed_block<K: Kernel + 'static>(
        &self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuard<'_, K>, Error> {
        self.validate_block_ref(block)?;
        self.get_typed_block_by_id(block.id)
    }

    fn get_typed_block_mut<K: Kernel + 'static>(
        &mut self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuardMut<'_, K>, Error> {
        self.validate_block_ref(block)?;
        Ok(TypedBlockGuardMut {
            wrapped: self.get_typed_wrapped_block_mut_by_id::<K>(block.id)?,
        })
    }

    /// Get typed shared access to a block in this flowgraph.
    pub fn block<K: Kernel + 'static>(
        &self,
        block: &BlockRef<K>,
    ) -> Result<TypedBlockGuard<'_, K>, Error> {
        self.get_typed_block(block)
    }

    /// Get typed mutable access to a block in this flowgraph.
    pub fn block_mut<K: Kernel + 'static>(
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
            buffer_id: BufferId(NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed)),
            src_block: src_port.block_id(),
            src_port: src_port.port_id(),
            dst_block: dst_port.block_id(),
            dst_port: dst_port.port_id(),
        };
        src_port.connect(dst_port);
        edge
    }

    /// Connect stream ports through typed block handles owned by this flowgraph.
    ///
    /// This is the typed block-level stream API used by the
    /// [connect](futuresdr::runtime::macros::connect) macro.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stream<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: Kernel + 'static,
        KD: Kernel + 'static,
        B: SendBufferWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut B,
        FD: FnOnce(&mut KD) -> &mut B::Reader,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        let edge = match (src_block.placement, dst_block.placement) {
            (BlockPlacement::Normal { .. }, BlockPlacement::Normal { .. }) => {
                let (src, dst) =
                    self.get_two_typed_wrapped_blocks_mut(src_block.id, dst_block.id)?;
                Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel))
            }
            (
                BlockPlacement::Local {
                    domain_id: src_domain,
                    local_id: src_local,
                    ..
                },
                BlockPlacement::Local {
                    domain_id: dst_domain,
                    local_id: dst_local,
                    ..
                },
            ) if src_domain == dst_domain => {
                if src_local == dst_local {
                    return Err(Error::LockError);
                }
                let domain = self
                    .local_domains
                    .get_mut(src_domain)
                    .ok_or(Error::InvalidBlock(src_block.id))?;
                let invalid_block = if src_local >= domain.blocks.len() {
                    src_block.id
                } else {
                    dst_block.id
                };
                let slots = domain.blocks.get_disjoint_mut([src_local, dst_local]);
                let [src_slot, dst_slot] = slots.map_err(|err| match err {
                    std::slice::GetDisjointMutError::IndexOutOfBounds => {
                        Error::InvalidBlock(invalid_block)
                    }
                    std::slice::GetDisjointMutError::OverlappingIndices => Error::LockError,
                })?;
                let src = src_slot
                    .as_mut()
                    .ok_or(Error::LockError)?
                    .as_mut()
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KS>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "local block {:?} has unexpected type for {}",
                            src_block.id,
                            std::any::type_name::<KS>()
                        ))
                    })?;
                let dst = dst_slot
                    .as_mut()
                    .ok_or(Error::LockError)?
                    .as_mut()
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KD>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "local block {:?} has unexpected type for {}",
                            dst_block.id,
                            std::any::type_name::<KD>()
                        ))
                    })?;
                Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel))
            }
            (
                BlockPlacement::Local {
                    domain_id: src_domain,
                    local_id: src_local,
                    ..
                },
                BlockPlacement::Local {
                    domain_id: dst_domain,
                    local_id: dst_local,
                    ..
                },
            ) => {
                let invalid_block = if src_domain >= self.local_domains.len() {
                    src_block.id
                } else {
                    dst_block.id
                };
                let [src_domain, dst_domain] = self
                    .local_domains
                    .get_disjoint_mut([src_domain, dst_domain])
                    .map_err(|err| match err {
                        std::slice::GetDisjointMutError::IndexOutOfBounds => {
                            Error::InvalidBlock(invalid_block)
                        }
                        std::slice::GetDisjointMutError::OverlappingIndices => Error::LockError,
                    })?;
                let src = src_domain
                    .blocks
                    .get_mut(src_local)
                    .and_then(Option::as_mut)
                    .ok_or(Error::InvalidBlock(src_block.id))?
                    .as_mut()
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KS>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "local block {:?} has unexpected type for {}",
                            src_block.id,
                            std::any::type_name::<KS>()
                        ))
                    })?;
                let dst = dst_domain
                    .blocks
                    .get_mut(dst_local)
                    .and_then(Option::as_mut)
                    .ok_or(Error::InvalidBlock(dst_block.id))?
                    .as_mut()
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KD>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "local block {:?} has unexpected type for {}",
                            dst_block.id,
                            std::any::type_name::<KD>()
                        ))
                    })?;
                Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel))
            }
            (
                BlockPlacement::Local {
                    domain_id,
                    local_id,
                    ..
                },
                BlockPlacement::Normal { .. },
            ) => {
                let local_domains = &mut self.local_domains;
                let blocks = &mut self.blocks;
                let src = local_domains[domain_id].blocks[local_id]
                    .as_mut()
                    .ok_or(Error::LockError)?
                    .as_mut()
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KS>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "local block {:?} has unexpected type for {}",
                            src_block.id,
                            std::any::type_name::<KS>()
                        ))
                    })?;
                let dst = blocks
                    .get_mut(dst_block.id.0)
                    .ok_or(Error::InvalidBlock(dst_block.id))?
                    .as_mut()
                    .map(normal_block::as_mut)
                    .ok_or(Error::LockError)?
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KD>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "block {:?} has unexpected type for {}",
                            dst_block.id,
                            std::any::type_name::<KD>()
                        ))
                    })?;
                Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel))
            }
            (
                BlockPlacement::Normal { .. },
                BlockPlacement::Local {
                    domain_id,
                    local_id,
                    ..
                },
            ) => {
                let blocks = &mut self.blocks;
                let local_domains = &mut self.local_domains;
                let src = blocks
                    .get_mut(src_block.id.0)
                    .ok_or(Error::InvalidBlock(src_block.id))?
                    .as_mut()
                    .map(normal_block::as_mut)
                    .ok_or(Error::LockError)?
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KS>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "block {:?} has unexpected type for {}",
                            src_block.id,
                            std::any::type_name::<KS>()
                        ))
                    })?;
                let dst = local_domains[domain_id].blocks[local_id]
                    .as_mut()
                    .ok_or(Error::LockError)?
                    .as_mut()
                    .as_any_mut()
                    .downcast_mut::<WrappedKernel<KD>>()
                    .ok_or_else(|| {
                        Error::ValidationError(format!(
                            "local block {:?} has unexpected type for {}",
                            dst_block.id,
                            std::any::type_name::<KD>()
                        ))
                    })?;
                Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel))
            }
        };
        self.stream_edges.push(edge);
        Ok(())
    }

    /// Connect stream ports through typed block handles owned by this flowgraph.
    #[cfg(target_arch = "wasm32")]
    pub fn stream<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: Kernel + 'static,
        KD: Kernel + 'static,
        B: BufferWriter,
        FS: FnOnce(&mut KS) -> &mut B,
        FD: FnOnce(&mut KD) -> &mut B::Reader,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        let (src, dst) = self.get_two_typed_wrapped_blocks_mut(src_block.id, dst_block.id)?;
        let edge = Self::connect_stream_ports(src_port(&mut src.kernel), dst_port(&mut dst.kernel));
        self.stream_edges.push(edge);
        Ok(())
    }

    /// Close a circuit between already connected circuit-capable buffers.
    ///
    /// Circuit-capable buffers are still connected like normal stream buffers with
    /// [`Flowgraph::stream`]. Closing the circuit is the additional step that
    /// makes the downstream end return buffers to the upstream start.
    ///
    /// This is the typed block-level circuit-closing API used by the
    /// [connect](futuresdr::runtime::macros::connect) macro's `<` operator.
    pub fn close_circuit<KS, KD, CW, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port: FS,
        dst_block: &BlockRef<KD>,
        dst_port: FD,
    ) -> Result<(), Error>
    where
        KS: Kernel + 'static,
        KD: Kernel + 'static,
        CW: CircuitWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut CW,
        FD: FnOnce(&mut KD) -> &mut CW::CircuitEnd,
    {
        self.validate_block_ref(src_block)?;
        self.validate_block_ref(dst_block)?;
        let (src, dst) = self.get_two_typed_wrapped_blocks_mut(src_block.id, dst_block.id)?;
        src_port(&mut src.kernel).close_circuit(dst_port(&mut dst.kernel));
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
            .as_mut()
            .map(normal_block::as_mut)
            .ok_or(Error::LockError)?;
        let dst_block = dst_slot
            .as_mut()
            .map(normal_block::as_mut)
            .ok_or(Error::LockError)?;
        let reader = dst_block.stream_input(&dst_port_id).map_err(|e| match e {
            Error::InvalidStreamPort(_, port) => {
                Error::InvalidStreamPort(crate::runtime::BlockPortCtx::Id(dst_block_id), port)
            }
            o => o,
        })?;

        src_block
            .connect_stream_output(&src_port_id, reader)
            .map_err(|e| match e {
                Error::InvalidStreamPort(_, port) => {
                    Error::InvalidStreamPort(crate::runtime::BlockPortCtx::Id(src_block_id), port)
                }
                o => o,
            })?;

        self.stream_edges.push(StreamEdge {
            buffer_id: BufferId(NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed)),
            src_block: src_block_id,
            src_port: src_port_id,
            dst_block: dst_block_id,
            dst_port: dst_port_id,
        });
        Ok(())
    }

    /// Make message connection
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

        debug_assert_ne!(src_block_id, dst_block_id);

        let dst_block = self.raw_block(dst_block_id)?;
        if !dst_block.message_inputs().contains(&dst_port_id.name()) {
            return Err(Error::InvalidMessagePort(
                BlockPortCtx::Id(dst_block_id),
                dst_port_id.clone(),
            ));
        }
        let dst_box = dst_block.inbox();
        let src_block = self.raw_block_mut(src_block_id)?;
        src_block.connect(&src_port_id, dst_box, &dst_port_id)?;
        self.message_edges
            .push((src_block_id, src_port_id, dst_block_id, dst_port_id));
        Ok(())
    }

    pub(crate) fn take_blocks(&mut self) -> Result<Vec<NormalStoredBlock>, Error> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for slot in self.blocks.iter_mut() {
            if let Some(block) = slot.take() {
                blocks.push(block);
            }
        }
        Ok(blocks)
    }

    pub(crate) fn block_count(&self) -> usize {
        self.block_placements.len()
    }

    pub(crate) fn inboxes(
        &self,
    ) -> Result<(Vec<Option<crate::runtime::dev::BlockInbox>>, Vec<BlockId>), Error> {
        let mut inboxes = vec![None; self.block_count()];
        let mut ids = Vec::new();
        for (id, inbox) in inboxes.iter_mut().enumerate().take(self.block_count()) {
            let block_id = BlockId(id);
            let block = self.raw_block(block_id)?;
            *inbox = Some(block.inbox());
            ids.push(block_id);
        }
        Ok((inboxes, ids))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_local_domains(&mut self) -> Result<Vec<Vec<StoredLocalBlock>>, Error> {
        let mut domains = Vec::with_capacity(self.local_domains.len());
        for domain in self.local_domains.iter_mut() {
            let mut blocks = Vec::new();
            for slot in domain.blocks.iter_mut() {
                if let Some(block) = slot.take() {
                    blocks.push(block);
                }
            }
            domains.push(blocks);
        }
        Ok(domains)
    }

    pub(crate) fn stream_edge_endpoints(&self) -> Vec<(BlockId, PortId, BlockId, PortId)> {
        self.stream_edges
            .iter()
            .map(StreamEdge::endpoints)
            .collect()
    }

    pub(crate) fn restore_blocks(
        &mut self,
        blocks: Vec<(BlockId, NormalStoredBlock)>,
    ) -> Result<(), Error> {
        for (id, block) in blocks {
            let slot = self.blocks.get_mut(id.0).ok_or(Error::InvalidBlock(id))?;
            if slot.is_some() {
                return Err(Error::RuntimeError(format!(
                    "block slot {:?} was restored more than once",
                    id
                )));
            }
            *slot = Some(block);
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn restore_local_blocks(
        &mut self,
        blocks: Vec<(BlockId, StoredLocalBlock)>,
    ) -> Result<(), Error> {
        for (id, block) in blocks {
            let mut restored = Some(block);
            for domain in self.local_domains.iter_mut() {
                for slot in domain.blocks.iter_mut() {
                    if slot.is_none()
                        && restored
                            .as_ref()
                            .is_some_and(|block| block.as_ref().id() == id)
                    {
                        *slot = restored.take();
                        break;
                    }
                }
                if restored.is_none() {
                    break;
                }
            }
            if restored.is_some() {
                return Err(Error::InvalidBlock(id));
            }
        }

        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl LocalDomain {
    /// Add a block to this local domain and return a typed reference to it.
    pub fn add<K>(&mut self, block: impl FnOnce() -> K) -> BlockRef<K>
    where
        K: Kernel + KernelInterface + 'static,
    {
        // SAFETY: LocalDomain values are builder handles created from a unique
        // `&mut Flowgraph`. The public API only exposes synchronous mutation.
        let fg = unsafe { &mut *self.fg };
        fg.add_local_to_domain(self.domain_id, false, block)
    }

    /// Connect two stream ports inside this local domain.
    pub fn stream<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port_fn: FS,
        dst_block: &BlockRef<KD>,
        dst_port_fn: FD,
    ) -> Result<(), Error>
    where
        KS: Kernel + 'static,
        KD: Kernel + 'static,
        B: BufferWriter,
        FS: FnOnce(&mut KS) -> &mut B,
        FD: FnOnce(&mut KD) -> &mut B::Reader,
    {
        let fg = unsafe { &mut *self.fg };
        fg.validate_block_ref(src_block)?;
        fg.validate_block_ref(dst_block)?;
        let (
            BlockPlacement::Local {
                domain_id: src_domain,
                local_id: src_local,
                ..
            },
            BlockPlacement::Local {
                domain_id: dst_domain,
                local_id: dst_local,
                ..
            },
        ) = (src_block.placement, dst_block.placement)
        else {
            return Err(Error::ValidationError(
                "local stream endpoints must be local blocks".to_string(),
            ));
        };
        if src_domain != self.domain_id || dst_domain != self.domain_id {
            return Err(Error::ValidationError(
                "local stream endpoints must be in this local domain".to_string(),
            ));
        }
        if src_local == dst_local {
            return Err(Error::LockError);
        }

        let domain = &mut fg.local_domains[self.domain_id];
        let [src_slot, dst_slot] = domain
            .blocks
            .get_disjoint_mut([src_local, dst_local])
            .map_err(|_| Error::LockError)?;
        let src = src_slot.as_mut().ok_or(Error::LockError)?.as_mut();
        let dst = dst_slot.as_mut().ok_or(Error::LockError)?.as_mut();
        let src = src
            .as_any_mut()
            .downcast_mut::<WrappedKernel<KS>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "local block {:?} has unexpected type for {}",
                    src_block.id,
                    std::any::type_name::<KS>()
                ))
            })?;
        let dst = dst
            .as_any_mut()
            .downcast_mut::<WrappedKernel<KD>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "local block {:?} has unexpected type for {}",
                    dst_block.id,
                    std::any::type_name::<KD>()
                ))
            })?;

        let edge = Flowgraph::connect_stream_ports(
            src_port_fn(&mut src.kernel),
            dst_port_fn(&mut dst.kernel),
        );
        fg.stream_edges.push(edge);
        Ok(())
    }

    /// Connect a local-domain output to a normal flowgraph block input.
    pub fn stream_to_normal<KS, KD, B, FS, FD>(
        &mut self,
        src_block: &BlockRef<KS>,
        src_port_fn: FS,
        dst_block: &BlockRef<KD>,
        dst_port_fn: FD,
    ) -> Result<(), Error>
    where
        KS: Kernel + 'static,
        KD: SendKernel + 'static,
        B: SendBufferWriter + 'static,
        FS: FnOnce(&mut KS) -> &mut B,
        FD: FnOnce(&mut KD) -> &mut B::Reader,
    {
        let fg = unsafe { &mut *self.fg };
        fg.validate_block_ref(src_block)?;
        fg.validate_block_ref(dst_block)?;
        let BlockPlacement::Local {
            domain_id,
            local_id,
            ..
        } = src_block.placement
        else {
            return Err(Error::ValidationError(
                "local source block must be a local block".to_string(),
            ));
        };
        if domain_id != self.domain_id {
            return Err(Error::ValidationError(
                "local source block must be in this local domain".to_string(),
            ));
        }

        let src = fg.local_domains[self.domain_id].blocks[local_id]
            .as_mut()
            .ok_or(Error::LockError)?
            .as_mut()
            .as_any_mut()
            .downcast_mut::<WrappedKernel<KS>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "local block {:?} has unexpected type for {}",
                    src_block.id,
                    std::any::type_name::<KS>()
                ))
            })?;
        let dst_block_id = dst_block.id;
        let dst_slot = fg
            .blocks
            .get_mut(dst_block_id.0)
            .ok_or(Error::InvalidBlock(dst_block_id))?;
        let dst = dst_slot.as_deref_mut().ok_or(Error::LockError)?;
        let dst = dst
            .as_any_mut()
            .downcast_mut::<WrappedKernel<KD>>()
            .ok_or_else(|| {
                Error::ValidationError(format!(
                    "block {:?} has unexpected type for {}",
                    dst_block_id,
                    std::any::type_name::<KD>()
                ))
            })?;
        let edge = Flowgraph::connect_stream_ports(
            src_port_fn(&mut src.kernel),
            dst_port_fn(&mut dst.kernel),
        );
        fg.stream_edges.push(edge);
        Ok(())
    }
}

impl Default for Flowgraph {
    fn default() -> Self {
        Self::new()
    }
}
