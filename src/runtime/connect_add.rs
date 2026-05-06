use crate::runtime::BlockRef;
use crate::runtime::Error;
use crate::runtime::Flowgraph;
use crate::runtime::Result;
#[cfg(target_arch = "wasm32")]
use crate::runtime::dev::LocalKernel;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::dev::SendKernel;
use crate::runtime::kernel_interface::LocalKernelInterface;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::kernel_interface::SendKernelInterface;

#[doc(hidden)]
pub trait ConnectAdd {
    type Added;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error>;
}

#[cfg(not(target_arch = "wasm32"))]
impl<K> ConnectAdd for K
where
    K: SendKernel + SendKernelInterface + LocalKernelInterface + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        Ok(fg.add(self))
    }
}

#[cfg(target_arch = "wasm32")]
impl<K> ConnectAdd for K
where
    K: LocalKernel + LocalKernelInterface + 'static,
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
