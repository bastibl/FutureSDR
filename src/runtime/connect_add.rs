use crate::runtime::BlockRef;
use crate::runtime::Error;
use crate::runtime::Flowgraph;
use crate::runtime::Result;
#[cfg(target_arch = "wasm32")]
use crate::runtime::dev::Kernel;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::dev::SendKernel;
#[cfg(target_arch = "wasm32")]
use crate::runtime::kernel_interface::KernelInterface;
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
    K: SendKernel + SendKernelInterface + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        Ok(fg.add(self))
    }
}

#[cfg(target_arch = "wasm32")]
impl<K> ConnectAdd for K
where
    K: Kernel + KernelInterface + 'static,
{
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        Ok(fg.add(self))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<K: SendKernel + 'static> ConnectAdd for BlockRef<K> {
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        self.with(&*fg, |_| ())?;
        Ok(self)
    }
}

#[cfg(target_arch = "wasm32")]
impl<K: Kernel + 'static> ConnectAdd for BlockRef<K> {
    type Added = BlockRef<K>;

    fn connect_add(self, fg: &mut Flowgraph) -> Result<Self::Added, Error> {
        self.with(&*fg, |_| ())?;
        Ok(self)
    }
}
