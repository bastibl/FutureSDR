use crate::channel::mpsc::Sender;
use crate::channel::mpsc::channel;
use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
use crate::runtime::Error;
use crate::runtime::ItemTag;
use crate::runtime::PortId;
use crate::runtime::buffer::BufferReader;
use crate::runtime::buffer::BufferWriter;
use crate::runtime::buffer::CpuSample;
use crate::runtime::buffer::InplaceBuffer;
use crate::runtime::buffer::InplaceReader;
use crate::runtime::buffer::InplaceWriter;
use burn::prelude::*;
use burn::tensor::BasicOps;
use burn::tensor::TensorKind;
use futures::prelude::*;
use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Clone)]
struct BufferReuse<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    queue: Arc<Mutex<Vec<Buffer<B, E>>>>,
    inbox: Sender<BlockMessage>,
}

/// In-place buffer
pub struct Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    valid: usize,
    pub tensor: Tensor<B, 1, E>,
    pub data: TensorData,
    tags: Vec<ItemTag>,
    reuse: Option<BufferReuse<B, E>>,
}

impl<B, E> Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    /// Create buffer
    fn with_items(items: usize, device: &B::Device, reuse: Option<BufferReuse<B, E>>) -> Self {
        let tensor = Tensor::empty([items], device);
        let data = tensor.to_data();
        Self {
            valid: 0,
            tensor,
            data,
            tags: Vec::new(),
            reuse,
        }
    }
}

impl<B, E> InplaceBuffer for Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    type Item = E::Elem;

    fn set_valid(&mut self, valid: usize) {
        self.valid = valid;
    }

    fn slice(&mut self) -> &mut [Self::Item] {
        &mut self.data.as_mut_slice().unwrap()[0..self.valid]
    }

    fn slice_with_tags(&mut self) -> (&mut [Self::Item], &mut Vec<ItemTag>) {
        (
            &mut self.data.as_mut_slice().unwrap()[0..self.valid],
            &mut self.tags,
        )
    }

    fn detach(&mut self) {
        self.reuse = None;
    }
}

impl<B, E> Drop for Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    fn drop(&mut self) {
        if let Some(r) = self.reuse.take() {
            let mut n = r.inbox.clone();
            r.queue.clone().lock().unwrap().push(Buffer {
                valid: 0,
                tensor: self.tensor.clone(),
                data: self.data.clone(),
                tags: Vec::new(),
                reuse: Some(r),
            });
            let _ = n.try_send(BlockMessage::Notify);
        }
    }
}

/// Circuit Writer
pub struct Writer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    reader_inbox: Sender<BlockMessage>,
    reader_input: PortId,
    writer_inbox: Sender<BlockMessage>,
    writer_id: BlockId,
    writer_output: PortId,
    inbound: Arc<Mutex<Vec<Buffer<B, E>>>>,
    outbound: Arc<Mutex<VecDeque<Buffer<B, E>>>>,
    // dummy to return when no buffer available
    device: Option<Device<B>>,
}

impl<B, E> Writer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    /// Create circuit buffer writer
    pub fn new() -> Self {
        let (rx, _) = channel(0);
        Self {
            reader_inbox: rx.clone(),
            reader_input: PortId::default(),
            writer_inbox: rx,
            writer_id: BlockId::default(),
            writer_output: PortId::default(),
            inbound: Arc::new(Mutex::new(Vec::new())),
            outbound: Arc::new(Mutex::new(VecDeque::new())),
            device: None,
        }
    }

    /// Set backend device
    ///
    /// This is required to create tensors
    pub fn set_device(&mut self, device: &B::Device) {
        self.device = Some(device.clone());
    }
}

