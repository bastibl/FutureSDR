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
use crate::runtime::config::config;
use burn::prelude::*;
use burn::tensor::BasicOps;
use burn::tensor::TensorKind;
use futures::prelude::*;
use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

enum BufferState<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    Tensor(Tensor<B, 1, E>),
    Data(TensorData),
    Empty,
}

/// In-place buffer
pub struct Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    valid: usize,
    state: BufferState<B, E>,
    device: B::Device,
    tags: Vec<ItemTag>,
}

impl<B, E> Buffer<B, E>
where
    B: Backend,
    E: TensorKind<B> + BasicOps<B> + Send + Sync + 'static,
    E::Elem: CpuSample,
{
    /// Create buffer
    fn with_items(items: usize, device: &B::Device) -> Self {
        let data = TensorData::zeros::<E::Elem, _>([items]);
        Self {
            valid: 0,
            state: BufferState::Data(data),
            device: device.clone(),
            tags: Vec::new(),
        }
    }

    /// Create a Buffer from a Tensor
    pub fn from_tensor(tensor: Tensor<B, 1, E>) -> Self {
        let device = tensor.device();
        Self {
            valid: tensor.shape().num_elements(),
            state: BufferState::Tensor(tensor),
            device,
            tags: Vec::new(),
        }
    }

    /// Consume the buffer to create a Tensor
    pub fn into_tensor(self) -> Tensor<B, 1, E> {
        match self.state {
            BufferState::Tensor(t) => t.slice(0..self.valid),
            BufferState::Data(d) => Tensor::from_data(d, &self.device).slice(0..self.valid),
            BufferState::Empty => unreachable!(),
        }
    }

    fn ensure_data(&mut self) {
        if matches!(self.state, BufferState::Tensor(_))
            && let BufferState::Tensor(t) = std::mem::replace(&mut self.state, BufferState::Empty)
        {
            self.state = BufferState::Data(t.into_data());
        }
    }

    fn num_elements(&self) -> usize {
        match &self.state {
            BufferState::Tensor(t) => t.shape().num_elements(),
            BufferState::Data(d) => d.num_elements(),
            BufferState::Empty => unreachable!(),
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
        self.ensure_data();
        match self.state {
            BufferState::Data(ref mut d) => &mut d.as_mut_slice().unwrap()[0..self.valid],
            _ => unreachable!(),
        }
    }

    fn slice_with_tags(&mut self) -> (&mut [Self::Item], &mut Vec<ItemTag>) {
        self.ensure_data();
        match self.state {
            BufferState::Data(ref mut d) => (
                &mut d.as_mut_slice().unwrap()[0..self.valid],
                &mut self.tags,
            ),
            _ => unreachable!(),
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
    #[allow(clippy::type_complexity)]
    inbound: Arc<Mutex<Vec<Option<Buffer<B, E>>>>>,
    outbound: Arc<Mutex<VecDeque<Buffer<B, E>>>>,
    buffer_size_in_items: usize,
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
            buffer_size_in_items: config().buffer_size / std::mem::size_of::<E::Elem>(),
            device: None,
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
        end.circuit_start = Some((self.writer_inbox.clone(), self.inbound.clone()));
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
        self.inbound.lock().unwrap().pop().and_then(|b| {
            if let Some(mut b) = b {
                b.set_valid(b.num_elements());
                Some(b)
            } else if let Some(ref d) = self.device {
                let mut b = Buffer::with_items(self.buffer_size_in_items, d);
                b.set_valid(b.num_elements());
                Some(b)
            } else {
                warn!("cannot create buffers/tensors, device not set");
                None
            }
        })
    }

    fn has_more_buffers(&mut self) -> bool {
        !self.inbound.lock().unwrap().is_empty()
    }

    fn inject_buffers_with_items(&mut self, n_buffers: usize, n_items: usize) {
        self.buffer_size_in_items = n_items;
        if let Some(ref d) = self.device {
            let mut q = self.inbound.lock().unwrap();
            for _ in 0..n_buffers {
                q.push(Some(Buffer::with_items(n_items, d)));
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
    #[allow(clippy::type_complexity)]
    circuit_start: Option<(Sender<BlockMessage>, Arc<Mutex<Vec<Option<Buffer<B, E>>>>>)>,
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
            circuit_start: None,
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

    fn put_empty_buffer(&mut self, buffer: Self::Buffer) {
        if let Some((ref mut inbox, ref buffers)) = self.circuit_start {
            buffers.lock().unwrap().push(Some(buffer));
            let _ = inbox.try_send(BlockMessage::Notify);
        }
    }

    fn notify_consumed_buffer(&mut self) {
        if let Some((ref mut inbox, ref buffers)) = self.circuit_start {
            buffers.lock().unwrap().push(None);
            let _ = inbox.try_send(BlockMessage::Notify);
        }
    }
}
