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

#[async_trait::async_trait(?Send)]
#[allow(dead_code)]
pub(crate) trait LocalBlock: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>);
    fn inbox(&self) -> BlockInbox;
    fn id(&self) -> BlockId;

    fn stream_input(&mut self, id: &PortId) -> Result<&mut dyn BufferReader, Error>;
    fn connect_stream_output(
        &mut self,
        id: &PortId,
        reader: &mut dyn BufferReader,
    ) -> Result<(), Error>;

    fn message_inputs(&self) -> &'static [&'static str];
    fn connect(
        &mut self,
        src_port: &PortId,
        sender: BlockInbox,
        dst_port: &PortId,
    ) -> Result<(), Error>;

    fn instance_name(&self) -> Option<&str>;
    fn set_instance_name(&mut self, name: &str);
    fn type_name(&self) -> &str;
    fn is_blocking(&self) -> bool;
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