impl<B, E> Default for Writer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<B, E> BufferWriter for Writer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    type Reader = Reader<B, E>;

    fn init(&mut self, block_id: BlockId, port_id: PortId, inbox: Sender<BlockMessage>) {
        self.writer_id = block_id;
        self.writer_output = port_id;
        self.writer_inbox = inbox;

        let mut q = self.inbound.lock().unwrap();
        q.iter_mut().for_each(|b| 
            if let Some(ref mut r) = b.reuse {
                r.inbox = self.writer_inbox.clone();
            }
        )
    }

    fn validate(&self) -> Result<(), Error> {
        if self.reader_inbox.is_closed() {
            Err(Error::ValidationError(format!(
                "{:?}:{:?} not connected",
                self.writer_id, self.writer_output
            )))
        } else {
            Ok(())
        }
    }

    fn connect(&mut self, dest: &mut Self::Reader) {
        self.reader_input = dest.reader_input.clone();
        self.reader_inbox = dest.reader_inbox.clone();
        self.outbound = dest.inbound.clone();

        dest.writer_inbox = self.writer_inbox.clone();
        dest.writer_output = self.writer_output.clone();
    }

    async fn notify_finished(&mut self) {
        let _ = self
            .reader_inbox
            .send(BlockMessage::StreamInputDone {
                input_id: self.reader_input.clone(),
            })
            .await;
    }

    fn block_id(&self) -> BlockId {
        self.writer_id
    }

    fn port_id(&self) -> PortId {
        self.writer_output.clone()
    }
}

impl<B, E> InplaceWriter for Writer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    type Item = E::Elem;

    type Buffer = Buffer<B, E>;

    fn put_full_buffer(&mut self, buffer: Self::Buffer) {
        self.outbound.lock().unwrap().push_back(buffer);
        let _ = self.reader_inbox.try_send(BlockMessage::Notify);
    }

    fn get_empty_buffer(&mut self) -> Option<Self::Buffer> {
        self.inbound.lock().unwrap().pop().map(|mut b| {
            b.set_valid(b.data.num_elements());
            b
        })
    }

    fn has_more_buffers(&mut self) -> bool {
        !self.inbound.lock().unwrap().is_empty()
    }

    fn inject_buffers_with_items(&mut self, n_buffers: usize, n_items: usize) {
        if let Some(ref d) = self.device {
            let mut q = self.inbound.lock().unwrap();
            let reuse = BufferReuse {
                queue: self.inbound.clone(),
                inbox: self.writer_inbox.clone(),
            };
            for _ in 0..n_buffers {
                let mut b = Buffer::with_items(n_items, d, Some(reuse.clone()));
                b.set_valid(b.data.num_elements());
                q.push(b);
            }
        } else {
            warn!("cannot create buffers/tensors, device not set");
        }
    }
}

/// Circuit Reader
pub struct Reader<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    reader_inbox: Sender<BlockMessage>,
    reader_id: BlockId,
    reader_input: PortId,
    writer_inbox: Sender<BlockMessage>,
    writer_output: PortId,
    inbound: Arc<Mutex<VecDeque<Buffer<B, E>>>>,
    finished: bool,
}

impl<B, E> Reader<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    /// Create circuit buffer reader
    pub fn new() -> Self {
        let (rx, _) = channel(0);
        Self {
            reader_inbox: rx.clone(),
            reader_id: BlockId::default(),
            reader_input: PortId::default(),
            writer_inbox: rx,
            writer_output: PortId::default(),
            inbound: Arc::new(Mutex::new(VecDeque::new())),
            finished: false,
        }
    }
}

impl<B, E> Default for Reader<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<B, E> BufferReader for Reader<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn init(&mut self, block_id: BlockId, port_id: PortId, inbox: Sender<BlockMessage>) {
        self.reader_id = block_id;
        self.reader_input = port_id;
        self.reader_inbox = inbox;
    }

    fn validate(&self) -> Result<(), Error> {
        if self.writer_inbox.is_closed() {
            Err(Error::ValidationError(format!(
                "{:?}:{:?} not connected",
                self.reader_id, self.reader_input
            )))
        } else {
            Ok(())
        }
    }

    async fn notify_finished(&mut self) {
        let _ = self
            .writer_inbox
            .send(BlockMessage::StreamOutputDone {
                output_id: self.writer_output.clone(),
            })
            .await;
    }

    fn finish(&mut self) {
        self.finished = true;
    }

    fn finished(&self) -> bool {
        self.finished && self.inbound.lock().unwrap().is_empty()
    }

    fn block_id(&self) -> BlockId {
        self.reader_id
    }

    fn port_id(&self) -> PortId {
        self.reader_input.clone()
    }
}

impl<B, E> InplaceReader for Reader<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    type Item = E::Elem;

    type Buffer = Buffer<B, E>;

    fn get_full_buffer(&mut self) -> Option<Self::Buffer> {
        self.inbound.lock().unwrap().pop_front()
    }

    fn has_more_buffers(&mut self) -> bool {
        !self.inbound.lock().unwrap().is_empty()
    }
}
