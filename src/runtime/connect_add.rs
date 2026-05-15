use crate::runtime::BlockRef;
use crate::runtime::Error;
use crate::runtime::Flowgraph;
use crate::runtime::Result;
use crate::runtime::dev::SendKernel;
use crate::runtime::flowgraph::LocalDomain;
use crate::runtime::flowgraph::LocalDomainContext;
use crate::runtime::kernel_interface::SendKernelInterface;

#[doc(hidden)]
pub trait AddLocal: Sized {
    #[cfg(not(target_arch = "wasm32"))]
    fn add_local(
        block: impl FnOnce() -> Self + Send + 'static,
        fg: &mut Flowgraph,
        domain: LocalDomain,
    ) -> BlockRef<Self>;

    async fn add_local_async(
        block: impl FnOnce() -> Self + Send + 'static,
        fg: &mut Flowgraph,
        domain: LocalDomain,
    ) -> BlockRef<Self>;

    fn add_domain(block: Self, ctx: &LocalDomainContext<'_>) -> BlockRef<Self>;
}

#[doc(hidden)]
pub trait ConnectAdd<B> {
    type Added;

    fn connect_add(self, block: B) -> Result<Self::Added, Error>;
}

#[doc(hidden)]
pub trait ConnectAddAsync<B> {
    type Added;

    async fn connect_add_async(self, block: B) -> Result<Self::Added, Error>;
}

impl<K> ConnectAdd<K> for &mut Flowgraph
where
    K: SendKernel + SendKernelInterface + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, block: K) -> Result<Self::Added, Error> {
        Ok(self.add(block))
    }
}

impl<K> ConnectAddAsync<K> for &mut Flowgraph
where
    K: SendKernel + SendKernelInterface + 'static,
{
    type Added = BlockRef<K>;

    async fn connect_add_async(self, block: K) -> Result<Self::Added, Error> {
        Ok(self.add_async(block).await)
    }
}

impl<K: 'static> ConnectAdd<BlockRef<K>> for &mut Flowgraph {
    type Added = BlockRef<K>;

    fn connect_add(self, block: BlockRef<K>) -> Result<Self::Added, Error> {
        self.validate_block_ref(&block)?;
        Ok(block)
    }
}

impl<K: 'static> ConnectAddAsync<BlockRef<K>> for &mut Flowgraph {
    type Added = BlockRef<K>;

    async fn connect_add_async(self, block: BlockRef<K>) -> Result<Self::Added, Error> {
        self.validate_block_ref(&block)?;
        Ok(block)
    }
}

impl<'ctx, 'borrow, K> ConnectAdd<K> for &'borrow LocalDomainContext<'ctx>
where
    K: AddLocal + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, block: K) -> Result<Self::Added, Error> {
        Ok(self.add(block))
    }
}

impl<'ctx, 'borrow, K> ConnectAddAsync<K> for &'borrow LocalDomainContext<'ctx>
where
    K: AddLocal + 'static,
{
    type Added = BlockRef<K>;

    async fn connect_add_async(self, block: K) -> Result<Self::Added, Error> {
        Ok(self.add(block))
    }
}

impl<'ctx, 'borrow, K: 'static> ConnectAdd<BlockRef<K>> for &'borrow LocalDomainContext<'ctx> {
    type Added = BlockRef<K>;

    fn connect_add(self, block: BlockRef<K>) -> Result<Self::Added, Error> {
        Ok(block)
    }
}

impl<'ctx, 'borrow, K: 'static> ConnectAddAsync<BlockRef<K>> for &'borrow LocalDomainContext<'ctx> {
    type Added = BlockRef<K>;

    async fn connect_add_async(self, block: BlockRef<K>) -> Result<Self::Added, Error> {
        Ok(block)
    }
}
