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
use crate::runtime::config;
use burn::prelude::*;
use burn::tensor::BasicOps;
use burn::tensor::TensorKind;
use futures::prelude::*;
use std::any::Any;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;

/// In-place buffer
pub struct Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    valid: usize,
    tensor: Tensor<B, 1, E>,
    data: TensorData,
    tags: Vec<ItemTag>,
}

impl<B, E> Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    /// Set the number of valid items in the buffer
    pub fn new(device: &B::Device) -> Self {
        Self::with_items(config::config().buffer_size / std::mem::size_of::<E>(), device)
    }
    /// Set the number of valid items in the buffer
    pub fn with_items(items: usize, device: &B::Device) -> Self {
        let tensor = Tensor::empty([items], device);
        let data = tensor.to_data();
        Self {
            valid: 0,
            tensor,
            data,
            tags: Vec::new(),
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
        (&mut self.data.as_mut_slice().unwrap()[0..self.valid], &mut self.tags)
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
    outbound: Arc<Mutex<VecDeque<Buffer<B, E>>>>,
    // dummy to return when no buffer available
    buffer_size_in_items: Option<usize>,
    device: Option<Device<B>>,
    n_buffers: Arc<AtomicUsize>,
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
            outbound: Arc::new(Mutex::new(VecDeque::new())),
            buffer_size_in_items: None,
            device: None,
            n_buffers: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Set backend device
    ///
    /// This is required to create tensors
    pub fn set_device(&mut self, device: &B::Device) {
        self.device = Some(device.clone());
    }

    /// Close Circuit
    pub fn close_circuit(&mut self, end: &mut Reader<B, E>) {
        end.n_buffers = self.n_buffers.clone();
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
        if let Some(ref d) = self.device {
            let n = self.n_buffers.load(Ordering::SeqCst);
            if n > 0 {
                self.n_buffers.fetch_sub(1, Ordering::SeqCst);
                if let Some(s) = self.buffer_size_in_items {
                    Some(Buffer::with_items(s, d))
                } else {
                    Some(Buffer::new(d))
                }
            } else {
                None
            }
        } else {
            warn!("cannot create tensor, device not set");
            None
        }
    }

    fn has_more_buffers(&mut self) -> bool {
       self.n_buffers.load(Ordering::SeqCst) > 0
    }

    fn inject_buffers_with_items(&mut self, n_buffers: usize, n_items: usize) {
        self.buffer_size_in_items = Some(n_items);
        self.n_buffers.fetch_add(n_buffers, Ordering::SeqCst);
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
    n_buffers: Arc<AtomicUsize>,
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
            n_buffers: Arc::new(AtomicUsize::new(0)),
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

    fn put_empty_buffer(&mut self, _buffer: Self::Buffer) {
        self.n_buffers.fetch_add(1, Ordering::SeqCst);
        let _ = self.writer_inbox.try_send(BlockMessage::Notify);
    }
}

