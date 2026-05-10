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
    fn add_local(
        block: impl FnOnce() -> Self + Send + 'static,
        fg: &mut Flowgraph,
        domain: LocalDomain,
    ) -> BlockRef<Self>;

    fn add_domain(block: Self, ctx: &LocalDomainContext<'_>) -> BlockRef<Self>;
}

#[doc(hidden)]
pub trait ConnectAdd<Target: ?Sized> {
    type Added;

    fn connect_add(self, target: &mut Target) -> Result<Self::Added, Error>;
}

impl<K> ConnectAdd<Flowgraph> for K
where
    K: SendKernel + SendKernelInterface + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        Ok(fg.add(self))
    }
}

impl<K: 'static> ConnectAdd<Flowgraph> for BlockRef<K> {
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        fg.validate_block_ref(&self)?;
        Ok(self)
    }
}

impl<'a, K> ConnectAdd<&LocalDomainContext<'a>> for K
where
    K: AddLocal + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, ctx: &mut &LocalDomainContext<'a>) -> Result<Self::Added, Error> {
        Ok((*ctx).add(self))
    }
}

impl<'a, K: 'static> ConnectAdd<&LocalDomainContext<'a>> for BlockRef<K> {
    type Added = BlockRef<K>;

    fn connect_add(self, _ctx: &mut &LocalDomainContext<'a>) -> Result<Self::Added, Error> {
        Ok(self)
    }
}
