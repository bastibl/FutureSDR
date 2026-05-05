use std::any::Any;
use std::fmt;

use crate::runtime::BlockId;
use crate::runtime::Error;
use crate::runtime::FlowgraphMessage;
use crate::runtime::PortId;
use crate::runtime::Result;
use crate::runtime::buffer::BufferReader;
use crate::runtime::dev::BlockInbox;
use futuresdr::runtime::channel::mpsc::Sender;

/// Object-safe runtime interface shared by normal and local block wrappers.
pub trait BlockObject: Any {
    /// Return this block as [`Any`] for downcasting.
    fn as_any(&self) -> &dyn Any;
    /// Return this block as mutable [`Any`] for downcasting.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Get the sender-side inbox of the block.
    fn inbox(&self) -> BlockInbox;
    /// Get the block id.
    fn id(&self) -> BlockId;

    /// Get a type-erased stream input by port id.
    fn stream_input(&mut self, id: &PortId) -> Result<&mut dyn BufferReader, Error>;
    /// Connect a type-erased stream output by downcasting the destination reader.
    fn connect_stream_output(
        &mut self,
        id: &PortId,
        reader: &mut dyn BufferReader,
    ) -> Result<(), Error>;

    /// Message input port names declared by this block.
    fn message_inputs(&self) -> &'static [&'static str];
    /// Connect one message output port to a downstream block inbox.
    fn connect(
        &mut self,
        src_port: &PortId,
        sender: BlockInbox,
        dst_port: &PortId,
    ) -> Result<(), Error>;

    /// Get the static type name of the block.
    fn type_name(&self) -> &str;
    /// Check whether this block is blocking.
    fn is_blocking(&self) -> bool;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait LocalBlock: BlockObject {
    async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>);
}

impl fmt::Debug for dyn LocalBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalBlock")
            .field("type_name", &self.type_name().to_string())
            .finish()
    }
}

pub struct StoredLocalBlock {
    block: Box<dyn LocalBlock>,
}

impl StoredLocalBlock {
    pub(crate) fn new(block: Box<dyn LocalBlock>) -> Self {
        Self { block }
    }

    pub(crate) fn as_ref(&self) -> &dyn LocalBlock {
        self.block.as_ref()
    }

    pub(crate) fn as_mut(&mut self) -> &mut dyn LocalBlock {
        self.block.as_mut()
    }
}

// Local blocks are only executed by the local-flowgraph path. Marking the
// storage Send keeps Flowgraph movable through existing APIs while the local
// path prevents async start on scheduler worker threads.
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for StoredLocalBlock {}
