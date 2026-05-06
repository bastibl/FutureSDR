use crate::runtime::BlockRef;
use crate::runtime::Error;
use crate::runtime::Flowgraph;
use crate::runtime::Result;
use crate::runtime::dev::SendKernel;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::flowgraph::LocalDomain;
use crate::runtime::kernel_interface::SendKernelInterface;

#[doc(hidden)]
pub trait AddLocal: Sized {
    #[cfg(target_arch = "wasm32")]
    fn add_local(self, fg: &mut Flowgraph) -> BlockRef<Self>;

    #[cfg(not(target_arch = "wasm32"))]
    fn add_local(
        block: impl FnOnce() -> Self + Send + 'static,
        domain: &mut LocalDomain,
    ) -> BlockRef<Self>;
}

#[doc(hidden)]
pub trait ConnectAdd {
    type Added;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error>;
}

impl<K> ConnectAdd for K
where
    K: SendKernel + SendKernelInterface + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        Ok(fg.add(self))
    }
}

impl<K: 'static> ConnectAdd for BlockRef<K> {
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        fg.validate_block_ref(&self)?;
        Ok(self)
    }
}
