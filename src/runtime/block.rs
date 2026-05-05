use std::fmt;

use crate::runtime::FlowgraphMessage;
use crate::runtime::local_block::BlockObject;
use futuresdr::runtime::channel::mpsc::Sender;

#[async_trait::async_trait]
/// Runtime object-safe interface for wrapped kernel instances.
///
/// Custom blocks implement [`Kernel`](crate::runtime::dev::Kernel); this trait
/// is implemented by the normal runtime wrapper around send-capable kernels and
/// is mainly useful for runtime extensions.
pub trait Block: BlockObject + Send {
    /// Run the block.
    async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>);
}

pub(crate) type BoxBlock = Box<dyn Block>;

pub(crate) type DynBlock = dyn Block;

impl fmt::Debug for dyn Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("type_name", &self.type_name().to_string())
            .finish()
    }
}
